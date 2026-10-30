//! How each language writes numbers and picks its plural forms, from
//! `data/numbers.txt` (standard library 10.6), as `std/format.to` reads it.

use super::{ArrayHeader, Str};
use std::collections::HashMap;
use std::sync::LazyLock;

fn text<'a>(s: *const Str) -> &'a str {
    unsafe { super::str_text(s) }
}

#[derive(Default)]
struct Numbers {
    /// Each locale's digits, separators, signs, minimum grouping digits
    /// and patterns, and its own symbols of currencies.
    locales: HashMap<String, (Vec<String>, HashMap<String, String>)>,
    /// The digits of the currencies whose amounts do not have two.
    fractions: HashMap<String, i64>,
    /// Each language's plural categories and their conditions, in order.
    plurals: HashMap<String, Vec<String>>,
}

static NUMBERS: LazyLock<Numbers> = LazyLock::new(|| {
    use std::io::Read;
    let packed = include_bytes!(concat!(env!("OUT_DIR"), "/numbers.txt.gz"));
    let mut table = String::new();
    let _ = flate2::read::GzDecoder::new(&packed[..]).read_to_string(&mut table);
    let mut numbers = Numbers::default();
    let pairs = |list: &str| -> Vec<(String, String)> {
        let found = list.split_terminator('|').filter_map(|pair| pair.split_once('='));
        found.map(|(key, value)| (key.to_string(), value.to_string())).collect()
    };
    for line in table.lines().filter(|line| !line.starts_with('#')) {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["fractions", list] => {
                numbers.fractions =
                    pairs(list).into_iter().map(|(code, digits)| (code, digits.parse().unwrap_or(2))).collect()
            }
            ["plurals", language, rules] => {
                let rules = pairs(rules).into_iter().flat_map(|(category, condition)| [category, condition]);
                numbers.plurals.insert(language.to_ascii_lowercase(), rules.collect());
            }
            [tag, found, symbols] => {
                let found = found.split_terminator('|').map(String::from).collect();
                let symbols = pairs(symbols).into_iter().collect();
                numbers.locales.insert(tag.to_ascii_lowercase(), (found, symbols));
            }
            _ => {}
        }
    }
    numbers
});

/// The tag of a BCP 47 locale in the data's form, and its language.
fn tag(locale: *const Str) -> (String, String) {
    let tag = text(locale).replace('_', "-").to_ascii_lowercase();
    let language = tag.split('-').next().unwrap_or_default().to_string();
    (tag, language)
}

/// What a locale has, or its language, or English.
fn entry<T>(table: &HashMap<String, T>, locale: *const Str) -> &T {
    let (tag, language) = tag(locale);
    table.get(&tag).or_else(|| table.get(&language)).or_else(|| table.get("en")).expect("English's numbers")
}

/// A locale's digits, decimal separator, group separator, minus sign, plus
/// sign, percent sign, minimum grouping digits and its patterns of
/// decimals, percents and currencies.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_number_symbols(locale: *const Str) -> *mut ArrayHeader {
    crate::http::string_array(&entry(&NUMBERS.locales, locale).0)
}

/// The symbol a locale writes for a currency, or its code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_currency_symbol(locale: *const Str, code: *const Str) -> *mut Str {
    let (tag, language) = tag(locale);
    let code = text(code).to_ascii_uppercase();
    let found = [tag.as_str(), language.as_str(), "en"]
        .into_iter()
        .filter_map(|key| NUMBERS.locales.get(key))
        .find_map(|(_, symbols)| symbols.get(&code));
    super::str_new(found.unwrap_or(&code).as_bytes())
}

/// The digits after the point of a currency's amounts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_currency_digits(code: *const Str) -> i64 {
    NUMBERS.fractions.get(&text(code).to_ascii_uppercase()).copied().unwrap_or(2)
}

/// A language's plural categories, each followed by its condition, in the
/// order they are tried; a count that meets none is `other`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_text_plural_rules(locale: *const Str) -> *mut ArrayHeader {
    crate::http::string_array(entry(&NUMBERS.plurals, locale))
}
