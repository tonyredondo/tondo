use tondo_reliability::log_fuzz::{convert, fields, level, observed, to_limits as limits};
use tondo_reliability::log_model::values::{
    self as reference, Event, Fields, Format, Limits, Value,
};
use tondo_stdlib::log as kernel;

fn fixture_value(description: &serde_json::Value) -> Value {
    let value = &description["value"];
    match description["kind"].as_str().unwrap() {
        "Null" => Value::Null,
        "Redacted" => Value::Redacted,
        "Bool" => Value::Bool(value.as_bool().unwrap()),
        "Int" => Value::Int(value.as_str().unwrap().parse().unwrap()),
        "UInt" => Value::UInt(value.as_str().unwrap().parse().unwrap()),
        "Float" => Value::Float(value.as_str().unwrap().parse().unwrap()),
        "Text" => Value::Text(value.as_str().unwrap().to_owned()),
        "Bytes" => Value::Bytes(
            value
                .as_str()
                .unwrap()
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect(),
        ),
        "Array" => Value::Array(
            value
                .as_array()
                .unwrap()
                .iter()
                .map(fixture_value)
                .collect(),
        ),
        "Object" => Value::Object(fixture_fields(value)),
        kind => panic!("unknown fixture value: {kind}"),
    }
}
fn fixture_fields(description: &serde_json::Value) -> Fields {
    let mut fields = Fields::default();
    for pair in description.as_array().unwrap() {
        fields
            .put(pair[0].as_str().unwrap(), fixture_value(&pair[1]))
            .unwrap();
    }
    fields
}
fn fixture_events(
    description: &serde_json::Value,
) -> (
    Result<Event, reference::Error>,
    Result<kernel::LogEvent, kernel::LogError>,
) {
    let level = match description["level"].as_str().unwrap() {
        "Trace" => 0,
        "Debug" => 1,
        "Info" => 2,
        "Warn" => 3,
        "Error" => 4,
        other => panic!("unknown level: {other}"),
    };
    let fields = fixture_fields(&description["fields"]);
    let target = description["target"].as_str().unwrap();
    let message = description["message"].as_str().unwrap();
    let time = description["timestamp"].as_str();
    let actual = kernel::LogEvent::create(
        self::level(level),
        target.into(),
        message.into(),
        self::fields(&fields),
        time.map(|value| tondo_stdlib::civil_time::UtcDateTime::parse(value).unwrap()),
    );
    let reference = Event::create(level, target, message, fields, time);
    (reference, actual)
}
#[test]
fn authored_corpus_agrees_with_reference_and_production_values_and_errors() {
    use serde_json::Value as Json;
    let corpus: Json = serde_json::from_str(include_str!("fixtures/log-cases.json")).unwrap();
    let rows = corpus["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 34);
    let mut names = std::collections::BTreeSet::new();
    for row in rows {
        let name = row["id"].as_str().unwrap();
        assert!(names.insert(name));
        match row["operation"].as_str().unwrap() {
            "field" => {
                let mut reference = fixture_fields(&row["existing"]);
                let mut actual = fields(&reference);
                let before = actual.clone();
                let reference_before = reference.clone();
                let candidate = fixture_value(&row["value"]);
                let key = row["key"].as_str().unwrap();
                let expected = Err(row["error"].as_str().unwrap().to_owned());
                assert_eq!(
                    observed(reference.put(key, candidate.clone())),
                    expected,
                    "reference {name}"
                );
                assert_eq!(
                    observed(actual.put(key.to_owned(), convert(&candidate))),
                    expected,
                    "kernel {name}"
                );
                assert_eq!(actual, before);
                assert_eq!(reference, reference_before);
            }
            "event" => {
                let (reference, actual) = fixture_events(&row["event"]);
                let expected = Err(row["error"].as_str().unwrap().to_owned());
                assert_eq!(
                    observed(reference.map(|_| ())),
                    expected,
                    "reference {name}"
                );
                assert_eq!(observed(actual.map(|_| ())), expected, "kernel {name}");
            }
            "format" => {
                let (reference, actual) = fixture_events(&row["event"]);
                let reference = reference.unwrap();
                let actual = actual.unwrap();
                let format = if row["format"] == "Text" {
                    Format::Text
                } else {
                    Format::JsonLines
                };
                let actual_format = if format == Format::Text {
                    kernel::LogFormat::Text
                } else {
                    kernel::LogFormat::JsonLines
                };
                let mut limits = Limits::default();
                if let Some(values) = row["limits"].as_object() {
                    for (key, value) in values {
                        let value = value.as_u64().unwrap() as usize;
                        match key.as_str() {
                            "event" => limits.event = value,
                            "fields" => limits.fields = value,
                            "depth" => limits.depth = value,
                            "key" => limits.key = value,
                            "string" => limits.string = value,
                            "queue" => limits.queue = value,
                            other => panic!("unknown limit: {other}"),
                        }
                    }
                }
                let expected = if let Some(error) = row["error"].as_str() {
                    Err(error.to_owned())
                } else {
                    Ok(row["record"].as_str().unwrap().as_bytes().to_vec())
                };
                assert_eq!(
                    observed(reference::format_record(&reference, format, limits)),
                    expected,
                    "reference {name}"
                );
                assert_eq!(
                    observed(kernel::format_record(
                        &actual,
                        actual_format,
                        self::limits(limits)
                    )),
                    expected,
                    "kernel {name}"
                );
            }
            other => panic!("unknown operation: {other}"),
        }
    }
}
