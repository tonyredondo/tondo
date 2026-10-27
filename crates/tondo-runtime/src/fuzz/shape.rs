//! The shape of a fuzz input, as the compiler writes it for the input's
//! type (testing 10.6), and what the search does with it: makes values of
//! it, and changes one into another, knowing what each part is.

use super::value::Json;

#[derive(Clone, Debug)]
pub(crate) enum Shape {
    Int,
    /// A `Byte`: an integer from 0 to 255.
    Byte,
    Float,
    Bool,
    Text,
    /// `serialization.Bytes`: base64 text.
    Bytes,
    DateTime,
    /// What the compiler cannot tell, as a type that encodes itself.
    Any,
    Option(Box<Shape>),
    List(Box<Shape>),
    Map(Box<Shape>),
    Ref(usize),
}

/// What a record, enum or newtype is: its fields, its variants, or the
/// shape of its value.
#[derive(Clone, Debug)]
pub(crate) enum Def {
    Record(Vec<(String, Shape)>),
    Enum(Vec<(String, Option<Payload>)>),
    Value(Shape),
}

#[derive(Clone, Debug)]
pub(crate) enum Payload {
    One(Shape),
    Tuple(Vec<Shape>),
    Record(Vec<(String, Shape)>),
}

#[derive(Clone, Debug)]
pub(crate) struct Shapes {
    pub root: Shape,
    pub defs: Vec<Def>,
}

impl Shapes {
    /// Reads what the compiler wrote: `{"root": shape, "defs": [def...]}`.
    pub(crate) fn read(text: &str) -> Shapes {
        let any = Shapes { root: Shape::Any, defs: Vec::new() };
        let Some(Json::Map(members)) = super::value::parse(text) else { return any };
        let field = |name: &str| members.iter().find(|(key, _)| key == name).map(|(_, value)| value);
        let root = field("root").map_or(Shape::Any, shape);
        let defs = match field("defs") {
            Some(Json::List(defs)) => defs.iter().map(def).collect(),
            _ => Vec::new(),
        };
        Shapes { root, defs }
    }
}

fn shape(json: &Json) -> Shape {
    match json {
        Json::Text(name) => match name.as_str() {
            "int" => Shape::Int,
            "byte" => Shape::Byte,
            "float" => Shape::Float,
            "bool" => Shape::Bool,
            "text" => Shape::Text,
            "bytes" => Shape::Bytes,
            "datetime" => Shape::DateTime,
            _ => Shape::Any,
        },
        Json::Map(members) => match members.first() {
            Some((key, inner)) if key == "option" => Shape::Option(Box::new(shape(inner))),
            Some((key, inner)) if key == "list" => Shape::List(Box::new(shape(inner))),
            Some((key, inner)) if key == "map" => Shape::Map(Box::new(shape(inner))),
            Some((key, Json::Int(index))) if key == "ref" => Shape::Ref(*index as usize),
            _ => Shape::Any,
        },
        _ => Shape::Any,
    }
}

fn fields(json: &Json) -> Vec<(String, Shape)> {
    match json {
        Json::List(fields) => fields
            .iter()
            .filter_map(|field| match field {
                Json::List(pair) => match &pair[..] {
                    [Json::Text(key), value] => Some((key.clone(), shape(value))),
                    _ => None,
                },
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn def(json: &Json) -> Def {
    let Json::Map(members) = json else { return Def::Value(shape(json)) };
    match members.first() {
        Some((key, list)) if key == "record" => Def::Record(fields(list)),
        Some((key, Json::List(variants))) if key == "enum" => Def::Enum(
            variants
                .iter()
                .filter_map(|variant| match variant {
                    Json::List(pair) => match &pair[..] {
                        [Json::Text(name), Json::Null] => Some((name.clone(), None)),
                        [Json::Text(name), payload] => Some((name.clone(), Some(self::payload(payload)))),
                        _ => None,
                    },
                    _ => None,
                })
                .collect(),
        ),
        _ => Def::Value(shape(json)),
    }
}

fn payload(json: &Json) -> Payload {
    match json {
        Json::Map(members) => match members.first() {
            Some((key, list)) if key == "record" => Payload::Record(fields(list)),
            Some((key, Json::List(items))) if key == "tuple" => Payload::Tuple(items.iter().map(shape).collect()),
            _ => Payload::One(shape(json)),
        },
        _ => Payload::One(shape(json)),
    }
}

/// A generator of numbers that does not need to be good, only fast and
/// different in each worker.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new() -> Rng {
        let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        Rng((time.as_nanos() as u64 ^ (std::process::id() as u64) << 32) | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number below `n`, which is not 0.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    pub(crate) fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

/// What mutations take values from: the program's literals and the
/// operands its comparisons met.
#[derive(Default)]
pub(crate) struct Dictionary {
    pub texts: Vec<String>,
    pub ints: Vec<i64>,
}

const INTERESTING: [i64; 18] = [
    0,
    1,
    -1,
    2,
    7,
    8,
    16,
    127,
    128,
    255,
    256,
    1024,
    65535,
    65536,
    i32::MAX as i64,
    i32::MIN as i64,
    i64::MAX,
    i64::MIN,
];

const SPECIAL: [&str; 14] = ["", " ", "\n", "\"", "\\", "{", "}", "[", "]", ",", ":", "\u{0}", "é", "😀"];

const ALPHABET: &[u8] = b"abcxyzABC019 -_.,:;/\\\"'{}[]()<>=+*&%$#@!?\n\t";

impl Shapes {
    /// A value of `shape`, small, and smaller as it nests.
    pub(crate) fn generate(&self, shape: &Shape, rng: &mut Rng, dictionary: &Dictionary, depth: usize) -> Json {
        let deep = depth > 6;
        match shape {
            Shape::Int => Json::Int(int(rng, dictionary)),
            Shape::Byte => Json::Int(i64::from(rng.next() as u8)),
            Shape::Float => Json::Float(float(rng)),
            Shape::Bool => Json::Bool(rng.chance(2)),
            Shape::Text => Json::Text(text(rng, dictionary)),
            Shape::Bytes => Json::Text(base64(&bytes(rng))),
            Shape::DateTime => Json::Text(format!(
                "20{:02}-0{}-1{}T0{}:00:00Z",
                rng.below(40),
                1 + rng.below(9),
                rng.below(9),
                rng.below(9)
            )),
            Shape::Any => Json::Null,
            Shape::Option(inner) => match deep || rng.chance(3) {
                true => Json::Null,
                false => self.generate(inner, rng, dictionary, depth + 1),
            },
            Shape::List(inner) if matches!(**inner, Shape::Byte) => byte_list(bytes(rng)),
            Shape::List(inner) => {
                let count = if deep { 0 } else { rng.below(4) };
                Json::List((0..count).map(|_| self.generate(inner, rng, dictionary, depth + 1)).collect())
            }
            Shape::Map(inner) => {
                let count = if deep { 0 } else { rng.below(3) };
                let members =
                    (0..count).map(|_| (text(rng, dictionary), self.generate(inner, rng, dictionary, depth + 1)));
                Json::Map(members.collect())
            }
            Shape::Ref(index) => match self.defs.get(*index) {
                Some(Def::Record(fields)) => self.record(fields, rng, dictionary, depth),
                Some(Def::Enum(variants)) if !variants.is_empty() => {
                    // Deep down, a variant without payload ends the value.
                    let unit = variants.iter().position(|(_, payload)| payload.is_none());
                    let at = match (deep, unit) {
                        (true, Some(unit)) => unit,
                        _ => rng.below(variants.len()),
                    };
                    self.variant(&variants[at], rng, dictionary, depth)
                }
                Some(Def::Value(inner)) => self.generate(inner, rng, dictionary, depth + 1),
                _ => Json::Null,
            },
        }
    }

    fn record(&self, fields: &[(String, Shape)], rng: &mut Rng, dictionary: &Dictionary, depth: usize) -> Json {
        Json::Map(
            fields.iter().map(|(key, shape)| (key.clone(), self.generate(shape, rng, dictionary, depth + 1))).collect(),
        )
    }

    fn variant(
        &self,
        (name, payload): &(String, Option<Payload>),
        rng: &mut Rng,
        dictionary: &Dictionary,
        depth: usize,
    ) -> Json {
        let value = match payload {
            None => return Json::Text(name.clone()),
            Some(Payload::One(shape)) => self.generate(shape, rng, dictionary, depth + 1),
            Some(Payload::Tuple(shapes)) => {
                Json::List(shapes.iter().map(|shape| self.generate(shape, rng, dictionary, depth + 1)).collect())
            }
            Some(Payload::Record(fields)) => self.record(fields, rng, dictionary, depth),
        };
        Json::Map(vec![(name.clone(), value)])
    }

    /// The shape of what is at `path` in `value`, whose shape is `shape`.
    fn shape_at<'a>(&'a self, value: &Json, shape: &'a Shape, path: &[usize]) -> Option<&'a Shape> {
        let Some((&first, rest)) = path.split_first() else { return Some(shape) };
        let shape = self.resolved(shape);
        match (value, shape) {
            (Json::List(items), Some(Shape::List(inner))) => self.shape_at(items.get(first)?, inner, rest),
            (Json::Map(members), Some(Shape::Map(inner))) => self.shape_at(&members.get(first)?.1, inner, rest),
            (_, Some(Shape::Option(inner))) => self.shape_at(value, inner, path),
            (Json::Map(members), Some(Shape::Ref(index))) => {
                let (key, member) = members.get(first)?;
                match self.defs.get(*index)? {
                    Def::Record(fields) => {
                        let (_, shape) = fields.iter().find(|(name, _)| name == key)?;
                        self.shape_at(member, shape, rest)
                    }
                    Def::Enum(variants) => {
                        let (_, payload) = variants.iter().find(|(name, _)| name == key)?;
                        match payload.as_ref()? {
                            Payload::One(shape) => self.shape_at(member, shape, rest),
                            Payload::Tuple(shapes) => {
                                let (&at, rest) = rest.split_first()?;
                                let Json::List(items) = member else { return None };
                                self.shape_at(items.get(at)?, shapes.get(at)?, rest)
                            }
                            Payload::Record(fields) => {
                                let (&at, rest) = rest.split_first()?;
                                let Json::Map(members) = member else { return None };
                                let (key, member) = members.get(at)?;
                                let (_, shape) = fields.iter().find(|(name, _)| name == key)?;
                                self.shape_at(member, shape, rest)
                            }
                        }
                    }
                    Def::Value(_) => None,
                }
            }
            (Json::List(items), Some(Shape::Ref(index))) => match self.defs.get(*index)? {
                Def::Value(inner) => self.shape_at(value, inner, path),
                _ => self.shape_at(items.get(first)?, &Shape::Any, rest),
            },
            _ => None,
        }
    }

    /// A shape with its newtypes seen through.
    fn resolved<'a>(&'a self, shape: &'a Shape) -> Option<&'a Shape> {
        let mut shape = shape;
        for _ in 0..16 {
            match shape {
                Shape::Ref(index) => match self.defs.get(*index) {
                    Some(Def::Value(inner)) => shape = inner,
                    Some(_) => return Some(shape),
                    None => return None,
                },
                _ => return Some(shape),
            }
        }
        None
    }

    /// Changes `value` in one place, or in a few: what changes and how
    /// follows what the place is; `other` is another input to take parts
    /// from.
    pub(crate) fn mutate(&self, value: &mut Json, rng: &mut Rng, dictionary: &Dictionary, other: Option<&Json>) {
        let rounds = 1 + rng.below(4);
        for _ in 0..rounds {
            let mut places = Vec::new();
            paths(value, &mut Vec::new(), &mut places);
            let path = places[rng.below(places.len())].clone();
            let shape = self.shape_at(value, &self.root.clone(), &path).cloned();
            let Some(node) = at_path(value, &path) else { continue };
            // A part from another input, where it fits.
            if let Some(other) = other.filter(|_| rng.chance(6))
                && let Some(part) = at_path_ref(other, &path).or_else(|| any_like(other, node, rng))
            {
                *node = part.clone();
                continue;
            }
            // Or a new value of the part's shape.
            if let Some(shape) = shape.as_ref().filter(|_| rng.chance(10)) {
                *node = self.generate(shape, rng, dictionary, path.len());
                continue;
            }
            self.change(node, shape.as_ref(), rng, dictionary, path.len());
        }
    }

    fn change(&self, node: &mut Json, shape: Option<&Shape>, rng: &mut Rng, dictionary: &Dictionary, depth: usize) {
        let resolved = shape.and_then(|s| self.resolved(s)).cloned();
        match (node, resolved.as_ref()) {
            (node @ Json::Null, Some(Shape::Option(inner))) => *node = self.generate(inner, rng, dictionary, depth),
            (node, Some(Shape::Option(_))) if rng.chance(3) => *node = Json::Null,
            (Json::Int(value), Some(Shape::Byte)) => {
                *value = match rng.below(3) {
                    0 => *value ^ 1 << rng.below(8),
                    1 => [0, 1, 0x7f, 0x80, 0xff][rng.below(5)],
                    _ => i64::from(rng.next() as u8),
                } & 0xff
            }
            (Json::Int(value), _) => *value = mutate_int(*value, rng, dictionary),
            (Json::Float(value), _) => *value = mutate_float(*value, rng),
            (Json::Bool(value), _) => *value = !*value,
            (Json::Text(text), Some(Shape::Bytes)) => {
                let mut data = unbase64(text);
                mutate_bytes(&mut data, rng);
                *text = base64(&data);
            }
            (Json::Text(text), Some(Shape::Ref(index))) if matches!(self.defs.get(*index), Some(Def::Enum(_))) => {
                if let Some(Def::Enum(variants)) = self.defs.get(*index) {
                    *text = String::new();
                    let variant = &variants[rng.below(variants.len())];
                    let json = self.variant(variant, rng, dictionary, depth);
                    if let Json::Text(name) = &json {
                        *text = name.clone();
                    }
                }
            }
            (Json::Text(text), _) => mutate_text(text, rng, dictionary),
            // A list of bytes changes as bytes do.
            (node @ Json::List(_), Some(Shape::List(inner))) if matches!(**inner, Shape::Byte) => {
                let Json::List(items) = node else { return };
                let mut data: Vec<u8> =
                    items.iter().map(|item| if let Json::Int(n) = item { *n as u8 } else { 0 }).collect();
                mutate_bytes(&mut data, rng);
                *node = byte_list(data);
            }
            (Json::List(items), shape) => {
                let element = match shape {
                    Some(Shape::List(inner)) => Some((**inner).clone()),
                    _ => None,
                };
                match rng.below(5) {
                    0 if !items.is_empty() => {
                        items.remove(rng.below(items.len()));
                    }
                    1 if !items.is_empty() => {
                        let copy = items[rng.below(items.len())].clone();
                        items.insert(rng.below(items.len() + 1), copy);
                    }
                    2 if items.len() > 1 => {
                        let (a, b) = (rng.below(items.len()), rng.below(items.len()));
                        items.swap(a, b);
                    }
                    3 if items.len() > 1 => items.truncate(rng.below(items.len())),
                    _ => {
                        let item = match &element {
                            Some(shape) => self.generate(shape, rng, dictionary, depth + 1),
                            None => items.first().cloned().unwrap_or(Json::Null),
                        };
                        items.insert(rng.below(items.len() + 1), item);
                    }
                }
            }
            (Json::Map(members), Some(Shape::Map(inner))) => match rng.below(3) {
                0 if !members.is_empty() => {
                    members.remove(rng.below(members.len()));
                }
                1 if !members.is_empty() => {
                    let at = rng.below(members.len());
                    mutate_text(&mut members[at].0, rng, dictionary);
                }
                _ => members.push((text(rng, dictionary), self.generate(inner, rng, dictionary, depth + 1))),
            },
            (node @ Json::Map(_), Some(shape @ Shape::Ref(index))) => match self.defs.get(*index) {
                // Another variant of the enum.
                Some(Def::Enum(_)) => *node = self.generate(shape, rng, dictionary, depth),
                _ => {
                    if let Json::Map(members) = node
                        && !members.is_empty()
                    {
                        let at = rng.below(members.len());
                        let (_, member) = &mut members[at];
                        self.change(member, None, rng, dictionary, depth + 1);
                    }
                }
            },
            (Json::Map(members), _) if !members.is_empty() => {
                let at = rng.below(members.len());
                self.change(&mut members[at].1, None, rng, dictionary, depth + 1);
            }
            (node @ Json::Null, None) => *node = Json::Int(int(rng, dictionary)),
            _ => {}
        }
    }
}

/// Every place in a value, by the indices that lead to it.
fn paths(value: &Json, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    out.push(path.clone());
    let children: Vec<&Json> = match value {
        Json::List(items) => items.iter().collect(),
        Json::Map(members) => members.iter().map(|(_, value)| value).collect(),
        _ => return,
    };
    for (index, child) in children.into_iter().enumerate() {
        path.push(index);
        paths(child, path, out);
        path.pop();
    }
}

fn at_path<'a>(value: &'a mut Json, path: &[usize]) -> Option<&'a mut Json> {
    let Some((&first, rest)) = path.split_first() else { return Some(value) };
    match value {
        Json::List(items) => at_path(items.get_mut(first)?, rest),
        Json::Map(members) => at_path(&mut members.get_mut(first)?.1, rest),
        _ => None,
    }
}

fn at_path_ref<'a>(value: &'a Json, path: &[usize]) -> Option<&'a Json> {
    let Some((&first, rest)) = path.split_first() else { return Some(value) };
    match value {
        Json::List(items) => at_path_ref(items.get(first)?, rest),
        Json::Map(members) => at_path_ref(&members.get(first)?.1, rest),
        _ => None,
    }
}

/// A part of `other` of the same kind as `like`.
fn any_like<'a>(other: &'a Json, like: &Json, rng: &mut Rng) -> Option<&'a Json> {
    let mut places = Vec::new();
    paths(other, &mut Vec::new(), &mut places);
    let same = |a: &Json, b: &Json| std::mem::discriminant(a) == std::mem::discriminant(b);
    let found: Vec<&Json> = places.iter().filter_map(|p| at_path_ref(other, p)).filter(|v| same(v, like)).collect();
    (!found.is_empty()).then(|| found[rng.below(found.len())])
}

fn int(rng: &mut Rng, dictionary: &Dictionary) -> i64 {
    match rng.below(4) {
        0 if !dictionary.ints.is_empty() => dictionary.ints[rng.below(dictionary.ints.len())],
        1 => INTERESTING[rng.below(INTERESTING.len())],
        2 => rng.next() as i64,
        _ => rng.below(200) as i64 - 50,
    }
}

fn mutate_int(value: i64, rng: &mut Rng, dictionary: &Dictionary) -> i64 {
    match rng.below(7) {
        0 => value.wrapping_add(1 + rng.below(16) as i64),
        1 => value.wrapping_sub(1 + rng.below(16) as i64),
        2 => value ^ (1 << rng.below(64)),
        3 => value.wrapping_neg(),
        4 if !dictionary.ints.is_empty() => {
            // A neighbour of what a comparison met.
            let near = dictionary.ints[rng.below(dictionary.ints.len())];
            near.wrapping_add(rng.below(3) as i64 - 1)
        }
        _ => int(rng, dictionary),
    }
}

fn float(rng: &mut Rng) -> f64 {
    match rng.below(4) {
        0 => [0.0, -0.0, 1.0, -1.0, 0.5, 1e308, -1e308, 1e-308, f64::EPSILON][rng.below(9)],
        1 => rng.below(1000) as f64 / 7.0,
        _ => (rng.next() as i64 as f64) / (1u64 << rng.below(63)) as f64,
    }
}

fn mutate_float(value: f64, rng: &mut Rng) -> f64 {
    let changed = match rng.below(4) {
        0 => value * 2.0,
        1 => value / 2.0,
        2 => -value,
        _ => float(rng),
    };
    if changed.is_finite() { changed } else { 0.0 }
}

fn text(rng: &mut Rng, dictionary: &Dictionary) -> String {
    if !dictionary.texts.is_empty() && rng.chance(3) {
        return dictionary.texts[rng.below(dictionary.texts.len())].clone();
    }
    let length = rng.below(8);
    (0..length).map(|_| ALPHABET[rng.below(ALPHABET.len())] as char).collect()
}

/// Changes a text by its characters, so that it stays UTF-8.
fn mutate_text(text: &mut String, rng: &mut Rng, dictionary: &Dictionary) {
    let mut chars: Vec<char> = text.chars().collect();
    let at = rng.below(chars.len() + 1);
    match rng.below(9) {
        0 if !chars.is_empty() => {
            let end = (at + 1 + rng.below(4)).min(chars.len());
            chars.drain(at.min(chars.len() - 1)..end);
        }
        1 if at < chars.len() => chars[at] = ALPHABET[rng.below(ALPHABET.len())] as char,
        2 => chars.insert(at, ALPHABET[rng.below(ALPHABET.len())] as char),
        3 => {
            let special: Vec<char> = SPECIAL[rng.below(SPECIAL.len())].chars().collect();
            chars.splice(at..at, special);
        }
        4 if !dictionary.texts.is_empty() => {
            let word: Vec<char> = dictionary.texts[rng.below(dictionary.texts.len())].chars().collect();
            chars.splice(at..at, word);
        }
        5 if !dictionary.texts.is_empty() => {
            chars = dictionary.texts[rng.below(dictionary.texts.len())].chars().collect();
        }
        6 if !chars.is_empty() => {
            // A run of it again.
            let start = rng.below(chars.len());
            let end = (start + 1 + rng.below(8)).min(chars.len());
            let run: Vec<char> = chars[start..end].to_vec();
            chars.splice(at..at, run);
        }
        7 if at < chars.len() => chars.truncate(at),
        _ => chars.insert(at, char::from_u32(rng.below(0x800) as u32).unwrap_or('a')),
    }
    chars.truncate(4096);
    *text = chars.into_iter().collect();
}

fn bytes(rng: &mut Rng) -> Vec<u8> {
    (0..rng.below(16)).map(|_| rng.next() as u8).collect()
}

fn mutate_bytes(data: &mut Vec<u8>, rng: &mut Rng) {
    let at = rng.below(data.len() + 1);
    match rng.below(5) {
        0 if at < data.len() => data[at] ^= 1 << rng.below(8),
        1 if at < data.len() => {
            data.remove(at);
        }
        2 if at < data.len() => data[at] = [0, 1, 0x7f, 0x80, 0xff][rng.below(5)],
        _ => data.insert(at, rng.next() as u8),
    }
    data.truncate(4096);
}

fn byte_list(data: Vec<u8>) -> Json {
    Json::List(data.into_iter().map(|byte| Json::Int(i64::from(byte))).collect())
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            match i <= chunk.len() {
                true => out.push(BASE64[(n >> (18 - 6 * i) & 63) as usize] as char),
                false => out.push('='),
            }
        }
    }
    out
}

fn unbase64(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut n, mut bits) = (0u32, 0);
    for byte in text.bytes() {
        let Some(value) = BASE64.iter().position(|&b| b == byte) else { continue };
        n = n << 6 | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((n >> bits) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM: &str = r#"{"root":{"ref":0},"defs":[{"record":[["item_name","text"],["count","int"],["tags",{"list":"text"}],["shape",{"ref":1}]]},{"enum":[["Empty",null],["Circle","float"],["Box",{"record":[["width","int"],["height","int"]]}]]}]}"#;

    #[test]
    fn makes_and_changes_values_of_their_shape() {
        let shapes = Shapes::read(ITEM);
        let (mut rng, dictionary) = (Rng(7), Dictionary::default());
        for _ in 0..200 {
            let mut value = shapes.generate(&shapes.root, &mut rng, &dictionary, 0);
            let Json::Map(members) = &value else { panic!("{value:?}") };
            let keys: Vec<&str> = members.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(keys, ["item_name", "count", "tags", "shape"]);
            let shape = &members[3].1;
            assert!(
                matches!(shape, Json::Text(name) if name == "Empty")
                    || matches!(shape, Json::Map(m) if m.len() == 1 && ["Circle", "Box"].contains(&m[0].0.as_str())),
                "{shape:?}"
            );
            let before = value.clone();
            shapes.mutate(&mut value, &mut rng, &dictionary, Some(&before));
            assert!(super::super::value::parse(&value.text()).is_some());
        }
        assert_eq!(unbase64(&base64(b"tondo!")), b"tondo!");
    }
}
