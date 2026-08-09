#![no_main]

use libfuzzer_sys::fuzz_target;
use tondo_stdlib::{json, messagepack, protobuf};

// Differential fuzz boundary for the three portable codec owners. The target
// is deliberately an oracle-free panic/termination probe: the interoperability
// oracle lives in the deterministic integration test, while this target drives
// arbitrary bytes through every bounded parser and its one-byte-fragment path.
fuzz_target!(|input: &[u8]| {
    let input = &input[..input.len().min(64 * 1024)];
    let _ = json::validate(input);
    let _ = messagepack::validate(input, Default::default());
    let _ = protobuf::validate::<()>(input, Default::default());

    let json_chunks = input.chunks(1);
    if let Ok(mut reader) = json::JsonReader::from_chunks(json_chunks, Default::default()) {
        let _ = reader.finish();
    }
    let messagepack_chunks = input.chunks(1);
    if let Ok(mut reader) =
        messagepack::MessagePackReader::from_chunks(messagepack_chunks, Default::default())
    {
        let _ = reader.finish();
    }
    let protobuf_chunks = input.chunks(1);
    if let Ok(mut reader) =
        protobuf::ProtoReader::<()>::from_chunks(protobuf_chunks, Default::default())
    {
        let _ = reader.finish();
    }
});
