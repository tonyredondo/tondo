//! Portable event protocol shared by typed serializers and dynamic codecs.

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    StartArray(Option<usize>),
    EndArray,
    StartMap(Option<usize>),
    Key(String),
    EndMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_depth: usize,
    pub max_events: usize,
    pub max_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 256,
            max_events: 1_000_000,
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerializationError {
    UnexpectedEvent,
    TypeMismatch,
    LimitExceeded,
    UnbalancedContainer,
}

/// Validate a complete event stream without materialising a document tree.
pub fn validate_events(events: &[Event], limits: Limits) -> Result<(), SerializationError> {
    if events.len() > limits.max_events {
        return Err(SerializationError::LimitExceeded);
    }
    let mut bytes = 0usize;
    let mut root_seen = false;
    #[derive(Clone, Copy)]
    enum Frame {
        Array,
        Map { expect_key: bool },
    }
    let mut stack = Vec::<Frame>::new();

    fn consume_value(stack: &mut [Frame], root_seen: &mut bool) -> Result<(), SerializationError> {
        match stack.last_mut() {
            Some(Frame::Array) => Ok(()),
            Some(Frame::Map { expect_key }) => {
                if *expect_key {
                    return Err(SerializationError::UnexpectedEvent);
                }
                *expect_key = true;
                Ok(())
            }
            None => {
                if *root_seen {
                    Err(SerializationError::UnexpectedEvent)
                } else {
                    *root_seen = true;
                    Ok(())
                }
            }
        }
    }

    for event in events {
        match event {
            Event::Key(value) => {
                let Some(Frame::Map { expect_key }) = stack.last_mut() else {
                    return Err(SerializationError::UnexpectedEvent);
                };
                if !*expect_key {
                    return Err(SerializationError::UnexpectedEvent);
                }
                bytes = bytes
                    .checked_add(value.len())
                    .ok_or(SerializationError::LimitExceeded)?;
                if bytes > limits.max_bytes {
                    return Err(SerializationError::LimitExceeded);
                }
                *expect_key = false;
            }
            Event::String(value) => {
                consume_value(&mut stack, &mut root_seen)?;
                bytes = bytes
                    .checked_add(value.len())
                    .ok_or(SerializationError::LimitExceeded)?;
                if bytes > limits.max_bytes {
                    return Err(SerializationError::LimitExceeded);
                }
            }
            Event::Bytes(value) => {
                consume_value(&mut stack, &mut root_seen)?;
                bytes = bytes
                    .checked_add(value.len())
                    .ok_or(SerializationError::LimitExceeded)?;
                if bytes > limits.max_bytes {
                    return Err(SerializationError::LimitExceeded);
                }
            }
            Event::StartArray(_) => {
                consume_value(&mut stack, &mut root_seen)?;
                if stack.len() >= limits.max_depth {
                    return Err(SerializationError::LimitExceeded);
                }
                stack.push(Frame::Array);
            }
            Event::StartMap(_) => {
                consume_value(&mut stack, &mut root_seen)?;
                if stack.len() >= limits.max_depth {
                    return Err(SerializationError::LimitExceeded);
                }
                stack.push(Frame::Map { expect_key: true });
            }
            Event::EndArray => {
                if !matches!(stack.pop(), Some(Frame::Array)) {
                    return Err(SerializationError::UnbalancedContainer);
                }
            }
            Event::EndMap => {
                if !matches!(stack.pop(), Some(Frame::Map { expect_key: true })) {
                    return Err(SerializationError::UnbalancedContainer);
                }
            }
            Event::Null | Event::Bool(_) | Event::Int(_) | Event::Float(_) => {
                consume_value(&mut stack, &mut root_seen)?;
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
            Event::StartMap(Some(1)),
            Event::Key("items".into()),
            Event::StartArray(Some(2)),
            Event::Int(1),
            Event::Int(2),
            Event::EndArray,
            Event::Key("done".into()),
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
            validate_events(&[Event::Key("bad".into())], Limits::default()),
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
}
