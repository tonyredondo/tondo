//! Shared-corpus conformance probe for the private native `std.encoding` ABI.
//!
//! The companion Tondo fixture exercises the same case IDs on the hosted VM.
//! Native operations use opaque `u64` handles and the exact scalar stdlib
//! kernel; no pointer, object layout, Cranelift lowering or public FFI is
//! inferred from this target-qualified lane.

const RESULT_OK: u64 = 2;
const RESULT_ERR: u64 = 3;
const STATUS_OK: u64 = 0;
const STATUS_ENCODING_ERROR: u64 = 29;
const STATUS_ENCODING_INVALID_OPTIONS: u64 = 30;
const ENCODING_CODEC_BASE64: u64 = 0;
const ENCODING_CODEC_HEX: u64 = 1;
const ENCODING_OPERATION_ENCODE: u64 = 0;
const ENCODING_OPERATION_DECODE: u64 = 1;
const ENCODING_POLICY_STANDARD_REQUIRED: u64 = 0;
const ENCODING_POLICY_URL_OMITTED: u64 = 3;
const ENCODING_POLICY_HEX_LOWER: u64 = 0;
const ENCODING_POLICY_HEX_UPPER: u64 = 1;
const ENCODING_POLICY_HEX_ANY: u64 = 2;
const ERROR_NON_CANONICAL: u64 = 4;
const ERROR_RESOURCE_LIMIT: u64 = 5;
const ERROR_CLOSED: u64 = 7;

fn require(condition: bool, message: &str) {
    assert!(condition, "std.encoding conformance: {message}");
}

fn release(value: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_release(value) == STATUS_OK,
        message,
    );
}

fn buffer(bytes: &[u8]) -> u64 {
    let handle = tondo_native_runtime::tondo_rt_buffer_from_bytes(bytes);
    require(handle != 0, "buffer allocation");
    handle
}

fn result_bytes(result: u64, expected: &[u8], message: &str) {
    require(
        tondo_native_runtime::tondo_rt_result_tag(result) == RESULT_OK,
        message,
    );
    let output = tondo_native_runtime::tondo_rt_result_payload(result);
    let length = tondo_native_runtime::tondo_rt_buffer_len(output);
    require(length == expected.len() as u64, message);
    for (index, expected_byte) in expected.iter().copied().enumerate() {
        require(
            tondo_native_runtime::tondo_rt_buffer_byte(output, index as u64)
                == expected_byte as u64,
            message,
        );
    }
    release(result, "release encoding result");
}

fn result_error(result: u64, expected_kind: u64, expected_offset: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_result_tag(result) == RESULT_ERR
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_ENCODING_ERROR
            && tondo_native_runtime::tondo_rt_encoding_error_kind(result) == expected_kind
            && tondo_native_runtime::tondo_rt_encoding_error_offset(result) == expected_offset,
        message,
    );
    release(result, "release encoding error result");
}

fn clean(case_id: &str) {
    require(tondo_native_runtime::tondo_rt_live_objects() == 0, case_id);
}

fn base64_interoperability() {
    tondo_native_runtime::tondo_rt_reset();
    let source = buffer(b"fo");
    let encoded = tondo_native_runtime::tondo_rt_encoding_materialize(
        ENCODING_CODEC_BASE64,
        ENCODING_OPERATION_ENCODE,
        ENCODING_POLICY_STANDARD_REQUIRED,
        source,
        1024,
        1024,
    );
    result_bytes(encoded, b"Zm8=", "standard Base64 vector");

    let encoded_input = buffer(b"Zm8=");
    let decoded = tondo_native_runtime::tondo_rt_encoding_materialize(
        ENCODING_CODEC_BASE64,
        ENCODING_OPERATION_DECODE,
        ENCODING_POLICY_STANDARD_REQUIRED,
        encoded_input,
        1024,
        1024,
    );
    result_bytes(decoded, b"fo", "standard Base64 round trip");

    let url = tondo_native_runtime::tondo_rt_encoding_materialize(
        ENCODING_CODEC_BASE64,
        ENCODING_OPERATION_ENCODE,
        ENCODING_POLICY_URL_OMITTED,
        source,
        1024,
        1024,
    );
    result_bytes(url, b"Zm8", "URL-safe unpadded vector");
    release(source, "release Base64 source");
    release(encoded_input, "release Base64 input");
    clean("Base64 interoperability cleanup");
    println!(
        r#"{{"id":"base64-interoperability","status":"passed","standard":"Zm8=","url":"Zm8","round_trip":"fo","cleanup":true}}"#
    );
}

fn hex_policy() {
    tondo_native_runtime::tondo_rt_reset();
    let source = buffer(b"fo");
    let lower = tondo_native_runtime::tondo_rt_encoding_materialize(
        ENCODING_CODEC_HEX,
        ENCODING_OPERATION_ENCODE,
        ENCODING_POLICY_HEX_LOWER,
        source,
        1024,
        1024,
    );
    result_bytes(lower, b"666f", "lowercase hexadecimal vector");
    let upper = tondo_native_runtime::tondo_rt_encoding_materialize(
        ENCODING_CODEC_HEX,
        ENCODING_OPERATION_ENCODE,
        ENCODING_POLICY_HEX_UPPER,
        source,
        1024,
        1024,
    );
    result_bytes(upper, b"666F", "uppercase hexadecimal vector");
    let mixed = buffer(b"666F");
    let decoded = tondo_native_runtime::tondo_rt_encoding_materialize(
        ENCODING_CODEC_HEX,
        ENCODING_OPERATION_DECODE,
        ENCODING_POLICY_HEX_ANY,
        mixed,
        1024,
        1024,
    );
    result_bytes(decoded, b"fo", "any-case hexadecimal round trip");
    release(source, "release hexadecimal source");
    release(mixed, "release hexadecimal input");
    clean("hex policy cleanup");
    println!(
        r#"{{"id":"hex-policy","status":"passed","lower":"666f","upper":"666F","round_trip":"fo","cleanup":true}}"#
    );
}

fn streaming_invariance() {
    tondo_native_runtime::tondo_rt_reset();
    let encoder = tondo_native_runtime::tondo_rt_encoding_stream_new(
        ENCODING_CODEC_BASE64,
        ENCODING_OPERATION_ENCODE,
        ENCODING_POLICY_URL_OMITTED,
        1024,
        1024,
    );
    require(encoder != 0, "create Base64 encoder");
    for part in [b"f".as_slice(), b"o".as_slice()] {
        let input = buffer(part);
        let pushed = tondo_native_runtime::tondo_rt_encoding_push(encoder, input);
        result_bytes(pushed, b"", "one-byte Base64 fragment");
        release(input, "release Base64 fragment");
    }
    let tail = tondo_native_runtime::tondo_rt_encoding_finish(encoder);
    result_bytes(tail, b"Zm8", "Base64 stream tail");
    let empty = buffer(b"");
    let closed = tondo_native_runtime::tondo_rt_encoding_push(encoder, empty);
    result_error(closed, ERROR_CLOSED, 0, "finished stream is terminal");
    release(empty, "release empty terminal input");
    release(encoder, "release Base64 encoder");

    let decoder = tondo_native_runtime::tondo_rt_encoding_stream_new(
        ENCODING_CODEC_BASE64,
        ENCODING_OPERATION_DECODE,
        ENCODING_POLICY_STANDARD_REQUIRED,
        1024,
        1024,
    );
    require(decoder != 0, "create Base64 decoder");
    for part in [b"Z".as_slice(), b"g==".as_slice()] {
        let input = buffer(part);
        let pushed = tondo_native_runtime::tondo_rt_encoding_push(decoder, input);
        if part[0] == b'Z' {
            result_bytes(pushed, b"", "decoder carry");
        } else {
            result_bytes(pushed, b"f", "decoder completed quantum");
        }
        release(input, "release Base64 decoder fragment");
    }
    let tail = tondo_native_runtime::tondo_rt_encoding_finish(decoder);
    result_bytes(tail, b"", "decoder empty tail");
    release(decoder, "release Base64 decoder");

    let hex_encoder = tondo_native_runtime::tondo_rt_encoding_stream_new(
        ENCODING_CODEC_HEX,
        ENCODING_OPERATION_ENCODE,
        ENCODING_POLICY_HEX_UPPER,
        1024,
        1024,
    );
    require(hex_encoder != 0, "create hexadecimal encoder");
    let input = buffer(b"fo");
    let pushed = tondo_native_runtime::tondo_rt_encoding_push(hex_encoder, input);
    result_bytes(pushed, b"666F", "hex stream output");
    release(input, "release hexadecimal fragment");
    let tail = tondo_native_runtime::tondo_rt_encoding_finish(hex_encoder);
    result_bytes(tail, b"", "hex stream tail");
    release(hex_encoder, "release hexadecimal encoder");
    clean("streaming cleanup");
    println!(
        r#"{{"id":"streaming-invariance","status":"passed","base64":"Zm8","hex":"666F","terminal":true,"cleanup":true}}"#
    );
}

fn strict_errors() {
    tondo_native_runtime::tondo_rt_reset();
    let noncanonical_base64 = buffer(b"Zh==");
    let base64_error = tondo_native_runtime::tondo_rt_encoding_materialize(
        ENCODING_CODEC_BASE64,
        ENCODING_OPERATION_DECODE,
        ENCODING_POLICY_STANDARD_REQUIRED,
        noncanonical_base64,
        1024,
        1024,
    );
    result_error(
        base64_error,
        ERROR_NON_CANONICAL,
        0,
        "Base64 non-canonical bits",
    );
    release(noncanonical_base64, "release invalid Base64 input");

    let noncanonical_hex = buffer(b"00AB");
    let hex_error = tondo_native_runtime::tondo_rt_encoding_materialize(
        ENCODING_CODEC_HEX,
        ENCODING_OPERATION_DECODE,
        ENCODING_POLICY_HEX_LOWER,
        noncanonical_hex,
        1024,
        1024,
    );
    result_error(
        hex_error,
        ERROR_NON_CANONICAL,
        2,
        "hex case policy error offset",
    );
    release(noncanonical_hex, "release invalid hexadecimal input");
    clean("strict error cleanup");
    println!(
        r#"{{"id":"strict-errors","status":"passed","base64_kind":4,"base64_offset":0,"hex_kind":4,"hex_offset":2,"cleanup":true}}"#
    );
}

fn limits_and_lifecycle() {
    tondo_native_runtime::tondo_rt_reset();
    let source = buffer(b"fo");
    let limited = tondo_native_runtime::tondo_rt_encoding_materialize(
        ENCODING_CODEC_BASE64,
        ENCODING_OPERATION_ENCODE,
        ENCODING_POLICY_STANDARD_REQUIRED,
        source,
        2,
        3,
    );
    result_error(
        limited,
        ERROR_RESOURCE_LIMIT,
        0,
        "materialized output limit",
    );
    release(source, "release limited source");

    let stream = tondo_native_runtime::tondo_rt_encoding_stream_new(
        ENCODING_CODEC_BASE64,
        ENCODING_OPERATION_ENCODE,
        ENCODING_POLICY_STANDARD_REQUIRED,
        2,
        3,
    );
    require(stream != 0, "create limited stream");
    let input = buffer(b"fo");
    let pushed = tondo_native_runtime::tondo_rt_encoding_push(stream, input);
    result_bytes(pushed, b"", "limited stream carry");
    release(input, "release limited stream input");
    let limit_error = tondo_native_runtime::tondo_rt_encoding_finish(stream);
    result_error(limit_error, ERROR_RESOURCE_LIMIT, 0, "stream output limit");
    let closed = tondo_native_runtime::tondo_rt_encoding_finish(stream);
    result_error(closed, ERROR_CLOSED, 0, "limit error closes stream");
    release(stream, "release limited stream");

    let empty = buffer(b"");
    let zero = tondo_native_runtime::tondo_rt_encoding_materialize(
        ENCODING_CODEC_BASE64,
        ENCODING_OPERATION_ENCODE,
        ENCODING_POLICY_STANDARD_REQUIRED,
        empty,
        0,
        0,
    );
    result_bytes(zero, b"", "zero limits accept empty input");
    release(empty, "release zero-limit input");
    require(
        tondo_native_runtime::tondo_rt_encoding_stream_new(
            99,
            ENCODING_OPERATION_ENCODE,
            ENCODING_POLICY_STANDARD_REQUIRED,
            1024,
            1024,
        ) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_ENCODING_INVALID_OPTIONS,
        "invalid option boundary",
    );
    clean("limits and lifecycle cleanup");
    println!(
        r#"{{"id":"limits-and-lifecycle","status":"passed","limit_kind":5,"limit_offset":0,"closed_kind":7,"zero_empty":true,"cleanup":true}}"#
    );
}

fn route_boundary() {
    tondo_native_runtime::tondo_rt_reset();
    clean("route boundary cleanup");
    println!(
        r#"{{"id":"route-boundary","status":"passed","scalar":"verified","simd":"not-measured-no-optimized-route","native_aot":"not-claimed"}}"#
    );
}

fn main() {
    base64_interoperability();
    hex_policy();
    streaming_invariance();
    strict_errors();
    limits_and_lifecycle();
    route_boundary();
    println!(r#"{{"id":"encoding-conformance","status":"passed"}}"#);
}
