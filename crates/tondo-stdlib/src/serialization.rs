//! Portable event protocol shared by typed serializers and dynamic codecs.

use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Null,
    Bool(bool),
    Int(i128),
    UInt(u128),
    Float(f64),
    Float32(u32),
    Float64(u64),
    String(String),
    Bytes(Vec<u8>),
    StartArray(Option<usize>),
    EndArray,
    StartMap(Option<usize>),
    MapKey,
    EndMap,
    StartRecord { name: String, fields: Option<usize> },
    Field(String),
    EndRecord,
    StartEnum { name: String, variant: String },
    EndEnum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_depth: usize,
    pub max_events: usize,
    pub max_bytes: usize,
    pub max_container_items: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 256,
            max_events: 1_000_000,
            max_bytes: 64 * 1024 * 1024,
            max_container_items: 1_048_576,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerializationError {
    UnexpectedEvent,
    TypeMismatch,
    LimitExceeded,
    UnbalancedContainer,
    DuplicateField,
    InvalidContainerLength,
}

/// Validate a complete event stream without materialising a document tree.
pub fn validate_events(events: &[Event], limits: Limits) -> Result<(), SerializationError> {
    if events.len() > limits.max_events {
        return Err(SerializationError::LimitExceeded);
    }
    let mut bytes = 0usize;
    let mut root_seen = false;
    #[derive(Clone, Copy)]
    enum FrameKind {
        Array,
        Map {
            expect_key_marker: bool,
            expect_key_value: bool,
        },
        Record {
            expect_field: bool,
        },
        Enum {
            payload_seen: bool,
        },
    }
    struct Frame {
        kind: FrameKind,
        declared_items: Option<usize>,
        items: usize,
        fields: BTreeSet<String>,
    }
    let mut stack = Vec::<Frame>::new();

    fn consume_value(
        stack: &mut [Frame],
        root_seen: &mut bool,
        limits: Limits,
    ) -> Result<(), SerializationError> {
        match stack.last_mut() {
            Some(frame) => match &mut frame.kind {
                FrameKind::Array => {
                    frame.items = frame
                        .items
                        .checked_add(1)
                        .ok_or(SerializationError::LimitExceeded)?;
                    if frame.items > limits.max_container_items
                        || frame
                            .declared_items
                            .is_some_and(|length| frame.items > length)
                    {
                        return Err(SerializationError::LimitExceeded);
                    }
                    Ok(())
                }
                FrameKind::Map {
                    expect_key_marker,
                    expect_key_value,
                } => {
                    if *expect_key_value {
                        *expect_key_value = false;
                        *expect_key_marker = false;
                        return Ok(());
                    }
                    if *expect_key_marker {
                        return Err(SerializationError::UnexpectedEvent);
                    }
                    frame.items = frame
                        .items
                        .checked_add(1)
                        .ok_or(SerializationError::LimitExceeded)?;
                    if frame.items > limits.max_container_items
                        || frame
                            .declared_items
                            .is_some_and(|length| frame.items > length)
                    {
                        return Err(SerializationError::LimitExceeded);
                    }
                    *expect_key_marker = true;
                    Ok(())
                }
                FrameKind::Record { expect_field } => {
                    if *expect_field {
                        return Err(SerializationError::UnexpectedEvent);
                    }
                    frame.items = frame
                        .items
                        .checked_add(1)
                        .ok_or(SerializationError::LimitExceeded)?;
                    if frame.items > limits.max_container_items
                        || frame
                            .declared_items
                            .is_some_and(|length| frame.items > length)
                    {
                        return Err(SerializationError::LimitExceeded);
                    }
                    *expect_field = true;
                    Ok(())
                }
                FrameKind::Enum { payload_seen } => {
                    if *payload_seen {
                        return Err(SerializationError::UnexpectedEvent);
                    }
                    *payload_seen = true;
                    Ok(())
                }
            },
            None => {
                if *root_seen {
                    return Err(SerializationError::UnexpectedEvent);
                }
                *root_seen = true;
                Ok(())
            }
        }
    }

    fn push_frame(
        stack: &mut Vec<Frame>,
        kind: FrameKind,
        declared_items: Option<usize>,
        limits: Limits,
    ) -> Result<(), SerializationError> {
        if stack.len() >= limits.max_depth {
            return Err(SerializationError::LimitExceeded);
        }
        if declared_items.is_some_and(|length| length > limits.max_container_items) {
            return Err(SerializationError::LimitExceeded);
        }
        stack.push(Frame {
            kind,
            declared_items,
            items: 0,
            fields: BTreeSet::new(),
        });
        Ok(())
    }

    fn finish_frame(frame: Frame, expected: FrameKind) -> Result<(), SerializationError> {
        let matches = match (frame.kind, expected) {
            (FrameKind::Array, FrameKind::Array) => true,
            (
                FrameKind::Map {
                    expect_key_marker,
                    expect_key_value,
                },
                FrameKind::Map { .. },
            ) => expect_key_marker && !expect_key_value,
            (FrameKind::Record { expect_field }, FrameKind::Record { .. }) => expect_field,
            (FrameKind::Enum { .. }, FrameKind::Enum { .. }) => true,
            _ => false,
        };
        if !matches {
            return Err(SerializationError::UnbalancedContainer);
        }
        if frame
            .declared_items
            .is_some_and(|length| frame.items != length)
        {
            return Err(SerializationError::InvalidContainerLength);
        }
        Ok(())
    }

    fn add_bytes(
        bytes: &mut usize,
        value: usize,
        limits: Limits,
    ) -> Result<(), SerializationError> {
        *bytes = bytes
            .checked_add(value)
            .ok_or(SerializationError::LimitExceeded)?;
        if *bytes > limits.max_bytes {
            return Err(SerializationError::LimitExceeded);
        }
        Ok(())
    }

    for event in events {
        match event {
            Event::MapKey => {
                let Some(frame) = stack.last_mut() else {
                    return Err(SerializationError::UnexpectedEvent);
                };
                let FrameKind::Map {
                    expect_key_marker,
                    expect_key_value,
                } = &mut frame.kind
                else {
                    return Err(SerializationError::UnexpectedEvent);
                };
                if !*expect_key_marker || *expect_key_value {
                    return Err(SerializationError::UnexpectedEvent);
                }
                *expect_key_marker = false;
                *expect_key_value = true;
            }
            Event::Field(value) => {
                let Some(frame) = stack.last_mut() else {
                    return Err(SerializationError::UnexpectedEvent);
                };
                let FrameKind::Record { expect_field } = &mut frame.kind else {
                    return Err(SerializationError::UnexpectedEvent);
                };
                if !*expect_field || !frame.fields.insert(value.clone()) {
                    return Err(SerializationError::DuplicateField);
                }
                add_bytes(&mut bytes, value.len(), limits)?;
                *expect_field = false;
            }
            Event::String(value) => {
                consume_value(&mut stack, &mut root_seen, limits)?;
                add_bytes(&mut bytes, value.len(), limits)?;
            }
            Event::Bytes(value) => {
                consume_value(&mut stack, &mut root_seen, limits)?;
                add_bytes(&mut bytes, value.len(), limits)?;
            }
            Event::StartArray(length) => {
                consume_value(&mut stack, &mut root_seen, limits)?;
                push_frame(&mut stack, FrameKind::Array, *length, limits)?;
            }
            Event::StartMap(length) => {
                consume_value(&mut stack, &mut root_seen, limits)?;
                push_frame(
                    &mut stack,
                    FrameKind::Map {
                        expect_key_marker: true,
                        expect_key_value: false,
                    },
                    *length,
                    limits,
                )?;
            }
            Event::EndArray => {
                let frame = stack.pop().ok_or(SerializationError::UnbalancedContainer)?;
                finish_frame(frame, FrameKind::Array)?;
            }
            Event::EndMap => {
                let frame = stack.pop().ok_or(SerializationError::UnbalancedContainer)?;
                finish_frame(
                    frame,
                    FrameKind::Map {
                        expect_key_marker: true,
                        expect_key_value: false,
                    },
                )?;
            }
            Event::StartRecord { name, fields } => {
                consume_value(&mut stack, &mut root_seen, limits)?;
                add_bytes(&mut bytes, name.len(), limits)?;
                push_frame(
                    &mut stack,
                    FrameKind::Record { expect_field: true },
                    *fields,
                    limits,
                )?;
            }
            Event::EndRecord => {
                let frame = stack.pop().ok_or(SerializationError::UnbalancedContainer)?;
                finish_frame(frame, FrameKind::Record { expect_field: true })?;
            }
            Event::StartEnum { name, variant } => {
                consume_value(&mut stack, &mut root_seen, limits)?;
                add_bytes(&mut bytes, name.len(), limits)?;
                add_bytes(&mut bytes, variant.len(), limits)?;
                push_frame(
                    &mut stack,
                    FrameKind::Enum {
                        payload_seen: false,
                    },
                    None,
                    limits,
                )?;
            }
            Event::EndEnum => {
                let frame = stack.pop().ok_or(SerializationError::UnbalancedContainer)?;
                finish_frame(
                    frame,
                    FrameKind::Enum {
                        payload_seen: false,
                    },
                )?;
            }
            Event::Null
            | Event::Bool(_)
            | Event::Int(_)
            | Event::UInt(_)
            | Event::Float(_)
            | Event::Float32(_)
            | Event::Float64(_) => {
                consume_value(&mut stack, &mut root_seen, limits)?;
            }
        }
    }
    if !stack.is_empty() || !root_seen {
        return Err(SerializationError::UnbalancedContainer);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_nested_arrays_and_maps() {
        let events = [
            Event::StartMap(Some(2)),
            Event::MapKey,
            Event::String("items".into()),
            Event::StartArray(Some(2)),
            Event::Int(1),
            Event::Int(2),
            Event::EndArray,
            Event::MapKey,
            Event::String("done".into()),
            Event::Bool(true),
            Event::EndMap,
        ];
        validate_events(&events, Limits::default()).unwrap();
    }

    #[test]
    fn rejects_unbalanced_and_key_order() {
        assert_eq!(
            validate_events(&[Event::EndArray], Limits::default()),
            Err(SerializationError::UnbalancedContainer)
        );
        assert_eq!(
            validate_events(&[Event::Field("bad".into())], Limits::default()),
            Err(SerializationError::UnexpectedEvent)
        );
    }

    #[test]
    fn enforces_depth_events_and_bytes() {
        let events = [Event::StartArray(None), Event::Int(1), Event::EndArray];
        assert_eq!(
            validate_events(
                &events,
                Limits {
                    max_depth: 0,
                    ..Limits::default()
                }
            ),
            Err(SerializationError::LimitExceeded)
        );
        assert_eq!(
            validate_events(
                &events,
                Limits {
                    max_events: 2,
                    ..Limits::default()
                }
            ),
            Err(SerializationError::LimitExceeded)
        );
        assert_eq!(
            validate_events(
                &[Event::String("too long".into())],
                Limits {
                    max_bytes: 2,
                    ..Limits::default()
                }
            ),
            Err(SerializationError::LimitExceeded)
        );
    }

    #[test]
    fn validates_records_enums_and_arbitrary_map_keys() {
        let events = [
            Event::StartRecord {
                name: "User".into(),
                fields: Some(2),
            },
            Event::Field("id".into()),
            Event::UInt(7),
            Event::Field("role".into()),
            Event::StartEnum {
                name: "Role".into(),
                variant: "Admin".into(),
            },
            Event::EndEnum,
            Event::EndRecord,
        ];
        validate_events(&events, Limits::default()).unwrap();

        let map = [
            Event::StartMap(Some(1)),
            Event::MapKey,
            Event::Int(1),
            Event::String("value".into()),
            Event::EndMap,
        ];
        validate_events(&map, Limits::default()).unwrap();
    }

    #[test]
    fn rejects_duplicate_fields_bad_lengths_and_incomplete_map_keys() {
        let duplicate = [
            Event::StartRecord {
                name: "User".into(),
                fields: None,
            },
            Event::Field("id".into()),
            Event::Int(1),
            Event::Field("id".into()),
        ];
        assert_eq!(
            validate_events(&duplicate, Limits::default()),
            Err(SerializationError::DuplicateField)
        );

        let wrong_length = [Event::StartArray(Some(2)), Event::Int(1), Event::EndArray];
        assert_eq!(
            validate_events(&wrong_length, Limits::default()),
            Err(SerializationError::InvalidContainerLength)
        );

        let incomplete_map = [Event::StartMap(Some(1)), Event::MapKey, Event::EndMap];
        assert_eq!(
            validate_events(&incomplete_map, Limits::default()),
            Err(SerializationError::UnbalancedContainer)
        );
    }
}
