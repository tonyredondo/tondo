//! The text operations of the prelude that need the runtime (standard
//! library 14.1): splitting, Unicode case and classes, and numbers to and
//! from text. Positions are scalar indexes, as in the language.

use super::{ArrayHeader, Str, element, str_new, tondo_rt_array_new};

fn text<'a>(s: *const Str) -> &'a str {
    unsafe { super::str_text(s) }
}

/// An `Array[String]` of new strings.
fn strings<'a>(parts: impl ExactSizeIterator<Item = &'a str>) -> *mut ArrayHeader {
    let array = tondo_rt_array_new(8, parts.len());
    for (index, part) in parts.enumerate() {
        unsafe { *(element(array, 8, index) as *mut *mut Str) = str_new(part.as_bytes()) };
    }
    array
}

/// The pieces between each `separator`; the characters, when it is empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_split(s: *const Str, separator: *const Str) -> *mut ArrayHeader {
    let (s, separator) = (text(s), text(separator));
    if separator.is_empty() {
        let chars: Vec<String> = s.chars().map(String::from).collect();
        return strings(chars.iter().map(String::as_str));
    }
    let (s, separator) = (s.as_bytes(), separator.as_bytes());
    let finder = memchr::memmem::Finder::new(separator);
    // Counted first, so that the array is made once at its length.
    let count = match separator {
        [byte] => s.iter().filter(|c| *c == byte).count(),
        _ => finder.find_iter(s).count(),
    };
    let array = tondo_rt_array_new(8, count + 1);
    let (mut index, mut start) = (0, 0);
    let mut piece = |end: usize| {
        unsafe { *(element(array, 8, index) as *mut *mut Str) = str_new(&s[start..end]) };
        index += 1;
        start = end + separator.len();
    };
    match separator {
        // A byte, as a comma, is looked for byte by byte: the pieces it
        // ends are short, shorter than what a search sets up for each.
        [byte] => s.iter().enumerate().filter(|(_, c)| *c == byte).for_each(|(at, _)| piece(at)),
        _ => finder.find_iter(s).for_each(&mut piece),
    }
    piece(s.len());
    array
}

/// The scalar that starts at a byte offset of a string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_scalar_at(s: *const Str, offset: i64) -> u32 {
    let rest = text(s).get(offset.max(0) as usize..).unwrap_or_default();
    rest.chars().next().map_or(0xFFFD, |c| c as u32)
}

/// The text between two byte offsets that start characters; empty when
/// they do not.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_between(s: *const Str, start: i64, end: i64) -> *mut Str {
    str_new(text(s).get(start.max(0) as usize..end.max(0) as usize).unwrap_or_default().as_bytes())
}

/// The byte offset of the first `needle` between two byte offsets, or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_find_in(s: *const Str, needle: *const Str, from: i64, to: i64) -> i64 {
    let from = from.max(0) as usize;
    let found = text(s).get(from..to.max(0) as usize).and_then(|part| part.find(text(needle)));
    found.map_or(-1, |at| (from + at) as i64)
}

/// The lines, without their `\n` or `\r\n`; no last empty line.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_lines(s: *const Str) -> *mut ArrayHeader {
    let lines: Vec<&str> = text(s).lines().collect();
    strings(lines.into_iter())
}

/// The pieces between runs of Unicode whitespace.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_words(s: *const Str) -> *mut ArrayHeader {
    let words: Vec<&str> = text(s).split_whitespace().collect();
    strings(words.into_iter())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_repeat(s: *const Str, count: i64) -> *mut Str {
    str_new(text(s).repeat(count.max(0) as usize).as_bytes())
}

/// The text without whitespace, or without the scalars of `characters`
/// when `whitespace` is 0, at both ends (`ends` 0), at its start (1) or at
/// its end (2).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_trim(
    s: *const Str,
    characters: *const Str,
    whitespace: u8,
    ends: i64,
) -> *mut Str {
    let (s, characters) = (text(s), text(characters));
    let trimmed = |c: char| if whitespace != 0 { c.is_whitespace() } else { characters.contains(c) };
    let kept = match ends {
        1 => s.trim_start_matches(trimmed),
        2 => s.trim_end_matches(trimmed),
        _ => s.trim_matches(trimmed),
    };
    str_new(kept.as_bytes())
}

/// The scalar index of the last `needle`, or `-1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_find_last(s: *const Str, needle: *const Str) -> i64 {
    let s = text(s);
    s.rfind(text(needle)).map_or(-1, |at| s[..at].chars().count() as i64)
}

/// How many times `needle` appears, without overlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_count(s: *const Str, needle: *const Str) -> i64 {
    let needle = text(needle);
    if needle.is_empty() {
        return text(s).chars().count() as i64 + 1;
    }
    text(s).matches(needle).count() as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_lower(s: *const Str) -> *mut Str {
    str_new(text(s).to_lowercase().as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_upper(s: *const Str) -> *mut Str {
    str_new(text(s).to_uppercase().as_bytes())
}

/// The integer a text writes in `radix` (2 to 36), with an optional sign
/// and `_` between digits; `ok` tells whether it is one that fits.
fn parse_int(s: &str, radix: i64) -> Option<i64> {
    let radix = u32::try_from(radix).ok().filter(|r| (2..=36).contains(r))?;
    let (negative, digits) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let digits: String = digits.chars().filter(|&c| c != '_').collect();
    let magnitude = u64::from_str_radix(&digits, radix).ok()?;
    if negative { 0i64.checked_sub_unsigned(magnitude) } else { i64::try_from(magnitude).ok() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_is_int(s: *const Str, radix: i64) -> u8 {
    parse_int(text(s), radix).is_some() as u8
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_parse_int(s: *const Str, radix: i64) -> i64 {
    parse_int(text(s), radix).unwrap_or(0)
}

/// A decimal float, `inf`, `-inf` or `NaN`, as Tondo writes them.
pub(crate) fn parse_float(s: &str) -> Option<f64> {
    match s {
        "inf" | "+inf" => return Some(f64::INFINITY),
        "-inf" => return Some(f64::NEG_INFINITY),
        "NaN" => return Some(f64::NAN),
        _ => {}
    }
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if !digits.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        || digits.contains(|c: char| c.is_alphabetic() && c != 'e' && c != 'E')
    {
        return None;
    }
    s.parse().ok()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_is_float(s: *const Str) -> u8 {
    parse_float(text(s)).is_some() as u8
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_parse_float(s: *const Str) -> f64 {
    parse_float(text(s)).unwrap_or(0.0)
}

/// An integer in `radix` (2 to 36), with lowercase digits.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_text_int(value: i64, radix: i64) -> *mut Str {
    let radix = radix.clamp(2, 36) as u64;
    let mut magnitude = value.unsigned_abs();
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit((magnitude % radix) as u32, radix as u32).unwrap() as u8);
        magnitude /= radix;
        if magnitude == 0 {
            break;
        }
    }
    if value < 0 {
        digits.push(b'-');
    }
    digits.reverse();
    str_new(&digits)
}

/// A float with exactly `decimals` digits after the point, rounded half to
/// even as the value is written.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_text_fixed(value: f64, decimals: i64) -> *mut Str {
    if !value.is_finite() {
        return str_new(format!("{value:?}").as_bytes());
    }
    str_new(format!("{value:.*}", decimals.clamp(0, 64) as usize).as_bytes())
}

/// A float as the format of an interpolation writes it (language 19.6):
/// with exactly `precision` digits after the point, or as few as read back
/// as the same value, of a `Float32` when `single`, when `precision` is
/// negative; in scientific notation, `1.5e3`, when `exponent`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_text_float(value: f64, precision: i64, exponent: u8, single: u8) -> *mut Str {
    let text = match (precision, exponent != 0, single != 0) {
        (..0, true, true) => format!("{:e}", value as f32),
        (..0, true, false) => format!("{value:e}"),
        (..0, false, true) => format!("{:?}", value as f32),
        (..0, false, false) => format!("{value:?}"),
        (_, true, _) => format!("{value:.*e}", precision.min(64) as usize),
        (_, false, _) if !value.is_finite() => format!("{value:?}"),
        (_, false, _) => format!("{value:.*}", precision.min(64) as usize),
    };
    str_new(text.as_bytes())
}

/// The text in a normalization form of Unicode: 0 NFC, 1 NFD, 2 NFKC,
/// 3 NFKD.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_normalize(s: *const Str, form: i64) -> *mut Str {
    use unicode_normalization::UnicodeNormalization;
    let text = text(s);
    let normal: String = match form {
        1 => text.nfd().collect(),
        2 => text.nfkc().collect(),
        3 => text.nfkd().collect(),
        _ => text.nfc().collect(),
    };
    str_new(normal.as_bytes())
}

/// Whether the text is in a normalization form, as `tondo_rt_text_normalize`
/// numbers them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_is_normalized(s: *const Str, form: i64) -> u8 {
    use unicode_normalization::{is_nfc, is_nfd, is_nfkc, is_nfkd};
    let text = text(s);
    (match form {
        1 => is_nfd(text),
        2 => is_nfkc(text),
        3 => is_nfkd(text),
        _ => is_nfc(text),
    }) as u8
}

fn scalar(c: u32) -> char {
    char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// Which class a scalar is in: 0 decimal digit, 1 letter, 2 whitespace,
/// 3 uppercase, 4 lowercase, 5 letter or digit, 6 punctuation, 7 symbol,
/// 8 mark, as their general categories say.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_char_is(c: u32, class: i64) -> u8 {
    let scalar = scalar(c);
    (match class {
        0 => scalar.is_ascii_digit(),
        1 => scalar.is_alphabetic(),
        2 => scalar.is_whitespace(),
        3 => scalar.is_uppercase(),
        4 => scalar.is_lowercase(),
        5 => scalar.is_alphanumeric(),
        6 => (11..=17).contains(&tondo_rt_char_category(c)),
        7 => (18..=21).contains(&tondo_rt_char_category(c)),
        _ => (5..=7).contains(&tondo_rt_char_category(c)),
    }) as u8
}

/// The general category of a scalar, as `text.Category` orders them.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_char_category(c: u32) -> i64 {
    use icu_properties::props::GeneralCategory as G;
    match icu_properties::CodePointMapData::<G>::new().get(scalar(c)) {
        G::UppercaseLetter => 0,
        G::LowercaseLetter => 1,
        G::TitlecaseLetter => 2,
        G::ModifierLetter => 3,
        G::OtherLetter => 4,
        G::NonspacingMark => 5,
        G::SpacingMark => 6,
        G::EnclosingMark => 7,
        G::DecimalNumber => 8,
        G::LetterNumber => 9,
        G::OtherNumber => 10,
        G::ConnectorPunctuation => 11,
        G::DashPunctuation => 12,
        G::OpenPunctuation => 13,
        G::ClosePunctuation => 14,
        G::InitialPunctuation => 15,
        G::FinalPunctuation => 16,
        G::OtherPunctuation => 17,
        G::MathSymbol => 18,
        G::CurrencySymbol => 19,
        G::ModifierSymbol => 20,
        G::OtherSymbol => 21,
        G::SpaceSeparator => 22,
        G::LineSeparator => 23,
        G::ParagraphSeparator => 24,
        G::Control => 25,
        G::Format => 26,
        G::Surrogate => 27,
        G::PrivateUse => 28,
        G::Unassigned => 29,
    }
}

/// The scripts of Unicode that have characters, by their long names, in
/// the order of `text.Script`: alphabetical.
const SCRIPTS: [&str; 178] = [
    "Adlam",
    "Ahom",
    "Anatolian_Hieroglyphs",
    "Arabic",
    "Armenian",
    "Avestan",
    "Balinese",
    "Bamum",
    "Bassa_Vah",
    "Batak",
    "Bengali",
    "Beria_Erfe",
    "Bhaiksuki",
    "Bopomofo",
    "Brahmi",
    "Braille",
    "Buginese",
    "Buhid",
    "Canadian_Aboriginal",
    "Carian",
    "Caucasian_Albanian",
    "Chakma",
    "Cham",
    "Cherokee",
    "Chorasmian",
    "Common",
    "Coptic",
    "Cuneiform",
    "Cypriot",
    "Cypro_Minoan",
    "Cyrillic",
    "Deseret",
    "Devanagari",
    "Dives_Akuru",
    "Dogra",
    "Duployan",
    "Egyptian_Hieroglyphs",
    "Elbasan",
    "Elymaic",
    "Ethiopic",
    "Garay",
    "Georgian",
    "Glagolitic",
    "Gothic",
    "Grantha",
    "Greek",
    "Gujarati",
    "Gunjala_Gondi",
    "Gurmukhi",
    "Gurung_Khema",
    "Han",
    "Hangul",
    "Hanifi_Rohingya",
    "Hanunoo",
    "Hatran",
    "Hebrew",
    "Hiragana",
    "Imperial_Aramaic",
    "Inherited",
    "Inscriptional_Pahlavi",
    "Inscriptional_Parthian",
    "Javanese",
    "Jurchen",
    "Kaithi",
    "Kannada",
    "Katakana",
    "Kawi",
    "Kayah_Li",
    "Kharoshthi",
    "Khitan_Small_Script",
    "Khmer",
    "Khojki",
    "Khudawadi",
    "Kirat_Rai",
    "Lao",
    "Latin",
    "Lepcha",
    "Limbu",
    "Linear_A",
    "Linear_B",
    "Lisu",
    "Lycian",
    "Lydian",
    "Mahajani",
    "Makasar",
    "Malayalam",
    "Mandaic",
    "Manichaean",
    "Marchen",
    "Masaram_Gondi",
    "Medefaidrin",
    "Meetei_Mayek",
    "Mende_Kikakui",
    "Meroitic_Cursive",
    "Meroitic_Hieroglyphs",
    "Miao",
    "Modi",
    "Mongolian",
    "Mro",
    "Multani",
    "Myanmar",
    "Nabataean",
    "Nag_Mundari",
    "Nandinagari",
    "New_Tai_Lue",
    "Newa",
    "Nko",
    "Nushu",
    "Nyiakeng_Puachue_Hmong",
    "Ogham",
    "Ol_Chiki",
    "Ol_Onal",
    "Old_Hungarian",
    "Old_Italic",
    "Old_North_Arabian",
    "Old_Permic",
    "Old_Persian",
    "Old_Sogdian",
    "Old_South_Arabian",
    "Old_Turkic",
    "Old_Uyghur",
    "Oriya",
    "Osage",
    "Osmanya",
    "Pahawh_Hmong",
    "Palmyrene",
    "Pau_Cin_Hau",
    "Phags_Pa",
    "Phoenician",
    "Proto_Cuneiform",
    "Psalter_Pahlavi",
    "Rejang",
    "Runic",
    "Samaritan",
    "Saurashtra",
    "Seal",
    "Sharada",
    "Shavian",
    "Siddham",
    "Sidetic",
    "SignWriting",
    "Sinhala",
    "Sogdian",
    "Sora_Sompeng",
    "Soyombo",
    "Sundanese",
    "Sunuwar",
    "Syloti_Nagri",
    "Syriac",
    "Tagalog",
    "Tagbanwa",
    "Tai_Le",
    "Tai_Tham",
    "Tai_Viet",
    "Tai_Yo",
    "Takri",
    "Tamil",
    "Tangsa",
    "Tangut",
    "Telugu",
    "Thaana",
    "Thai",
    "Tibetan",
    "Tifinagh",
    "Tirhuta",
    "Todhri",
    "Tolong_Siki",
    "Toto",
    "Tulu_Tigalari",
    "Ugaritic",
    "Unknown",
    "Vai",
    "Vithkuqi",
    "Wancho",
    "Warang_Citi",
    "Yezidi",
    "Yi",
    "Zanabazar_Square",
];
/// The script of a scalar, as `text.Script` orders them.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_char_script(c: u32) -> i64 {
    use icu_properties::props::Script;
    let script = icu_properties::CodePointMapData::<Script>::new().get(scalar(c));
    let name = icu_properties::PropertyNamesLong::<Script>::new().get(script).unwrap_or("Unknown");
    SCRIPTS.binary_search(&name).or_else(|_| SCRIPTS.binary_search(&"Unknown")).map_or(0, |at| at as i64)
}

/// `text.collator`: what compares texts as a language orders them, at a
/// strength from 0, letters only, to 2, also case; with numbers by their
/// value.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_text_collator(
    locale: *const Str,
    strength: i64,
    numeric: u8,
) -> *mut super::native::NativeObject {
    use icu_collator::options::{CollatorOptions, Strength};
    use icu_collator::preferences::CollationNumericOrdering;
    let locale = icu_locale_core::Locale::try_from_str(text(locale)).unwrap_or(icu_locale_core::Locale::UNKNOWN);
    let mut preferences = icu_collator::CollatorPreferences::from(&locale);
    preferences.numeric_ordering =
        Some(if numeric != 0 { CollationNumericOrdering::True } else { CollationNumericOrdering::False });
    let mut options = CollatorOptions::default();
    options.strength = Some(match strength {
        0 => Strength::Primary,
        1 => Strength::Secondary,
        _ => Strength::Tertiary,
    });
    let collator = icu_collator::Collator::try_new(preferences, options)
        .or_else(|_| icu_collator::Collator::try_new(Default::default(), options))
        .expect("the root collation is compiled in");
    super::native::new(collator)
}

/// `Collator.compare`: -1, 0 or 1.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_text_collate(object: *mut super::native::NativeObject, a: *const Str, b: *const Str) -> i64 {
    match super::native::get::<icu_collator::CollatorBorrowed<'static>>(object) {
        Some(collator) => collator.compare(text(a), text(b)) as i64,
        None => text(a).cmp(text(b)) as i64,
    }
}

/// The scalar in the other case when it is a single one, or the same.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_char_case(c: u32, upper: u8) -> u32 {
    let c = scalar(c);
    let mut changed: Vec<char> = if upper != 0 { c.to_uppercase().collect() } else { c.to_lowercase().collect() };
    match changed.len() {
        1 => changed.pop().unwrap() as u32,
        _ => c as u32,
    }
}

/// Whether a number is a Unicode scalar value.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_char_valid(code: i64) -> u8 {
    u32::try_from(code).ok().and_then(char::from_u32).is_some() as u8
}
