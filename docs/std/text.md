# std.text

the error of text operations and the normalization of Unicode (10.1). The methods of `String` and `Char` are in the prelude: `tondo api prelude`.

```tondo
import std.console
import std.text

let composed = text.normalize("e\u{301}")
console.println("{composed.length()} {text.category('A')} {text.script('λ')}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.text` shows the same in a terminal.

- [Functions](#functions): [`normalize`](#normalize), [`isNormalized`](#isnormalized), [`category`](#category), [`script`](#script), [`collator`](#collator)
- [TextError](#texterror)
- [Normalization](#normalization)
- [Category](#category-1)
- [Script](#script-1)
- [Strength](#strength)
- [Collator](#collator-1): [`Collator.compare`](#collatorcompare)
- [For other types](#for-other-types)

### Functions

#### normalize

```tondo
pub fn normalize(text: String, form: Normalization = Normalization.Nfc): String
```

The text in the normalization form `form`, which gives texts that look
the same the same scalars: `"cafe\u{301}"` is `"café"` in NFC.

#### isNormalized

```tondo
pub fn isNormalized(text: String, form: Normalization = Normalization.Nfc): Bool
```

Whether the text is already in the normalization form `form`.

#### category

```tondo
pub fn category(c: Char): Category
```

The general category of a character: `text.category('a')` is
`Category.LowercaseLetter`.

#### script

```tondo
pub fn script(c: Char): Script
```

The script of a character: `text.script('字')` is `Script.Han`.

#### collator

```tondo
pub fn collator(
    locale: String,
    strength: Strength = Strength.Tertiary,
    numeric: Bool = false,
): Collator
```

A collator for `locale`, a BCP 47 tag: `es`, `sv`, `de-u-co-phonebk`;
`und`, or a language without an order of its own, takes Unicode's. With
`numeric`, digits compare as the numbers they write: `file2` goes
before `file10`.

### TextError

```tondo
pub enum TextError {
    InvalidIndex
    InvalidBoundary
    ResourceLimit
    /// Bytes that are not UTF-8, or text that is not a number.
    InvalidData
}
```

Why a text operation failed: an index outside the text, one inside a
character, a limit reached, or data that is not UTF-8 or not a number.

```tondo
import std.console
import std.text

match String.fromBytes([0xff]) {
    ok(decoded) => console.println(decoded)
    err(error) => {
        let failure: text.TextError = error
        console.println("{failure}")
    }
}
```

### Normalization

```tondo
pub enum Normalization {
    Nfc
    Nfd
    Nfkc
    Nfkd
}
```

A normalization form of Unicode (UAX #15): NFC and NFD compose and
decompose characters; NFKC and NFKD also replace those of compatibility
with what they stand for, as `ﬁ` with `fi`.

```tondo
import std.console
import std.text

let decomposed = text.normalize("é", form: text.Normalization.Nfd)
console.println("{decomposed.length()} {text.isNormalized(decomposed)}")
```

### Category

```tondo
pub enum Category {
    UppercaseLetter
    LowercaseLetter
    TitlecaseLetter
    ModifierLetter
    OtherLetter
    NonspacingMark
    SpacingMark
    EnclosingMark
    DecimalNumber
    LetterNumber
    OtherNumber
    ConnectorPunctuation
    DashPunctuation
    OpenPunctuation
    ClosePunctuation
    InitialPunctuation
    FinalPunctuation
    OtherPunctuation
    MathSymbol
    CurrencySymbol
    ModifierSymbol
    OtherSymbol
    SpaceSeparator
    LineSeparator
    ParagraphSeparator
    Control
    Format
    Surrogate
    PrivateUse
    Unassigned
}
```

The general category of a character in Unicode.

```tondo
import std.console
import std.text

let letters = "a1B".chars().filter((c) { text.category(c) == text.Category.UppercaseLetter }).collect()
console.println("{letters}")
```

Implements `Display`.

### Script

```tondo
pub enum Script {
    Adlam
    Ahom
    AnatolianHieroglyphs
    Arabic
    Armenian
    Avestan
    Balinese
    Bamum
    BassaVah
    Batak
    Bengali
    BeriaErfe
    Bhaiksuki
    Bopomofo
    Brahmi
    Braille
    Buginese
    Buhid
    CanadianAboriginal
    Carian
    CaucasianAlbanian
    Chakma
    Cham
    Cherokee
    Chorasmian
    Common
    Coptic
    Cuneiform
    Cypriot
    CyproMinoan
    Cyrillic
    Deseret
    Devanagari
    DivesAkuru
    Dogra
    Duployan
    EgyptianHieroglyphs
    Elbasan
    Elymaic
    Ethiopic
    Garay
    Georgian
    Glagolitic
    Gothic
    Grantha
    Greek
    Gujarati
    GunjalaGondi
    Gurmukhi
    GurungKhema
    Han
    Hangul
    HanifiRohingya
    Hanunoo
    Hatran
    Hebrew
    Hiragana
    ImperialAramaic
    Inherited
    InscriptionalPahlavi
    InscriptionalParthian
    Javanese
    Jurchen
    Kaithi
    Kannada
    Katakana
    Kawi
    KayahLi
    Kharoshthi
    KhitanSmallScript
    Khmer
    Khojki
    Khudawadi
    KiratRai
    Lao
    Latin
    Lepcha
    Limbu
    LinearA
    LinearB
    Lisu
    Lycian
    Lydian
    Mahajani
    Makasar
    Malayalam
    Mandaic
    Manichaean
    Marchen
    MasaramGondi
    Medefaidrin
    MeeteiMayek
    MendeKikakui
    MeroiticCursive
    MeroiticHieroglyphs
    Miao
    Modi
    Mongolian
    Mro
    Multani
    Myanmar
    Nabataean
    NagMundari
    Nandinagari
    NewTaiLue
    Newa
    Nko
    Nushu
    NyiakengPuachueHmong
    Ogham
    OlChiki
    OlOnal
    OldHungarian
    OldItalic
    OldNorthArabian
    OldPermic
    OldPersian
    OldSogdian
    OldSouthArabian
    OldTurkic
    OldUyghur
    Oriya
    Osage
    Osmanya
    PahawhHmong
    Palmyrene
    PauCinHau
    PhagsPa
    Phoenician
    ProtoCuneiform
    PsalterPahlavi
    Rejang
    Runic
    Samaritan
    Saurashtra
    Seal
    Sharada
    Shavian
    Siddham
    Sidetic
    SignWriting
    Sinhala
    Sogdian
    SoraSompeng
    Soyombo
    Sundanese
    Sunuwar
    SylotiNagri
    Syriac
    Tagalog
    Tagbanwa
    TaiLe
    TaiTham
    TaiViet
    TaiYo
    Takri
    Tamil
    Tangsa
    Tangut
    Telugu
    Thaana
    Thai
    Tibetan
    Tifinagh
    Tirhuta
    Todhri
    TolongSiki
    Toto
    TuluTigalari
    Ugaritic
    Unknown
    Vai
    Vithkuqi
    Wancho
    WarangCiti
    Yezidi
    Yi
    ZanabazarSquare
}
```

A script of Unicode: `Latin`, `Han`, `Arabic`; `Common` for characters
that many share, as digits and punctuation, `Inherited` for marks that
take the script of the character before, and `Unknown` for unassigned
ones.

```tondo
import std.console
import std.text

console.println("{text.script('λ') == text.Script.Greek} {text.script('ж')}")
```

Implements `Display`.

### Strength

```tondo
pub enum Strength {
    Primary
    Secondary
    Tertiary
}
```

Which differences a collator counts: `Primary`, only the letters, so
`a`, `á` and `A` are equal; `Secondary`, also the accents; `Tertiary`,
also the case.

```tondo
import std.console
import std.text

// Primary strength: `a`, `á` and `A` compare equal.
let loose = text.collator("es", strength: text.Strength.Primary)
console.println("{loose.compare("canon", "CAÑÓN")}")
```

### Collator

```tondo
pub type Collator = {
    // private fields
}
```

Compares texts as a language orders them, as the Unicode Collation
Algorithm and CLDR define it: `ñ` after `n` in Spanish, `ä` with `a` in
German and after `z` in Swedish, `ch` after `h` in Czech.

```tondo
import std.console
import std.text

let spanish = text.collator("es", numeric: true)
let sorted = ["ñu", "nube", "file10", "file9"].sortedWith((a, b) { spanish.compare(a, b) })
console.println("{sorted}")
```

#### Collator.compare

```tondo
pub fn Collator.compare(self, a: String, b: String): Int
```

-1, 0 or 1 as `a` goes before `b`, with it or after it, as `sortedWith`
takes: `names.sortedWith((a, b) { order.compare(a, b) })`.

### For other types

```tondo
impl reflect.Reflect for text.TextError
```
