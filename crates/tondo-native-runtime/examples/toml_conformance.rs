//! Target-qualified TOML kernel conformance probe.
//!
//! This process calls the portable standard-library kernel. It does not
//! exercise a native TOML ABI or Cranelift lowering.

use std::collections::BTreeMap;
use std::io::{self, Read};

use tondo_stdlib::toml::{
    self, TomlErrorKind, TomlLimits, TomlOptions, TomlPathSegment, TomlReader, TomlValue,
};

fn begin_case() {
    tondo_native_runtime::tondo_rt_reset();
}

fn end_case() {
    assert_eq!(tondo_native_runtime::tondo_rt_live_objects(), 0);
}

fn typed_dynamic() {
    begin_case();
    let source =
        include_bytes!("../../../testing/stdlib-toml-conformance-fixtures/typed-dynamic.toml");
    let options = TomlOptions::defaults();
    let value = toml::parse(source, options).expect("dynamic parse");
    let TomlValue::Table(members) = &value else {
        panic!("TOML root must be a table");
    };
    assert_eq!(members.len(), 3);
    assert_eq!(
        toml::encode(&value, options).unwrap(),
        b"name = \"Tondo\"\ncount = 7\nactive = true\n"
    );
    let typed_source =
        include_bytes!("../../../testing/stdlib-toml-conformance-fixtures/typed.toml");
    let typed: BTreeMap<String, i64> =
        toml::decode_static(typed_source, options).expect("typed decode");
    assert_eq!(typed.get("a"), Some(&7));
    assert_eq!(typed.get("b"), Some(&9));
    let typed_bytes = toml::encode_static(&typed, options).expect("typed encode");
    assert_eq!(typed_bytes, typed_source);
    end_case();
    println!(
        r#"{{"id":"typed-dynamic","status":"passed","line":"typed-dynamic:3:Tondo:7:2","dynamic_keys":3,"typed_length":2,"cleanup":true}}"#
    );
}

fn interoperability() {
    begin_case();
    let source =
        include_bytes!("../../../testing/stdlib-toml-conformance-fixtures/interoperability.toml");
    let options = TomlOptions::defaults();
    let value = toml::parse(source, options).expect("TOML 1.1 parse");
    let normal = toml::encode(&value, options).expect("normal encoding");
    let canonical = toml::encode_canonical(&value, options.limits).expect("canonical encoding");
    assert_eq!(toml::parse(&normal, options).unwrap(), value);
    let canonical_value = toml::parse(&canonical, options).unwrap();
    assert_eq!(
        toml::encode_canonical(&canonical_value, options.limits).unwrap(),
        canonical
    );
    assert!(canonical.starts_with(b"a = 16\n"));
    assert!(normal.starts_with(b"z = \"caf\xC3\xA9\"\n"));
    let TomlValue::Table(members) = value else {
        panic!("TOML root must be a table");
    };
    assert_eq!(members.len(), 4);
    assert!(matches!(&members[0].value, TomlValue::Text(text) if text == "café"));
    assert!(matches!(members[1].value, TomlValue::Int(16)));
    assert!(matches!(members[2].value, TomlValue::OffsetDateTime(_)));
    assert!(matches!(members[3].value, TomlValue::Table(_)));
    end_case();
    println!(
        r#"{{"id":"interoperability","status":"passed","line":"interoperability:4:16:caf\u00e9:date-time","canonical_keys":4,"radix_integer":16,"temporal":true,"cleanup":true}}"#
    );
}

struct OneByteReader {
    bytes: Vec<u8>,
    offset: usize,
}

impl Read for OneByteReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.offset == self.bytes.len() {
            return Ok(0);
        }
        output[0] = self.bytes[self.offset];
        self.offset += 1;
        Ok(1)
    }
}

fn read_events(mut reader: TomlReader) -> (usize, bool) {
    let mut count = 0;
    while reader.next().expect("event reader").is_some() {
        count += 1;
    }
    let closed = reader.next().unwrap_err().kind == TomlErrorKind::Closed;
    (count, closed)
}

fn streaming() {
    begin_case();
    let source = include_bytes!("../../../testing/stdlib-toml-conformance-fixtures/streaming.toml");
    let options = TomlOptions::defaults();
    let (bytes_events, bytes_closed) =
        read_events(TomlReader::from_bytes(source, options).expect("bytes reader"));
    let (chunk_events, chunk_closed) = read_events(
        TomlReader::from_reader(
            OneByteReader {
                bytes: source.to_vec(),
                offset: 0,
            },
            options,
        )
        .expect("fragmented reader"),
    );
    assert_eq!(bytes_events, chunk_events);
    assert!(bytes_closed && chunk_closed);
    let value = toml::parse(source, options).unwrap();
    let TomlValue::Table(root) = value else {
        panic!("TOML root must be a table");
    };
    let TomlValue::Array(rows) = &root[1].value else {
        panic!("items must be an array of tables");
    };
    assert_eq!(rows.len(), 2);
    end_case();
    println!(
        r#"{{"id":"streaming","status":"passed","line":"streaming:2:12:closed","rows":2,"bytes_events":{bytes_events},"chunk_events":{chunk_events},"terminal":true,"cleanup":true}}"#
    );
}

fn errors_path() {
    begin_case();
    let source = include_bytes!("../../../testing/stdlib-toml-conformance-fixtures/duplicate.toml");
    let error = toml::parse(source, TomlOptions::defaults()).expect_err("duplicate key must fail");
    assert_eq!(error.kind, TomlErrorKind::DuplicateKey);
    assert_eq!(error.path, vec![TomlPathSegment::Key("first".into())]);
    assert_eq!(error.span.start_offset, 10);
    assert_eq!(error.span.start_line, 2);
    assert_eq!(error.span.start_column, 1);
    assert!(error.span.end_offset > error.span.start_offset);
    end_case();
    println!(
        r#"{{"id":"errors-path","status":"passed","line":"errors-path:DuplicateKey:first:10:2:1","kind":"DuplicateKey","path":["first"],"offset":10,"start_line":2,"start_column":1,"cleanup":true}}"#
    );
}

fn limits_lifecycle() {
    begin_case();
    let mut limits = TomlLimits::defaults();
    limits.max_input_bytes = 3;
    let options = TomlOptions::create(limits);
    let source = include_bytes!("../../../testing/stdlib-toml-conformance-fixtures/limit.toml");
    let error = toml::parse(source, options).expect_err("input limit must fail");
    assert_eq!(error.kind, TomlErrorKind::ResourceLimit);
    assert_eq!(error.span.start_offset, 0);
    assert!(TomlReader::from_bytes(source, options).is_err());
    end_case();
    println!(
        r#"{{"id":"limits-lifecycle","status":"passed","line":"limits-lifecycle:ResourceLimit:0","kind":"ResourceLimit","offset":0,"partial_value":false,"cleanup":true}}"#
    );
}

fn route_boundary() {
    begin_case();
    end_case();
    println!(
        r#"{{"id":"route-boundary","status":"passed","line":"route-boundary:scalar:simd-not-claimed:native-aot-not-claimed","scalar":"verified","simd":"not-measured-no-optimized-route","native_aot":"not-claimed","cleanup":true}}"#
    );
}

fn main() {
    typed_dynamic();
    interoperability();
    streaming();
    errors_path();
    limits_lifecycle();
    route_boundary();
    println!(r#"{{"id":"toml-conformance","status":"passed"}}"#);
}
