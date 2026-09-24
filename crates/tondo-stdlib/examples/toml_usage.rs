//! Executable usage guide for the current Rust std.toml kernel.
//! This is not a Tondo source-level std.toml API example.

use std::collections::BTreeMap;

use tondo_stdlib::toml::{
    self, TomlError, TomlErrorKind, TomlEvent, TomlLimits, TomlOptions, TomlPathSegment,
    TomlReader, TomlScalar, TomlValue, TomlWriter,
};

fn materialized_and_typed() -> Result<(), TomlError> {
    let source = b"name = 'cafe'\ncount = 7\nwhen = 1979-05-27T07:32:00Z\n";
    let options = TomlOptions::defaults();
    let value = toml::parse(source, options)?;
    assert!(matches!(&value, TomlValue::Table(members) if members.len() == 3));
    let canonical = toml::encode_canonical(&value, options.limits)?;
    assert_eq!(
        toml::encode_canonical(&toml::parse(&canonical, options)?, options.limits)?,
        canonical
    );
    assert!(canonical.starts_with(b"count = 7\n"));

    let typed = BTreeMap::from([("count".to_owned(), 7_i64)]);
    let encoded = toml::encode_static(&typed, options)?;
    assert_eq!(
        toml::decode_static::<BTreeMap<String, i64>>(&encoded, options)?,
        typed
    );
    Ok(())
}

fn borrowed_view_and_costs() -> Result<(), TomlError> {
    let source = b"title = 'Tondo'\n";
    let options = TomlOptions::defaults();
    let view = toml::parse_view(source, options)?;
    assert_eq!(view.bytes(), source);
    assert_eq!(view.clone_value()?, toml::parse(source, options)?);
    // parse_view validates by building and dropping a full tree; clone_value reparses.
    Ok(())
}

fn buffered_events_and_lifecycle() -> Result<(), TomlError> {
    let options = TomlOptions::defaults();
    let source = b"a = 2\n";
    let mut reader = TomlReader::from_chunks(source.iter().map(std::slice::from_ref), options)?;
    let mut events = Vec::new();
    loop {
        let event = reader
            .next()?
            .expect("StreamEnd must close the event sequence");
        let ended = event == TomlEvent::StreamEnd;
        events.push(event);
        if ended {
            break;
        }
    }
    reader.finish()?;
    assert_eq!(reader.next().unwrap_err().kind, TomlErrorKind::Closed);
    assert_eq!(
        events,
        [
            TomlEvent::StreamStart,
            TomlEvent::Key(vec!["a".to_owned()]),
            TomlEvent::Scalar(TomlScalar::Int(2)),
            TomlEvent::StreamEnd,
        ]
    );

    let mut writer = TomlWriter::to_writer(options)?;
    for event in events {
        writer.write(event)?;
    }
    assert_eq!(writer.finish()?, source);
    assert_eq!(writer.finish().unwrap_err().kind, TomlErrorKind::Closed);

    let mut incomplete = TomlWriter::to_writer(options)?;
    incomplete.write(TomlEvent::StreamStart)?;
    assert_eq!(
        incomplete.finish().unwrap_err().kind,
        TomlErrorKind::UnexpectedToken
    );
    Ok(())
}

fn errors_and_limits() -> Result<(), TomlError> {
    let options = TomlOptions::defaults();
    let duplicate = toml::parse(b"first = 1\nfirst = 2\n", options).unwrap_err();
    assert_eq!(duplicate.kind, TomlErrorKind::DuplicateKey);
    assert_eq!(duplicate.path, [TomlPathSegment::Key("first".to_owned())]);
    assert_eq!(
        (duplicate.span.start_offset, duplicate.span.start_line),
        (10, 2)
    );

    let bounded = TomlOptions::create(TomlLimits {
        max_input_bytes: 4,
        ..TomlLimits::defaults()
    });
    assert_eq!(
        toml::parse(b"first = 1\n", bounded).unwrap_err().kind,
        TomlErrorKind::ResourceLimit
    );
    assert_eq!(
        TomlReader::from_chunks([b"first = 1\n"], bounded)
            .unwrap_err()
            .kind,
        TomlErrorKind::ResourceLimit
    );
    assert_eq!(
        toml::encode(&TomlValue::Null, options).unwrap_err().kind,
        TomlErrorKind::TypeMismatch
    );
    Ok(())
}

fn main() -> Result<(), TomlError> {
    materialized_and_typed()?;
    borrowed_view_and_costs()?;
    buffered_events_and_lifecycle()?;
    errors_and_limits()?;
    println!("toml-doc-ok");
    Ok(())
}
