//! Portable event protocol shared by typed serializers and dynamic codecs.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

/// Codec identities used by the static serialization protocols.
///
/// These are zero-sized type-level values.  They deliberately carry no
/// runtime registry or format-specific state: the concrete encoder/decoder
/// determines the wire representation and the compiler monomorphizes the
/// `Encode[C]`/`Decode[C]` implementation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Json;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MessagePack;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Protobuf;

/// A common owned value for the dynamic JSON/MessagePack path.
///
/// Protobuf intentionally keeps its wire-oriented `ProtoValue` model instead
/// of converting messages into this tree.  Typed encoders never need to
/// construct `Value`.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float32(u32),
    Float64(u64),
    /// A validated decimal token whose exact spelling is significant to JSON.
    Number(String),
    String(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Object(Vec<(String, Value)>),
    Extension {
        type_code: i8,
        payload: Vec<u8>,
    },
}

/// A borrowed, immutable view of a dynamic value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueView<'a> {
    value: &'a Value,
}

impl<'a> ValueView<'a> {
    pub fn new(value: &'a Value) -> Self {
        Self { value }
    }

    pub fn as_value(self) -> &'a Value {
        self.value
    }

    pub fn clone_value(self) -> Value {
        self.value.clone()
    }
}

/// Validated, codec-specific opaque bytes.  Construction is intentionally
/// owned by each codec module so `Raw<C>` can never be created without that
/// codec's validation step (apart from the explicitly unsafe escape hatch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raw<C> {
    bytes: Vec<u8>,
    _codec: PhantomData<fn() -> C>,
}

/// Owned bytes used by the common typed protocol.
///
/// `Vec<u8>` deliberately remains an `Array[UInt8]`; a caller that wants a
/// format binary/string payload uses this explicit wrapper.  That keeps the
/// wire shape visible in generic code and avoids a surprising codec-specific
/// special case for every `Vec` implementation.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bytes(Vec<u8>);

impl Bytes {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

    pub fn from_slice(value: &[u8]) -> Self {
        Self(value.to_vec())
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl<C> Raw<C> {
    pub(crate) fn from_validated(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            _codec: PhantomData,
        }
    }

    /// Construct raw bytes without validation.  The source language exposes
    /// this operation only from its `unsafe` surface; the Rust host bridge is
    /// memory-safe and therefore models it as an explicitly named operation.
    pub fn from_unchecked(bytes: Vec<u8>) -> Self {
        Self::from_validated(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

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

/// Public static encoder protocol from the 0.1 standard-library ABI.
///
/// `write_event` is the small primitive implemented by a format owner.  The
/// named operations are deliberately defaults so a codec cannot accidentally
/// diverge in the event vocabulary.  There is no `Any`, registration table or
/// trait object in this boundary.
pub trait Encoder<C, E> {
    fn write_event(&mut self, event: Event) -> Result<(), E>;

    fn null(&mut self) -> Result<(), E> {
        self.write_event(Event::Null)
    }

    fn bool(&mut self, value: bool) -> Result<(), E> {
        self.write_event(Event::Bool(value))
    }

    fn int(&mut self, value: i64) -> Result<(), E> {
        self.write_event(Event::Int(i128::from(value)))
    }

    fn uint(&mut self, value: u64) -> Result<(), E> {
        self.write_event(Event::UInt(u128::from(value)))
    }

    fn float32(&mut self, value: f32) -> Result<(), E> {
        self.write_event(Event::Float32(value.to_bits()))
    }

    fn float64(&mut self, value: f64) -> Result<(), E> {
        self.write_event(Event::Float64(value.to_bits()))
    }

    fn string(&mut self, value: &str) -> Result<(), E> {
        self.write_event(Event::String(value.to_owned()))
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), E> {
        self.write_event(Event::Bytes(value.to_vec()))
    }

    fn start_array(&mut self, length: Option<usize>) -> Result<(), E> {
        self.write_event(Event::StartArray(length))
    }

    fn end_array(&mut self) -> Result<(), E> {
        self.write_event(Event::EndArray)
    }

    fn start_map(&mut self, length: Option<usize>) -> Result<(), E> {
        self.write_event(Event::StartMap(length))
    }

    fn map_key(&mut self) -> Result<(), E> {
        self.write_event(Event::MapKey)
    }

    fn end_map(&mut self) -> Result<(), E> {
        self.write_event(Event::EndMap)
    }

    fn start_record(&mut self, name: &str, fields: Option<usize>) -> Result<(), E> {
        self.write_event(Event::StartRecord {
            name: name.to_owned(),
            fields,
        })
    }

    fn field(&mut self, name: &str) -> Result<(), E> {
        self.write_event(Event::Field(name.to_owned()))
    }

    fn end_record(&mut self) -> Result<(), E> {
        self.write_event(Event::EndRecord)
    }

    fn start_enum(&mut self, name: &str, variant: &str) -> Result<(), E> {
        self.write_event(Event::StartEnum {
            name: name.to_owned(),
            variant: variant.to_owned(),
        })
    }

    fn end_enum(&mut self) -> Result<(), E> {
        self.write_event(Event::EndEnum)
    }
}

/// Public static decoder protocol from the 0.1 standard-library ABI.
pub trait Decoder<C, E> {
    fn limits(&self) -> Limits;

    fn peek_event(&mut self) -> Result<Option<Event>, E>;

    fn next(&mut self) -> Result<Option<Event>, E>;

    fn own(&mut self, event: Event) -> Result<Event, E> {
        Ok(event)
    }
}

/// A statically dispatched typed encoder.  `C` is a zero-sized codec identity
/// such as [`Json`], [`MessagePack`] or [`Protobuf`].
pub trait Encode<C> {
    fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
    where
        E: From<SerializationError>,
        S: Encoder<C, E>;
}

/// A statically dispatched typed decoder.  Implementations construct their
/// result only after all nested events have been validated by the decoder.
pub trait Decode<C>: Sized {
    fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
    where
        E: From<SerializationError>,
        D: Decoder<C, E>;
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

impl<C> Encoder<C, SerializationError> for EventSerializer {
    fn write_event(&mut self, event: Event) -> Result<(), SerializationError> {
        <Self as Serializer>::write_event(self, event)
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

impl<C> Decoder<C, SerializationError> for EventDeserializer<'_> {
    fn limits(&self) -> Limits {
        self.limits
    }

    fn peek_event(&mut self) -> Result<Option<Event>, SerializationError> {
        <Self as Deserializer>::peek_event(self)
    }

    fn next(&mut self) -> Result<Option<Event>, SerializationError> {
        <Self as Deserializer>::next_event(self)
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

fn next_required<C, E, D>(decoder: &mut D) -> Result<Event, E>
where
    E: From<SerializationError>,
    D: Decoder<C, E>,
{
    decoder
        .next()?
        .ok_or_else(|| E::from(SerializationError::EndOfInput))
}

fn type_mismatch<E>() -> E
where
    E: From<SerializationError>,
{
    E::from(SerializationError::TypeMismatch)
}

macro_rules! static_scalar_codec {
    ($ty:ty, $encode:ident, $pattern:pat => $value:expr) => {
        impl<C> Encode<C> for $ty {
            fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
            where
                E: From<SerializationError>,
                S: Encoder<C, E>,
            {
                encoder.$encode(*self)
            }
        }

        impl<C> Decode<C> for $ty {
            fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
            where
                E: From<SerializationError>,
                D: Decoder<C, E>,
            {
                match next_required(decoder)? {
                    $pattern => Ok($value),
                    _ => Err(type_mismatch()),
                }
            }
        }
    };
}

static_scalar_codec!(bool, bool, Event::Bool(value) => value);
macro_rules! signed_scalar_codec {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl<C> Encode<C> for $ty {
                fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
                where
                    E: From<SerializationError>,
                    S: Encoder<C, E>,
                {
                    encoder.int(i64::from(*self))
                }
            }

            impl<C> Decode<C> for $ty {
                fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
                where
                    E: From<SerializationError>,
                    D: Decoder<C, E>,
                {
                    match next_required(decoder)? {
                        Event::Int(value) => <$ty>::try_from(value).map_err(|_| type_mismatch()),
                        Event::UInt(value) => <$ty>::try_from(value).map_err(|_| type_mismatch()),
                        _ => Err(type_mismatch()),
                    }
                }
            }
        )+
    };
}

macro_rules! unsigned_scalar_codec {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl<C> Encode<C> for $ty {
                fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
                where
                    E: From<SerializationError>,
                    S: Encoder<C, E>,
                {
                    encoder.uint(u64::from(*self))
                }
            }

            impl<C> Decode<C> for $ty {
                fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
                where
                    E: From<SerializationError>,
                    D: Decoder<C, E>,
                {
                    match next_required(decoder)? {
                        Event::UInt(value) => <$ty>::try_from(value).map_err(|_| type_mismatch()),
                        Event::Int(value) => <$ty>::try_from(value).map_err(|_| type_mismatch()),
                        _ => Err(type_mismatch()),
                    }
                }
            }
        )+
    };
}

signed_scalar_codec!(i8, i16, i32, i64);
unsigned_scalar_codec!(u8, u16, u32, u64);

impl<C> Encode<C> for f32 {
    fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
    where
        E: From<SerializationError>,
        S: Encoder<C, E>,
    {
        encoder.float32(*self)
    }
}

impl<C> Decode<C> for f32 {
    fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
    where
        E: From<SerializationError>,
        D: Decoder<C, E>,
    {
        match next_required(decoder)? {
            Event::Float32(bits) => Ok(f32::from_bits(bits)),
            Event::Float64(bits) => Ok(f64::from_bits(bits) as f32),
            Event::Int(value) => Ok(value as f32),
            Event::UInt(value) => Ok(value as f32),
            _ => Err(type_mismatch()),
        }
    }
}

impl<C> Encode<C> for f64 {
    fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
    where
        E: From<SerializationError>,
        S: Encoder<C, E>,
    {
        encoder.float64(*self)
    }
}

impl<C> Decode<C> for f64 {
    fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
    where
        E: From<SerializationError>,
        D: Decoder<C, E>,
    {
        match next_required(decoder)? {
            Event::Float64(bits) => Ok(f64::from_bits(bits)),
            Event::Float32(bits) => Ok(f32::from_bits(bits) as f64),
            Event::Int(value) => Ok(value as f64),
            Event::UInt(value) => Ok(value as f64),
            _ => Err(type_mismatch()),
        }
    }
}

impl<C> Encode<C> for String {
    fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
    where
        E: From<SerializationError>,
        S: Encoder<C, E>,
    {
        encoder.string(self)
    }
}

impl<C> Decode<C> for String {
    fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
    where
        E: From<SerializationError>,
        D: Decoder<C, E>,
    {
        match next_required(decoder)? {
            Event::String(value) => Ok(value),
            _ => Err(type_mismatch()),
        }
    }
}

impl<C> Encode<C> for &str {
    fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
    where
        E: From<SerializationError>,
        S: Encoder<C, E>,
    {
        encoder.string(self)
    }
}

impl<C> Encode<C> for Bytes {
    fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
    where
        E: From<SerializationError>,
        S: Encoder<C, E>,
    {
        encoder.bytes(self.as_slice())
    }
}

impl<C> Decode<C> for Bytes {
    fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
    where
        E: From<SerializationError>,
        D: Decoder<C, E>,
    {
        match next_required(decoder)? {
            Event::Bytes(value) => Ok(Self::new(value)),
            _ => Err(type_mismatch()),
        }
    }
}

impl<C> Encode<C> for () {
    fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
    where
        E: From<SerializationError>,
        S: Encoder<C, E>,
    {
        encoder.null()
    }
}

impl<C> Decode<C> for () {
    fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
    where
        E: From<SerializationError>,
        D: Decoder<C, E>,
    {
        match next_required(decoder)? {
            Event::Null => Ok(()),
            _ => Err(type_mismatch()),
        }
    }
}

impl<C, T: Encode<C>> Encode<C> for Vec<T> {
    fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
    where
        E: From<SerializationError>,
        S: Encoder<C, E>,
    {
        encoder.start_array(Some(self.len()))?;
        for value in self {
            value.encode(encoder)?;
        }
        encoder.end_array()
    }
}

impl<C, T: Decode<C>> Decode<C> for Vec<T> {
    fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
    where
        E: From<SerializationError>,
        D: Decoder<C, E>,
    {
        let declared = match next_required(decoder)? {
            Event::StartArray(length) => length,
            _ => return Err(type_mismatch()),
        };
        if declared.is_some_and(|length| length > decoder.limits().max_container_items) {
            return Err(E::from(SerializationError::LimitExceeded));
        }
        let mut values = Vec::new();
        if let Some(length) = declared {
            values.reserve(length);
        }
        loop {
            match decoder.peek_event()? {
                Some(Event::EndArray) => {
                    let _ = decoder.next()?;
                    break;
                }
                Some(_) => {
                    if values.len() >= decoder.limits().max_container_items {
                        return Err(E::from(SerializationError::LimitExceeded));
                    }
                    values.push(T::decode(decoder)?);
                }
                None => return Err(E::from(SerializationError::EndOfInput)),
            }
        }
        if declared.is_some_and(|length| length != values.len()) {
            return Err(E::from(SerializationError::InvalidContainerLength));
        }
        Ok(values)
    }
}

impl<C, K, V> Encode<C> for BTreeMap<K, V>
where
    K: Encode<C>,
    V: Encode<C>,
{
    fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
    where
        E: From<SerializationError>,
        S: Encoder<C, E>,
    {
        encoder.start_map(Some(self.len()))?;
        for (key, value) in self {
            encoder.map_key()?;
            key.encode(encoder)?;
            value.encode(encoder)?;
        }
        encoder.end_map()
    }
}

impl<C, K, V> Decode<C> for BTreeMap<K, V>
where
    K: Decode<C> + Ord,
    V: Decode<C>,
{
    fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
    where
        E: From<SerializationError>,
        D: Decoder<C, E>,
    {
        let declared = match next_required(decoder)? {
            Event::StartMap(length) => length,
            _ => return Err(type_mismatch()),
        };
        if declared.is_some_and(|length| length > decoder.limits().max_container_items) {
            return Err(E::from(SerializationError::LimitExceeded));
        }
        let mut values = BTreeMap::new();
        loop {
            match decoder.peek_event()? {
                Some(Event::EndMap) => {
                    let _ = decoder.next()?;
                    break;
                }
                Some(Event::MapKey) => {
                    let _ = decoder.next()?;
                    if values.len() >= decoder.limits().max_container_items {
                        return Err(E::from(SerializationError::LimitExceeded));
                    }
                    let key = K::decode(decoder)?;
                    let value = V::decode(decoder)?;
                    if values.insert(key, value).is_some() {
                        return Err(E::from(SerializationError::DuplicateField));
                    }
                }
                Some(_) => return Err(E::from(SerializationError::UnexpectedEvent)),
                None => return Err(E::from(SerializationError::EndOfInput)),
            }
        }
        if declared.is_some_and(|length| length != values.len()) {
            return Err(E::from(SerializationError::InvalidContainerLength));
        }
        Ok(values)
    }
}

impl<C, T: Encode<C>> Encode<C> for Option<T> {
    fn encode<E, S>(&self, encoder: &mut S) -> Result<(), E>
    where
        E: From<SerializationError>,
        S: Encoder<C, E>,
    {
        match self {
            Some(value) => value.encode(encoder),
            None => encoder.null(),
        }
    }
}

impl<C, T: Decode<C>> Decode<C> for Option<T> {
    fn decode<E, D>(decoder: &mut D) -> Result<Self, E>
    where
        E: From<SerializationError>,
        D: Decoder<C, E>,
    {
        match decoder.peek_event()? {
            Some(Event::Null) => {
                let _ = decoder.next()?;
                Ok(None)
            }
            Some(_) => T::decode(decoder).map(Some),
            None => Err(E::from(SerializationError::EndOfInput)),
        }
    }
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

    #[test]
    fn canonical_encode_decode_protocol_is_static_and_codec_parameterized() {
        let value = Some(vec![1_i16, -2_i16, 3_i16]);
        let mut encoder = EventSerializer::new(Limits::default());
        <Option<Vec<i16>> as Encode<Json>>::encode::<SerializationError, _>(&value, &mut encoder)
            .unwrap();
        let events = encoder.finish().unwrap();
        assert_eq!(
            events,
            vec![
                Event::StartArray(Some(3)),
                Event::Int(1),
                Event::Int(-2),
                Event::Int(3),
                Event::EndArray,
            ]
        );

        let mut decoder = EventDeserializer::new(&events, Limits::default()).unwrap();
        let decoded = <Option<Vec<i16>> as Decode<MessagePack>>::decode::<SerializationError, _>(
            &mut decoder,
        )
        .unwrap();
        decoder.finish().unwrap();
        assert_eq!(decoded, value);
    }

    #[test]
    fn canonical_protocol_rejects_declared_array_length_mismatch_atomically() {
        let events = [Event::StartArray(Some(2)), Event::Int(7), Event::EndArray];
        let mut decoder = EventDeserializer::new(&events, Limits::default()).unwrap();
        let result = <Vec<i64> as Decode<Protobuf>>::decode::<SerializationError, _>(&mut decoder);
        assert_eq!(result, Err(SerializationError::InvalidContainerLength));
    }

    #[test]
    fn dynamic_value_views_and_raw_bytes_are_owned_or_borrowed_explicitly() {
        let value = Value::Object(vec![("answer".into(), Value::Int(42))]);
        let view = ValueView::new(&value);
        assert_eq!(view.as_value(), &value);
        assert_eq!(view.clone_value(), value);

        let raw = Raw::<Json>::from_validated(vec![b'{', b'}']);
        assert_eq!(raw.as_bytes(), b"{}");
        assert_eq!(raw.clone().into_bytes(), b"{}");
        let unchecked = Raw::<MessagePack>::from_unchecked(vec![0xc0]);
        assert_eq!(unchecked.as_bytes(), &[0xc0]);
    }

    #[test]
    fn canonical_protocol_keeps_bytes_unit_and_maps_explicit() {
        let mut values = BTreeMap::new();
        values.insert("payload".to_owned(), Bytes::from_slice(&[1, 2, 3]));
        let mut encoder = EventSerializer::new(Limits::default());
        <BTreeMap<String, Bytes> as Encode<Json>>::encode::<SerializationError, _>(
            &values,
            &mut encoder,
        )
        .unwrap();
        let events = encoder.finish().unwrap();
        assert_eq!(
            events,
            vec![
                Event::StartMap(Some(1)),
                Event::MapKey,
                Event::String("payload".into()),
                Event::Bytes(vec![1, 2, 3]),
                Event::EndMap,
            ]
        );
        let mut decoder = EventDeserializer::new(&events, Limits::default()).unwrap();
        let decoded = <BTreeMap<String, Bytes> as Decode<Json>>::decode::<SerializationError, _>(
            &mut decoder,
        )
        .unwrap();
        decoder.finish().unwrap();
        assert_eq!(decoded.get("payload").unwrap().as_slice(), &[1, 2, 3]);

        let mut unit_encoder = EventSerializer::new(Limits::default());
        <() as Encode<Json>>::encode::<SerializationError, _>(&(), &mut unit_encoder).unwrap();
        let unit_events = unit_encoder.finish().unwrap();
        assert_eq!(unit_events, vec![Event::Null]);
        let mut unit_decoder = EventDeserializer::new(&unit_events, Limits::default()).unwrap();
        <() as Decode<Json>>::decode::<SerializationError, _>(&mut unit_decoder).unwrap();
        unit_decoder.finish().unwrap();
    }
}
