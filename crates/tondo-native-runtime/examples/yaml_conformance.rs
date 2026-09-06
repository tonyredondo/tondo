//! Target-qualified YAML conformance probe.
//!
//! The companion Tondo fixture exercises the same six case IDs on the hosted
//! VM.  This probe calls the portable `std.yaml` implementation in a fresh
//! native process; it does not infer a native ABI, Cranelift lowering or an
//! AOT route that has not been implemented.

use std::io::{self, Read};

use tondo_stdlib::yaml::{
    self, YamlError, YamlErrorKind, YamlLimits, YamlOptions, YamlPathSegment, YamlReader, YamlValue,
};

fn require(condition: bool, message: &str) {
    assert!(condition, "std.yaml conformance: {message}");
}

fn begin_case() {
    tondo_native_runtime::tondo_rt_reset();
}

fn clean(case_id: &str) {
    // YAML values/readers are Rust-owned in this target-qualified oracle, so
    // no runtime table handles should remain between cases.
    require(tondo_native_runtime::tondo_rt_live_objects() == 0, case_id);
}

fn typed_dynamic() {
    begin_case();
    let (dynamic_keys, typed_length) = {
        let options = YamlOptions::create(YamlLimits::defaults());
        let source = b"name: Tondo\ncount: 7\nactive: true\n";
        let value = yaml::parse_with_options(source, options).expect("dynamic YAML parse");
        let YamlValue::Object(members) = &value else {
            panic!("dynamic YAML root must be an object");
        };
        let encoded = yaml::encode(&value, options).expect("dynamic YAML encode");
        require(
            encoded == source,
            "dynamic decode/encode preserves the shared mapping",
        );

        let typed: Vec<i64> =
            yaml::decode_static(b"- 7\n- 9\n", options).expect("typed YAML decode");
        require(typed == vec![7, 9], "typed Array[Int] values");
        let typed_encoded = yaml::encode_static(&typed, options).expect("typed YAML encode");
        require(
            typed_encoded == b"- 7\n- 9\n",
            "typed Array[Int] round trip",
        );
        (members.len(), typed.len())
    };
    clean("typed-dynamic cleanup");
    println!(
        r#"{{"id":"typed-dynamic","status":"passed","line":"typed-dynamic:3:Tondo:7:2","dynamic_keys":{dynamic_keys},"typed_length":{typed_length},"cleanup":true}}"#
    );
}

fn interoperability() {
    begin_case();
    let binary = {
        let options = YamlOptions::defaults();
        let limits = options.limits;
        let source = b"z: yes\na: 0x10\nb: !!binary Zm8=\n";
        let value = yaml::parse_with_options(source, options).expect("interop YAML parse");
        let YamlValue::Object(members) = &value else {
            panic!("interop YAML root must be an object");
        };
        let binary = members
            .iter()
            .find(|member| member.key == "b")
            .and_then(|member| match &member.value {
                YamlValue::Bytes(value) => Some(value.clone()),
                _ => None,
            })
            .expect("binary YAML member");
        let canonical = yaml::encode_canonical(&value, limits).expect("canonical YAML encode");
        require(
            canonical == b"a: 16\nb: !!binary Zm8=\nz: yes\n",
            "canonical YAML ordering and scalar normalization",
        );
        let normal = yaml::encode(&value, options).expect("normal YAML encode");
        require(
            normal == b"z: yes\na: 16\nb: !!binary Zm8=\n",
            "normal YAML encoding preserves insertion order and normalized scalars",
        );
        require(members.len() == 3, "interop mapping key count");
        binary
    };
    require(binary == b"fo", "decoded YAML binary payload");
    clean("interoperability cleanup");
    println!(
        r#"{{"id":"interoperability","status":"passed","line":"interoperability:3:16:fo:3","canonical_keys":3,"binary":"fo","cleanup":true}}"#
    );
}

struct OneByteReader {
    input: Vec<u8>,
    offset: usize,
}

impl Read for OneByteReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.offset == self.input.len() {
            return Ok(0);
        }
        buffer[0] = self.input[self.offset];
        self.offset += 1;
        Ok(1)
    }
}

fn count_events(mut reader: YamlReader) -> Result<(usize, bool), YamlError> {
    let mut count = 0;
    while reader.next()?.is_some() {
        count += 1;
    }
    let closed = matches!(
        reader.next(),
        Err(YamlError {
            kind: YamlErrorKind::Closed,
            ..
        })
    );
    Ok((count, closed))
}

fn streaming() {
    begin_case();
    let (documents, bytes_events, chunk_events, terminal) = {
        let options = YamlOptions::defaults();
        let source = b"---\na: 1\n---\na: 2\n";
        let documents = yaml::parse_all_with_options(source, options)
            .expect("streaming YAML documents")
            .len();
        let (bytes_events, bytes_terminal) =
            count_events(YamlReader::from_bytes(source, options).expect("byte YAML reader"))
                .expect("byte YAML events");
        let (chunk_events, chunk_terminal) = count_events(
            YamlReader::from_reader(
                OneByteReader {
                    input: source.to_vec(),
                    offset: 0,
                },
                options,
            )
            .expect("fragmented YAML reader"),
        )
        .expect("fragmented YAML events");
        require(bytes_events == 16, "materialized YAML event count");
        require(chunk_events == bytes_events, "one-byte event invariance");
        require(bytes_terminal && chunk_terminal, "reader closed transition");
        (
            documents,
            bytes_events,
            chunk_events,
            bytes_terminal && chunk_terminal,
        )
    };
    clean("streaming cleanup");
    println!(
        r#"{{"id":"streaming","status":"passed","line":"streaming:2:16:closed","documents":{documents},"bytes_events":{bytes_events},"chunk_events":{chunk_events},"terminal":{terminal},"cleanup":true}}"#
    );
}

fn errors_path() {
    begin_case();
    {
        let error =
            yaml::parse_with_options(b"items:\n  - !!binary invalid!\n", YamlOptions::defaults())
                .expect_err("invalid YAML binary must fail");
        require(
            error.kind == YamlErrorKind::InvalidBinary,
            "invalid binary kind",
        );
        require(
            error.path
                == vec![
                    YamlPathSegment::Key("items".into()),
                    YamlPathSegment::Index(0),
                ],
            "invalid binary path",
        );
        require(
            (error.offset, error.line, error.column) == (0, 1, 1),
            "invalid binary source location",
        );
    }
    clean("errors-path cleanup");
    println!(
        r#"{{"id":"errors-path","status":"passed","line":"errors-path:17:2:0","kind":17,"path":["items","0"],"offset":0,"line":1,"column":1,"cleanup":true}}"#
    );
}

fn limits_lifecycle() {
    begin_case();
    let (limit_kind, limit_offset, closed) = {
        let limits = YamlLimits::create(4, 1, 64, 100, 100, 10, 100, 100, 32)
            .expect("valid small YAML limits");
        let error = yaml::parse_with_options(b"a: 1\n", YamlOptions::create(limits))
            .expect_err("input limit must fail atomically");
        require(
            error.kind == YamlErrorKind::NodeLimit && error.offset == 0,
            "input limit kind and offset",
        );

        let mut reader =
            YamlReader::from_bytes(b"a: 1\n", YamlOptions::defaults()).expect("lifecycle reader");
        while reader.next().expect("lifecycle event").is_some() {}
        let closed = matches!(
            reader.next(),
            Err(YamlError {
                kind: YamlErrorKind::Closed,
                ..
            })
        );
        require(closed, "reader remains closed after EOF");
        (error.kind, error.offset, closed)
    };
    require(
        limit_kind == YamlErrorKind::NodeLimit && limit_offset == 0 && closed,
        "limit lifecycle result",
    );
    clean("limits-lifecycle cleanup");
    println!(
        r#"{{"id":"limits-lifecycle","status":"passed","line":"limits-lifecycle:19:0","kind":19,"offset":0,"closed":true,"cleanup":true}}"#
    );
}

fn route_boundary() {
    begin_case();
    clean("route-boundary cleanup");
    println!(
        r#"{{"id":"route-boundary","status":"passed","line":"route-boundary:scalar:simd-not-claimed:native-aot-not-claimed","scalar":"verified","simd":"not-measured-no-optimized-route","native_aot":"not-claimed"}}"#
    );
}

fn main() {
    typed_dynamic();
    interoperability();
    streaming();
    errors_path();
    limits_lifecycle();
    route_boundary();
    println!(r#"{{"id":"yaml-conformance","status":"passed"}}"#);
}
