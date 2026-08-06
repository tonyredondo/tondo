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
    EndOfInput,
    TypeMismatch,
    LimitExceeded,
    UnbalancedContainer,
    DuplicateField,
    InvalidContainerLength,
}

/// The statically-dispatched sink used by every typed serializer.
///
/// A serializer writes the common event vocabulary; format owners decide how
/// those events become bytes.  The trait has no `Any`/reflection escape hatch,
/// so a typed path can be monomorphized by the caller without building a DOM.
pub trait Serializer {
    type Error;

    fn write_event(&mut self, event: Event) -> Result<(), Self::Error>;
}

/// The statically-dispatched source used by every typed deserializer.
pub trait Deserializer {
    type Error;

    fn limits(&self) -> Limits;

    fn peek_event(&mut self) -> Result<Option<Event>, Self::Error>;

    fn next_event(&mut self) -> Result<Option<Event>, Self::Error>;
}

/// A value that can be encoded into the common event protocol.
pub trait Serialize {
    fn serialize<S: Serializer<Error = SerializationError>>(
        &self,
        serializer: &mut S,
    ) -> Result<(), SerializationError>;
}

/// A value that can be decoded from the common event protocol.
pub trait Deserialize: Sized {
    fn deserialize<D: Deserializer<Error = SerializationError>>(
        deserializer: &mut D,
    ) -> Result<Self, SerializationError>;
}

/// A bounded event sink used by typed codecs and tests.
#[derive(Debug, Default)]
pub struct EventSerializer {
    events: Vec<Event>,
    limits: Limits,
}

impl EventSerializer {
    pub fn new(limits: Limits) -> Self {
        Self {
            events: Vec::new(),
            limits,
        }
    }

    pub fn finish(self) -> Result<Vec<Event>, SerializationError> {
        validate_events(&self.events, self.limits)?;
        Ok(self.events)
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }
}

impl Serializer for EventSerializer {
    type Error = SerializationError;

    fn write_event(&mut self, event: Event) -> Result<(), Self::Error> {
        if self.events.len() >= self.limits.max_events {
            return Err(SerializationError::LimitExceeded);
        }
        self.events.push(event);
        Ok(())
    }
}

/// A bounded event source.  It owns the event slice only for the duration of
/// the decode and never materialises a format-specific document tree.
pub struct EventDeserializer<'a> {
    events: &'a [Event],
    index: usize,
    limits: Limits,
}

impl<'a> EventDeserializer<'a> {
    pub fn new(events: &'a [Event], limits: Limits) -> Result<Self, SerializationError> {
        if events.len() > limits.max_events {
            return Err(SerializationError::LimitExceeded);
        }
        Ok(Self {
            events,
            index: 0,
            limits,
        })
    }

    pub fn finish(&self) -> Result<(), SerializationError> {
        (self.index == self.events.len())
            .then_some(())
            .ok_or(SerializationError::UnexpectedEvent)
    }

    pub fn peek_event(&self) -> Option<&Event> {
        self.events.get(self.index)
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }
}

impl Deserializer for EventDeserializer<'_> {
    type Error = SerializationError;

    fn limits(&self) -> Limits {
        self.limits
    }

    fn peek_event(&mut self) -> Result<Option<Event>, Self::Error> {
        Ok(self.events.get(self.index).cloned())
    }

    fn next_event(&mut self) -> Result<Option<Event>, Self::Error> {
        let event = self.events.get(self.index).cloned();
        if event.is_some() {
            self.index += 1;
        }
        Ok(event)
    }
}

pub fn serialize_value<T: Serialize>(
    value: &T,
    limits: Limits,
) -> Result<Vec<Event>, SerializationError> {
    let mut serializer = EventSerializer::new(limits);
    value.serialize(&mut serializer)?;
    serializer.finish()
}

pub fn deserialize_value<T: Deserialize>(
    events: &[Event],
    limits: Limits,
) -> Result<T, SerializationError> {
    let mut deserializer = EventDeserializer::new(events, limits)?;
    let value = T::deserialize(&mut deserializer)?;
    deserializer.finish()?;
    Ok(value)
}

fn write_scalar<S: Serializer<Error = SerializationError>>(
    serializer: &mut S,
    event: Event,
) -> Result<(), SerializationError> {
    serializer.write_event(event)
}

macro_rules! scalar_codec {
    ($ty:ty, $event:expr, $pattern:pat => $value:expr) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer<Error = SerializationError>>(
                &self,
                serializer: &mut S,
            ) -> Result<(), SerializationError> {
                write_scalar(serializer, $event(*self))
            }
        }

        impl Deserialize for $ty {
            fn deserialize<D: Deserializer<Error = SerializationError>>(
                deserializer: &mut D,
            ) -> Result<Self, SerializationError> {
                match deserializer.next_event()? {
                    Some($pattern) => Ok($value),
                    Some(_) => Err(SerializationError::TypeMismatch),
                    None => Err(SerializationError::EndOfInput),
                }
            }
        }
    };
}

scalar_codec!(bool, Event::Bool, Event::Bool(value) => value);
impl Serialize for i64 {
    fn serialize<S: Serializer<Error = SerializationError>>(
        &self,
        serializer: &mut S,
    ) -> Result<(), SerializationError> {
        write_scalar(serializer, Event::Int(i128::from(*self)))
    }
}

impl Deserialize for i64 {
    fn deserialize<D: Deserializer<Error = SerializationError>>(
        deserializer: &mut D,
    ) -> Result<Self, SerializationError> {
        match deserializer.next_event()? {
            Some(Event::Int(value)) => {
                i64::try_from(value).map_err(|_| SerializationError::TypeMismatch)
            }
            Some(Event::UInt(value)) => {
                i64::try_from(value).map_err(|_| SerializationError::TypeMismatch)
            }
            Some(_) => Err(SerializationError::TypeMismatch),
            None => Err(SerializationError::EndOfInput),
        }
    }
}

impl Serialize for u64 {
    fn serialize<S: Serializer<Error = SerializationError>>(
        &self,
        serializer: &mut S,
    ) -> Result<(), SerializationError> {
        write_scalar(serializer, Event::UInt(u128::from(*self)))
    }
}

impl Deserialize for u64 {
    fn deserialize<D: Deserializer<Error = SerializationError>>(
        deserializer: &mut D,
    ) -> Result<Self, SerializationError> {
        match deserializer.next_event()? {
            Some(Event::UInt(value)) => {
                u64::try_from(value).map_err(|_| SerializationError::TypeMismatch)
            }
            Some(Event::Int(value)) => {
                u64::try_from(value).map_err(|_| SerializationError::TypeMismatch)
            }
            Some(_) => Err(SerializationError::TypeMismatch),
            None => Err(SerializationError::EndOfInput),
        }
    }
}
impl Serialize for f32 {
    fn serialize<S: Serializer<Error = SerializationError>>(
        &self,
        serializer: &mut S,
    ) -> Result<(), SerializationError> {
        write_scalar(serializer, Event::Float32(self.to_bits()))
    }
}

impl Deserialize for f32 {
    fn deserialize<D: Deserializer<Error = SerializationError>>(
        deserializer: &mut D,
    ) -> Result<Self, SerializationError> {
        let value = match deserializer.next_event()? {
            Some(Event::Float32(value)) => f32::from_bits(value),
            Some(Event::Float64(value)) => f64::from_bits(value) as f32,
            Some(Event::Int(value)) => value as f32,
            Some(Event::UInt(value)) => value as f32,
            Some(_) => return Err(SerializationError::TypeMismatch),
            None => return Err(SerializationError::EndOfInput),
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(SerializationError::TypeMismatch)
        }
    }
}

impl Serialize for f64 {
    fn serialize<S: Serializer<Error = SerializationError>>(
        &self,
        serializer: &mut S,
    ) -> Result<(), SerializationError> {
        write_scalar(serializer, Event::Float64(self.to_bits()))
    }
}

impl Deserialize for f64 {
    fn deserialize<D: Deserializer<Error = SerializationError>>(
        deserializer: &mut D,
    ) -> Result<Self, SerializationError> {
        let value = match deserializer.next_event()? {
            Some(Event::Float64(value)) => f64::from_bits(value),
            Some(Event::Float32(value)) => f32::from_bits(value) as f64,
            Some(Event::Int(value)) => value as f64,
            Some(Event::UInt(value)) => value as f64,
            Some(_) => return Err(SerializationError::TypeMismatch),
            None => return Err(SerializationError::EndOfInput),
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(SerializationError::TypeMismatch)
        }
    }
}

impl Serialize for String {
    fn serialize<S: Serializer<Error = SerializationError>>(
        &self,
        serializer: &mut S,
    ) -> Result<(), SerializationError> {
        write_scalar(serializer, Event::String(self.clone()))
    }
}

impl Deserialize for String {
    fn deserialize<D: Deserializer<Error = SerializationError>>(
        deserializer: &mut D,
    ) -> Result<Self, SerializationError> {
        match deserializer.next_event()? {
            Some(Event::String(value)) => Ok(value),
            Some(_) => Err(SerializationError::TypeMismatch),
            None => Err(SerializationError::EndOfInput),
        }
    }
}

impl<T: Serialize> Serialize for Vec<T> {
    fn serialize<S: Serializer<Error = SerializationError>>(
        &self,
        serializer: &mut S,
    ) -> Result<(), SerializationError> {
        serializer.write_event(Event::StartArray(Some(self.len())))?;
        for value in self {
            value.serialize(serializer)?;
        }
        serializer.write_event(Event::EndArray)
    }
}

impl<T: Deserialize> Deserialize for Vec<T> {
    fn deserialize<D: Deserializer<Error = SerializationError>>(
        deserializer: &mut D,
    ) -> Result<Self, SerializationError> {
        let Some(Event::StartArray(declared)) = deserializer.next_event()? else {
            return Err(SerializationError::TypeMismatch);
        };
        let mut values = Vec::new();
        if let Some(length) = declared {
            values.reserve(length.min(deserializer.limits().max_container_items));
        }
        loop {
            match deserializer.peek_event()? {
                Some(Event::EndArray) => {
                    let _ = deserializer.next_event()?;
                    break;
                }
                Some(_) => values.push(T::deserialize(deserializer)?),
                None => return Err(SerializationError::EndOfInput),
            }
        }
        Ok(values)
    }
}

impl<T: Serialize> Serialize for Option<T> {
    fn serialize<S: Serializer<Error = SerializationError>>(
        &self,
        serializer: &mut S,
    ) -> Result<(), SerializationError> {
        match self {
            Some(value) => value.serialize(serializer),
            None => serializer.write_event(Event::Null),
        }
    }
}

impl<T: Deserialize> Deserialize for Option<T> {
    fn deserialize<D: Deserializer<Error = SerializationError>>(
        deserializer: &mut D,
    ) -> Result<Self, SerializationError> {
        match deserializer.peek_event()? {
            Some(Event::Null) => {
                let _ = deserializer.next_event()?;
                Ok(None)
            }
            Some(_) => T::deserialize(deserializer).map(Some),
            None => Err(SerializationError::EndOfInput),
        }
    }
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

    #[test]
    fn typed_protocol_is_static_bounded_and_round_trips_scalars_and_arrays() {
        let value = vec![1_i64, 2_i64, 3_i64];
        let events = serialize_value(&value, Limits::default()).unwrap();
        assert_eq!(
            events,
            vec![
                Event::StartArray(Some(3)),
                Event::Int(1),
                Event::Int(2),
                Event::Int(3),
                Event::EndArray,
            ]
        );
        assert_eq!(
            deserialize_value::<Vec<i64>>(&events, Limits::default()).unwrap(),
            value
        );

        let optional = serialize_value(&Option::<String>::None, Limits::default()).unwrap();
        assert_eq!(optional, vec![Event::Null]);
        assert_eq!(
            deserialize_value::<Option<String>>(&optional, Limits::default()).unwrap(),
            None
        );
    }

    #[test]
    fn typed_protocol_publishes_only_after_complete_validation() {
        let limits = Limits {
            max_events: 2,
            ..Limits::default()
        };
        assert_eq!(
            serialize_value(&vec![1_i64, 2_i64], limits),
            Err(SerializationError::LimitExceeded)
        );
        let incomplete = [Event::StartArray(Some(1)), Event::Int(1)];
        assert_eq!(
            deserialize_value::<Vec<i64>>(&incomplete, Limits::default()),
            Err(SerializationError::EndOfInput)
        );
        let trailing = [Event::Bool(true), Event::Null];
        assert_eq!(
            deserialize_value::<bool>(&trailing, Limits::default()),
            Err(SerializationError::UnexpectedEvent)
        );
    }
}
