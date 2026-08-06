use std::hint::black_box;
use std::time::Instant;

use tondo_stdlib::{json, math, messagepack, protobuf, testing};

const SAMPLES: usize = 9;
const BATCH: usize = 256;

fn sample(name: &str, mut operation: impl FnMut()) {
    for _ in 0..SAMPLES {
        let started = Instant::now();
        for _ in 0..BATCH {
            operation();
        }
        let nanos = started.elapsed().as_nanos() / BATCH as u128;
        println!("{name}\t{nanos}");
    }
}

fn main() {
    let json_input = br#"{"z":[1,2,3],"a":{"n":true}}"#;
    let messagepack_input = messagepack::encode(&messagepack::Value::Map(vec![
        messagepack::MessagePackEntry {
            key: messagepack::Value::String("b".into()),
            value: messagepack::Value::UInt(2),
        },
        messagepack::MessagePackEntry {
            key: messagepack::Value::String("a".into()),
            value: messagepack::Value::UInt(1),
        },
    ]));
    let protobuf_input = [0x08, 0x01, 0x12, 0x03, b'o', b'k', b'!'];

    sample("json", || {
        let value = json::parse(black_box(json_input)).expect("valid JSON");
        black_box(json::encode_canonical(&value).expect("canonical JSON"));
    });
    sample("messagepack", || {
        let value = messagepack::decode(black_box(&messagepack_input)).expect("valid MessagePack");
        black_box(messagepack::encode_deterministic(&value).expect("deterministic MessagePack"));
    });
    sample("protobuf", || {
        black_box(
            protobuf::decode_message(
                black_box(&protobuf_input),
                protobuf::ProtoDecodeOptions::default(),
            )
            .expect("valid protobuf"),
        );
    });
    sample("testing", || {
        let mut generator = testing::Generator::new(7);
        black_box(generator.next_text(32).expect("bounded text"));
        black_box(testing::diff_text("old\n", "new\n"));
    });
    sample("math", || {
        black_box(math::fma(1.25, 2.0, 0.5));
    });
}
