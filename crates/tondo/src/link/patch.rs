//! A live build's patches (toolchain 7.10): the object of the program
//! compiled again, laid out as one image that the runtime of the running
//! program maps wherever it can. Within the image, calls and addresses
//! are relative. What the patch takes of the running image and of the
//! runtime is a word the runtime fills from the image's tables when it
//! maps the patch, and the words that hold the patch's own addresses get
//! the image's base added.
//!
//! The file: `TONDOPT1`, then as 64-bit words the image's size, where its
//! table of slots is, and how many relocations and entries follow; the
//! image; each word to relocate; and each entry, a word with its place
//! followed by two 32-bit numbers, its table and its index.

use object::{Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SectionKind};
use std::collections::HashMap;

/// What the patch does not define: an entry of one of the running
/// program's tables.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct External {
    pub table: u32,
    pub index: u32,
}

/// What a relocation asks for, the same in ELF and Mach-O.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Absolute64,
    Relative32,
    /// A call or jump: x86-64's `call rel32`, arm64's `bl`.
    Branch32,
    Branch26,
    /// An entry that holds the target's address, x86-64's.
    GotRelative32,
    /// arm64's page of the target, and its offset in the page, scaled by
    /// the size the instruction loads or stores.
    Page21,
    PageOffset12,
    GotPage21,
    GotPageOffset12,
}

fn kind(flags: RelocationFlags, arm64: bool) -> Option<Kind> {
    use object::{elf, macho};
    Some(match flags {
        RelocationFlags::Elf { r_type } => match (arm64, r_type) {
            (false, elf::R_X86_64_64) | (true, elf::R_AARCH64_ABS64) => Kind::Absolute64,
            (false, elf::R_X86_64_PC32) | (true, elf::R_AARCH64_PREL32) => Kind::Relative32,
            (false, elf::R_X86_64_PLT32) => Kind::Branch32,
            (false, elf::R_X86_64_GOTPCREL | elf::R_X86_64_GOTPCRELX | elf::R_X86_64_REX_GOTPCRELX) => {
                Kind::GotRelative32
            }
            (true, elf::R_AARCH64_CALL26 | elf::R_AARCH64_JUMP26) => Kind::Branch26,
            (true, elf::R_AARCH64_ADR_PREL_PG_HI21) => Kind::Page21,
            (
                true,
                elf::R_AARCH64_ADD_ABS_LO12_NC
                | elf::R_AARCH64_LDST8_ABS_LO12_NC
                | elf::R_AARCH64_LDST16_ABS_LO12_NC
                | elf::R_AARCH64_LDST32_ABS_LO12_NC
                | elf::R_AARCH64_LDST64_ABS_LO12_NC
                | elf::R_AARCH64_LDST128_ABS_LO12_NC,
            ) => Kind::PageOffset12,
            (true, elf::R_AARCH64_ADR_GOT_PAGE) => Kind::GotPage21,
            (true, elf::R_AARCH64_LD64_GOT_LO12_NC) => Kind::GotPageOffset12,
            _ => return None,
        },
        RelocationFlags::MachO { r_type, r_pcrel, r_length } => match (arm64, r_type, r_pcrel, r_length) {
            (false, macho::X86_64_RELOC_UNSIGNED, false, 3) | (true, macho::ARM64_RELOC_UNSIGNED, false, 3) => {
                Kind::Absolute64
            }
            (false, macho::X86_64_RELOC_SIGNED, true, 2) => Kind::Relative32,
            (false, macho::X86_64_RELOC_BRANCH, true, 2) => Kind::Branch32,
            (false, macho::X86_64_RELOC_GOT_LOAD | macho::X86_64_RELOC_GOT, true, 2) => Kind::GotRelative32,
            (true, macho::ARM64_RELOC_BRANCH26, true, _) => Kind::Branch26,
            (true, macho::ARM64_RELOC_PAGE21, true, _) => Kind::Page21,
            (true, macho::ARM64_RELOC_PAGEOFF12, false, _) => Kind::PageOffset12,
            (true, macho::ARM64_RELOC_GOT_LOAD_PAGE21, true, _) => Kind::GotPage21,
            (true, macho::ARM64_RELOC_GOT_LOAD_PAGEOFF12, false, _) => Kind::GotPageOffset12,
            _ => return None,
        },
        _ => return None,
    })
}

/// What a symbol of the object is in the image.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Place {
    Here(u64),
    There(External),
}

/// Lays out the patch that `object` is; `resolve` says which entry of the
/// running program's tables each symbol the object leaves undefined is,
/// and `table_name` names the patch's table of the slots it sets.
pub fn link(
    object: &[u8],
    resolve: &dyn Fn(&str) -> Result<External, String>,
    table_name: &str,
) -> Result<Vec<u8>, String> {
    let file = object::File::parse(object).map_err(|e| format!("the patch: {e}"))?;
    let arm64 = file.architecture() == object::Architecture::Aarch64;
    let macho = file.format() == object::BinaryFormat::MachO;
    let symbol_name = |name: &'_ str| -> String {
        match macho {
            true => name.strip_prefix('_').unwrap_or(name).to_string(),
            false => name.to_string(),
        }
    };
    // The sections that load: code first, then what is only read, then
    // the rest.
    let mut sections: Vec<object::Section> = file
        .sections()
        .filter(|s| {
            matches!(
                s.kind(),
                SectionKind::Text
                    | SectionKind::ReadOnlyData
                    | SectionKind::ReadOnlyDataWithRel
                    | SectionKind::ReadOnlyString
                    | SectionKind::Data
                    | SectionKind::UninitializedData
            )
        })
        .collect();
    sections.sort_by_key(|s| match s.kind() {
        SectionKind::Text => 0,
        SectionKind::Data | SectionKind::UninitializedData => 2,
        _ => 1,
    });
    let mut image: Vec<u8> = Vec::new();
    let mut placed: HashMap<object::SectionIndex, u64> = HashMap::new();
    for section in &sections {
        let start = image.len().next_multiple_of(section.align().max(1) as usize);
        image.resize(start, 0);
        match section.kind() {
            SectionKind::UninitializedData => image.resize(start + section.size() as usize, 0),
            _ => image.extend_from_slice(section.data().map_err(|e| e.to_string())?),
        }
        placed.insert(section.index(), start as u64);
    }
    // Where each symbol is; what the patch does not define, by its name,
    // until a relocation needs it.
    let mut places: HashMap<object::SymbolIndex, Result<Place, String>> = HashMap::new();
    let mut table = None;
    for symbol in file.symbols() {
        let name = symbol_name(symbol.name().unwrap_or_default());
        let place = match symbol.section_index() {
            Some(index) => match (placed.get(&index), file.section_by_index(index)) {
                (Some(&start), Ok(section)) => Ok(Place::Here(start + symbol.address() - section.address())),
                _ => continue,
            },
            None if symbol.is_undefined() && !name.is_empty() => Err(name.clone()),
            None => continue,
        };
        if name == table_name {
            table = place.clone().ok();
        }
        places.insert(symbol.index(), place);
    }
    let Some(Place::Here(table)) = table else { return Err(format!("the patch has no `{table_name}`")) };
    let mut resolved: HashMap<String, External> = HashMap::new();
    // The entries and the stubs that reach what is not in the patch, after
    // the sections.
    let mut relocations = Vec::new();
    for section in &sections {
        let start = placed[&section.index()];
        for (offset, relocation) in section.relocations() {
            let kind = kind(relocation.flags(), arm64)
                .ok_or_else(|| format!("the patch has a relocation tondo cannot apply: {:?}", relocation.flags()))?;
            let RelocationTarget::Symbol(symbol) = relocation.target() else {
                return Err("the patch has a relocation to a section, which tondo does not apply".into());
            };
            let place = match places.get(&symbol).ok_or("the patch refers to a symbol that is not in it")? {
                Ok(place) => *place,
                Err(name) => match resolved.get(name) {
                    Some(&external) => Place::There(external),
                    None => {
                        let external = resolve(name)?;
                        resolved.insert(name.clone(), external);
                        Place::There(external)
                    }
                },
            };
            let mut addend = relocation.addend();
            if relocation.has_implicit_addend() {
                let at = (start + offset) as usize;
                addend += match kind {
                    Kind::Absolute64 => i64::from_le_bytes(image[at..at + 8].try_into().expect("8 bytes")),
                    Kind::Relative32 | Kind::Branch32 | Kind::GotRelative32 => {
                        i32::from_le_bytes(image[at..at + 4].try_into().expect("4 bytes")) as i64
                    }
                    _ => 0,
                };
            }
            relocations.push((start + offset, kind, place, addend));
        }
    }
    let mut got: HashMap<Place, u64> = HashMap::new();
    let mut stubs: HashMap<External, u64> = HashMap::new();
    let mut words: Vec<u64> = Vec::new();
    let mut entries: Vec<(u64, External)> = Vec::new();
    for &(_, kind, place, _) in &relocations {
        if matches!(kind, Kind::GotRelative32 | Kind::GotPage21 | Kind::GotPageOffset12) && !got.contains_key(&place) {
            got.insert(place, 0);
        }
        if let (Kind::Branch32 | Kind::Branch26, Place::There(external)) = (kind, place) {
            got.entry(place).or_insert(0);
            stubs.entry(external).or_insert(0);
        }
    }
    image.resize(image.len().next_multiple_of(8), 0);
    let mut ordered: Vec<Place> = got.keys().copied().collect();
    ordered.sort_by_key(|p| format!("{p:?}"));
    for place in ordered {
        let at = image.len() as u64;
        got.insert(place, at);
        image.extend_from_slice(&[0; 8]);
        match place {
            Place::Here(target) => {
                image[at as usize..at as usize + 8].copy_from_slice(&target.to_le_bytes());
                words.push(at);
            }
            Place::There(external) => entries.push((at, external)),
        }
    }
    let mut ordered: Vec<External> = stubs.keys().copied().collect();
    ordered.sort_by_key(|e| (e.table, e.index));
    for external in ordered {
        let at = image.len().next_multiple_of(16) as u64;
        image.resize(at as usize, 0);
        stubs.insert(external, at);
        let slot = got[&Place::There(external)];
        image.extend_from_slice(&stub(arm64, at, slot));
    }
    for (at, kind, place, addend) in relocations {
        let target = match place {
            Place::Here(target) => target,
            Place::There(external) => match kind {
                Kind::Branch32 | Kind::Branch26 => stubs[&external],
                Kind::Absolute64 if addend == 0 => {
                    entries.push((at, external));
                    continue;
                }
                Kind::GotRelative32 | Kind::GotPage21 | Kind::GotPageOffset12 => 0,
                _ => {
                    return Err(format!(
                        "the patch refers to the running program in a way tondo cannot apply: {kind:?}"
                    ));
                }
            },
        };
        let entry = got.get(&place).copied().unwrap_or(0);
        let s = target as i64 + addend;
        let g = entry as i64 + addend;
        let p = at as i64;
        let field = at as usize;
        match kind {
            Kind::Absolute64 => {
                image[field..field + 8].copy_from_slice(&(s as u64).to_le_bytes());
                words.push(at);
            }
            Kind::Relative32 | Kind::Branch32 => put32(&mut image, field, s - p)?,
            Kind::GotRelative32 => put32(&mut image, field, g - p)?,
            Kind::Branch26 => {
                let delta = s - p;
                if delta % 4 != 0 || !(-(1 << 27)..1 << 27).contains(&delta) {
                    return Err("a call of the patch is out of range".into());
                }
                patch_instruction(&mut image, field, 0x03ff_ffff, ((delta >> 2) as u32) & 0x03ff_ffff);
            }
            Kind::Page21 | Kind::GotPage21 => {
                let value = if kind == Kind::Page21 { s } else { g };
                let pages = (value >> 12) - (p >> 12);
                let (lo, hi) = ((pages as u32) & 3, ((pages as u32) >> 2) & 0x7_ffff);
                patch_instruction(&mut image, field, 0x6000_0000 | 0x00ff_ffe0, (lo << 29) | (hi << 5));
            }
            Kind::PageOffset12 | Kind::GotPageOffset12 => {
                let value = if kind == Kind::PageOffset12 { s } else { g };
                let instruction = u32::from_le_bytes(image[field..field + 4].try_into().expect("4 bytes"));
                let scale = scale(instruction);
                let offset = (value as u64 & 0xfff) >> scale;
                patch_instruction(&mut image, field, 0xfff << 10, (offset as u32) << 10);
            }
        }
    }
    let mut out = b"TONDOPT1".to_vec();
    for value in [image.len() as u64, table, words.len() as u64, entries.len() as u64] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&image);
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    for (at, external) in entries {
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&external.table.to_le_bytes());
        out.extend_from_slice(&external.index.to_le_bytes());
    }
    Ok(out)
}

fn put32(image: &mut [u8], at: usize, value: i64) -> Result<(), String> {
    let value = i32::try_from(value).map_err(|_| "a reference of the patch is out of range".to_string())?;
    image[at..at + 4].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn patch_instruction(image: &mut [u8], at: usize, mask: u32, bits: u32) {
    let instruction = u32::from_le_bytes(image[at..at + 4].try_into().expect("4 bytes"));
    image[at..at + 4].copy_from_slice(&((instruction & !mask) | (bits & mask)).to_le_bytes());
}

/// How much an arm64 load or store scales its 12-bit offset: by the size
/// it moves; an `add` does not.
fn scale(instruction: u32) -> u32 {
    if instruction & 0x3b00_0000 != 0x3900_0000 {
        return 0;
    }
    let size = instruction >> 30;
    // A 128-bit vector load or store.
    if size == 0 && instruction & 0x0480_0000 == 0x0480_0000 { 4 } else { size }
}

/// A stub that jumps to what the entry at `slot` holds.
fn stub(arm64: bool, at: u64, slot: u64) -> Vec<u8> {
    if !arm64 {
        // jmp *slot(%rip)
        let delta = (slot as i64 - (at as i64 + 6)) as i32;
        return [&[0xff, 0x25][..], &delta.to_le_bytes()].concat();
    }
    // adrp x16, slot; ldr x16, [x16, :lo12:slot]; br x16
    let pages = ((slot >> 12) as i64 - (at >> 12) as i64) as u32;
    let adrp = 0x9000_0010 | ((pages & 3) << 29) | (((pages >> 2) & 0x7_ffff) << 5);
    let ldr = 0xf940_0210 | ((((slot & 0xfff) >> 3) as u32) << 10);
    let br = 0xd61f_0200u32;
    [adrp.to_le_bytes(), ldr.to_le_bytes(), br.to_le_bytes()].concat()
}
