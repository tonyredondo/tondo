//! The C declarations of a library from its debug information (DWARF),
//! for `tondo bind` when there is no header: the functions it exports, with
//! the types their debug information gives them.
//!
//! A shared library holds its debug information, or a macOS `.dSYM`
//! beside it does; a static archive holds it in its objects, whose
//! relocations are applied while reading. Debug information has no
//! macros, so a module made from it has functions and types but no
//! constants other than those of enums.

// The constants of DWARF keep the names its specification gives them.
#![allow(non_upper_case_globals)]

use super::{C, CEnum, CField, CFunction, CType, Function, Kind, Record, Typedef};
use gimli::constants::*;
use gimli::{AttributeValue, EndianSlice, RelocateReader, RunTimeEndian, UnitOffset};
use object::read::RelocationMap;
use object::{Object, ObjectSection, ObjectSymbol};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The file of the declarations a module binds, and that of the types
/// they reach, which are bound only where needed.
const BOUND: usize = 0;
const REACHED: usize = 1;

/// The C declarations of the library at `path`, or why there are none.
pub(super) fn library(path: &Path) -> Result<C, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut objects: Vec<Cow<[u8]>> = Vec::new();
    if let Ok(archive) = object::read::archive::ArchiveFile::parse(&*bytes) {
        for member in archive.members() {
            let member = member.map_err(|e| format!("{}: {e}", path.display()))?;
            objects.push(Cow::Borrowed(member.data(&*bytes).map_err(|e| format!("{}: {e}", path.display()))?));
        }
    } else {
        objects.push(Cow::Borrowed(&bytes));
    }
    // On macOS the debug information of a shared library is in its `.dSYM`.
    let file_name = path.file_name().unwrap_or_default();
    let dsym = path.with_file_name(format!("{}.dSYM", file_name.to_string_lossy()));
    let dsym = dsym.join("Contents/Resources/DWARF").join(file_name);
    let dsym_bytes = std::fs::read(&dsym).ok();

    let mut exported = HashSet::new();
    for data in &objects {
        let Ok(file) = object::File::parse(&**data) else { continue };
        exported.extend(exports(&file));
    }
    let mut reader = Reader { c: C::default(), exported, done: HashSet::new() };
    reader.c.targets.insert(BOUND);
    reader.c.complete_records = true;
    let debug: Vec<&[u8]> = match &dsym_bytes {
        Some(dsym) => vec![dsym],
        None => objects.iter().map(|o| &**o).collect(),
    };
    for data in debug {
        let Ok(file) = object::File::parse(data) else { continue };
        reader.object(&file).map_err(|e| format!("{}: unreadable debug information: {e}", path.display()))?;
    }
    if reader.c.functions.is_empty() {
        return Err(format!("{} has no debug information about the functions it exports", path.display()));
    }
    Ok(reader.c)
}

/// The functions an object defines for others.
fn exports(file: &object::File) -> Vec<String> {
    let mach_o = file.format() == object::BinaryFormat::MachO;
    file.symbols()
        .chain(file.dynamic_symbols())
        .filter(|s| s.is_definition() && s.is_global() && s.kind() == object::SymbolKind::Text)
        .filter_map(|s| s.name().ok())
        .map(|name| if mach_o { name.strip_prefix('_').unwrap_or(name) } else { name }.to_string())
        .collect()
}

/// Relocations of a section of a relocatable object, applied while its
/// debug information is read.
#[derive(Clone, Copy, Debug)]
struct Relocations<'a>(&'a RelocationMap);

impl gimli::read::Relocate<usize> for Relocations<'_> {
    fn relocate_address(&self, offset: usize, value: u64) -> gimli::Result<u64> {
        Ok(self.0.relocate(offset as u64, value))
    }

    fn relocate_offset(&self, offset: usize, value: usize) -> gimli::Result<usize> {
        <usize as gimli::ReaderOffset>::from_u64(self.0.relocate(offset as u64, value as u64))
    }
}

type R<'a> = RelocateReader<EndianSlice<'a, RunTimeEndian>, Relocations<'a>>;

struct Reader {
    c: C,
    exported: HashSet<String>,
    /// Functions already read, from another object.
    done: HashSet<String>,
}

impl Reader {
    fn object(&mut self, file: &object::File) -> gimli::Result<()> {
        let endian = if file.is_little_endian() { RunTimeEndian::Little } else { RunTimeEndian::Big };
        let mut sections: HashMap<gimli::SectionId, (Cow<[u8]>, RelocationMap)> = HashMap::new();
        for id in [
            gimli::SectionId::DebugInfo,
            gimli::SectionId::DebugAbbrev,
            gimli::SectionId::DebugStr,
            gimli::SectionId::DebugLineStr,
            gimli::SectionId::DebugStrOffsets,
            gimli::SectionId::DebugAddr,
            gimli::SectionId::DebugTypes,
            gimli::SectionId::DebugLine,
            gimli::SectionId::DebugRanges,
            gimli::SectionId::DebugRngLists,
            gimli::SectionId::DebugLoc,
            gimli::SectionId::DebugLocLists,
        ] {
            if let Some(section) = file.section_by_name(id.name())
                && let Ok(data) = section.uncompressed_data()
            {
                let relocations = section.relocation_map().unwrap_or_default();
                sections.insert(id, (data, relocations));
            }
        }
        if !sections.contains_key(&gimli::SectionId::DebugInfo) {
            return Ok(());
        }
        let empty = RelocationMap::default();
        let dwarf = gimli::Dwarf::load(|id| -> gimli::Result<R> {
            Ok(match sections.get(&id) {
                Some((data, relocations)) => {
                    RelocateReader::new(EndianSlice::new(data, endian), Relocations(relocations))
                }
                None => RelocateReader::new(EndianSlice::new(&[], endian), Relocations(&empty)),
            })
        })?;
        let mut units = dwarf.units();
        while let Some(header) = units.next()? {
            let unit = dwarf.unit(header)?;
            let mut unit_reader = UnitReader { reader: self, dwarf: &dwarf, unit: &unit, types: HashMap::new() };
            unit_reader.functions()?;
        }
        Ok(())
    }
}

struct UnitReader<'r, 'a, 'u> {
    reader: &'r mut Reader,
    dwarf: &'u gimli::Dwarf<R<'a>>,
    unit: &'u gimli::Unit<R<'a>>,
    types: HashMap<UnitOffset, CType>,
}

impl UnitReader<'_, '_, '_> {
    fn name(&self, entry: &gimli::DebuggingInformationEntry<R>) -> Option<String> {
        let value = entry.attr_value(DW_AT_name)?;
        let name = self.dwarf.attr_string(self.unit, value).ok()?;
        gimli::Reader::to_string_lossy(&name).ok().map(|n| n.into_owned())
    }

    fn number(entry: &gimli::DebuggingInformationEntry<R>, attribute: gimli::DwAt) -> Option<u64> {
        entry.attr(attribute).and_then(|a| a.udata_value())
    }

    /// The exported functions of the unit.
    fn functions(&mut self) -> gimli::Result<()> {
        let mut offsets = Vec::new();
        let mut cursor = self.unit.entries();
        while let Some(entry) = cursor.next_dfs()? {
            if entry.depth() == 1 && entry.tag() == DW_TAG_subprogram {
                offsets.push(entry.offset());
            }
        }
        for offset in offsets {
            let entry = self.unit.entry(offset)?;
            let Some(name) = self.name(&entry) else { continue };
            if !self.reader.exported.contains(&name) || !self.reader.done.insert(name.clone()) {
                continue;
            }
            let ty = self.function(offset, &entry)?;
            self.reader.c.functions.push(Function { name, ty, symbol: None, file: BOUND, inline: false });
            self.reader.c.order.push((Kind::Function, self.reader.c.functions.len() - 1));
        }
        Ok(())
    }

    /// The parameters and result of a subprogram or subroutine type.
    fn function(
        &mut self,
        offset: UnitOffset,
        entry: &gimli::DebuggingInformationEntry<R>,
    ) -> gimli::Result<CFunction> {
        let result = self.type_at(entry)?;
        let (mut params, mut variadic) = (Vec::new(), false);
        let mut children = Vec::new();
        {
            let mut tree = self.unit.entries_tree(Some(offset))?;
            let mut nodes = tree.root()?.children();
            while let Some(node) = nodes.next()? {
                children.push(node.entry().offset());
            }
        }
        for child in children {
            let child = self.unit.entry(child)?;
            match child.tag() {
                DW_TAG_formal_parameter => {
                    let ty = self.type_at(&child)?;
                    params.push((self.name(&child), ty));
                }
                DW_TAG_unspecified_parameters => variadic = true,
                _ => {}
            }
        }
        Ok(CFunction { result, params, variadic })
    }

    /// The type an entry's `DW_AT_type` names; `void` without one.
    fn type_at(&mut self, entry: &gimli::DebuggingInformationEntry<R>) -> gimli::Result<CType> {
        match entry.attr_value(DW_AT_type) {
            Some(AttributeValue::UnitRef(offset)) => self.ty(offset),
            Some(_) => Ok(CType::Unsupported("a type of another unit")),
            None => Ok(CType::Void),
        }
    }

    fn ty(&mut self, offset: UnitOffset) -> gimli::Result<CType> {
        if let Some(ty) = self.types.get(&offset) {
            return Ok(ty.clone());
        }
        let entry = self.unit.entry(offset)?;
        let size = Self::number(&entry, DW_AT_byte_size).unwrap_or(0);
        let ty = match entry.tag() {
            DW_TAG_base_type => {
                let name = self.name(&entry).unwrap_or_default();
                let encoding = match entry.attr_value(DW_AT_encoding) {
                    Some(AttributeValue::Encoding(encoding)) => encoding,
                    _ => DW_ATE_signed,
                };
                base(encoding, size, &name)
            }
            DW_TAG_const_type | DW_TAG_volatile_type | DW_TAG_restrict_type | DW_TAG_atomic_type => {
                self.type_at(&entry)?
            }
            DW_TAG_pointer_type => {
                let constant = match entry.attr_value(DW_AT_type) {
                    Some(AttributeValue::UnitRef(target)) => self.unit.entry(target)?.tag() == DW_TAG_const_type,
                    _ => false,
                };
                CType::Pointer(Box::new(self.type_at(&entry)?), constant)
            }
            DW_TAG_typedef => {
                let name = self.name(&entry).unwrap_or_default();
                // The typedef takes its name before what it names is read,
                // since that may refer to it.
                self.types.insert(offset, CType::Named(name.clone()));
                let ty = self.type_at(&entry)?;
                let c = &mut self.reader.c;
                if !c.typedef_names.contains_key(&name) {
                    // The library's own names are bound; those of the system
                    // start with `_` or are known.
                    let file = if name.starts_with('_') { REACHED } else { BOUND };
                    c.typedefs.push(Typedef { name: name.clone(), ty, file });
                    c.typedef_names.insert(name.clone(), c.typedefs.len() - 1);
                    if file == BOUND {
                        c.order.push((Kind::Typedef, c.typedefs.len() - 1));
                    }
                }
                CType::Named(name)
            }
            DW_TAG_structure_type | DW_TAG_union_type => self.record(offset, &entry)?,
            DW_TAG_enumeration_type => self.enumeration(offset, &entry)?,
            DW_TAG_array_type => {
                let element = self.type_at(&entry)?;
                let mut counts = Vec::new();
                let mut tree = self.unit.entries_tree(Some(offset))?;
                let mut nodes = tree.root()?.children();
                while let Some(node) = nodes.next()? {
                    let range = node.entry();
                    if range.tag() == DW_TAG_subrange_type {
                        let count = Self::number(range, DW_AT_count)
                            .or_else(|| Self::number(range, DW_AT_upper_bound).map(|bound| bound + 1));
                        counts.push(count);
                    }
                }
                counts.into_iter().rev().fold(element, |ty, count| CType::Array(Box::new(ty), count))
            }
            DW_TAG_subroutine_type => CType::Function(Box::new(self.function(offset, &entry)?)),
            _ => CType::Unsupported("a type C debug information describes in another way"),
        };
        self.types.insert(offset, ty.clone());
        Ok(ty)
    }

    fn record(&mut self, offset: UnitOffset, entry: &gimli::DebuggingInformationEntry<R>) -> gimli::Result<CType> {
        let union = entry.tag() == DW_TAG_union_type;
        let tag = self.name(entry);
        let declaration = entry.attr_value(DW_AT_declaration).is_some();
        let c = &mut self.reader.c;
        // The same struct in each unit that uses it is one type.
        let existing =
            tag.as_ref().and_then(|t| c.records.iter().position(|r| r.tag.as_ref() == Some(t) && r.union == union));
        if let Some(id) = existing
            && (declaration || c.records[id].fields.is_some())
        {
            return Ok(CType::Record(id));
        }
        let id = match existing {
            Some(id) => id,
            None => {
                let file = if tag.as_ref().is_some_and(|t| t.starts_with('_')) { REACHED } else { BOUND };
                c.records.push(Record { tag, union, fields: None, file });
                if file == BOUND {
                    c.order.push((Kind::Record, c.records.len() - 1));
                }
                c.records.len() - 1
            }
        };
        self.types.insert(offset, CType::Record(id));
        if declaration {
            return Ok(CType::Record(id));
        }
        let mut members = Vec::new();
        {
            let mut tree = self.unit.entries_tree(Some(offset))?;
            let mut nodes = tree.root()?.children();
            while let Some(node) = nodes.next()? {
                if node.entry().tag() == DW_TAG_member {
                    members.push(node.entry().offset());
                }
            }
        }
        let mut fields = Vec::new();
        for member in members {
            let member = self.unit.entry(member)?;
            let ty = self.type_at(&member)?;
            let bits = Self::number(&member, DW_AT_bit_size);
            fields.push(CField { name: self.name(&member), ty, bits });
        }
        self.reader.c.records[id].fields = Some(fields);
        Ok(CType::Record(id))
    }

    fn enumeration(&mut self, offset: UnitOffset, entry: &gimli::DebuggingInformationEntry<R>) -> gimli::Result<CType> {
        let tag = self.name(entry);
        if let Some(id) = tag.as_ref().and_then(|t| self.reader.c.enums.iter().position(|e| e.tag.as_ref() == Some(t)))
        {
            return Ok(CType::Enum(id));
        }
        let mut items = Vec::new();
        let mut tree = self.unit.entries_tree(Some(offset))?;
        let mut nodes = tree.root()?.children();
        while let Some(node) = nodes.next()? {
            let item = node.entry();
            if item.tag() != DW_TAG_enumerator {
                continue;
            }
            let value = match item.attr_value(DW_AT_const_value) {
                Some(AttributeValue::Sdata(v)) => Some(v as i128),
                Some(AttributeValue::Udata(v)) => Some(v as i128),
                Some(AttributeValue::Data1(v)) => Some(v as i128),
                Some(AttributeValue::Data2(v)) => Some(v as i128),
                Some(AttributeValue::Data4(v)) => Some(v as i128),
                Some(AttributeValue::Data8(v)) => Some(v as i128),
                _ => None,
            };
            if let Some(name) = self.name(item) {
                items.push((name, value));
            }
        }
        let c = &mut self.reader.c;
        for (name, value) in &items {
            if let Some(value) = value {
                c.constants.insert(name.clone(), *value);
            }
        }
        let file = if tag.as_ref().is_some_and(|t| t.starts_with('_')) { REACHED } else { BOUND };
        c.enums.push(CEnum { tag, items, file });
        c.order.push((Kind::Enum, c.enums.len() - 1));
        Ok(CType::Enum(c.enums.len() - 1))
    }
}

/// The C type of a base type of debug information.
fn base(encoding: gimli::DwAte, size: u64, name: &str) -> CType {
    let long_long = name.contains("long long");
    match (encoding, size) {
        (DW_ATE_boolean, _) => CType::Bool,
        (DW_ATE_float, 4) => CType::Float,
        (DW_ATE_float, 8) => CType::Double,
        (DW_ATE_float, _) => CType::Unsupported("long double"),
        (DW_ATE_signed_char | DW_ATE_unsigned_char, 1) if name == "char" => CType::Char,
        (DW_ATE_signed_char, 1) | (DW_ATE_signed, 1) => CType::SChar,
        (DW_ATE_unsigned_char, 1) | (DW_ATE_unsigned, 1) => CType::UChar,
        (DW_ATE_signed, 2) => CType::Short,
        (DW_ATE_unsigned, 2) => CType::UShort,
        (DW_ATE_signed, 4) => CType::Int,
        (DW_ATE_unsigned, 4) => CType::UInt,
        (DW_ATE_signed, 8) if long_long => CType::LongLong,
        (DW_ATE_signed, 8) => CType::Long,
        (DW_ATE_unsigned, 8) if long_long => CType::ULongLong,
        (DW_ATE_unsigned, 8) => CType::ULong,
        (DW_ATE_signed | DW_ATE_unsigned, 16) => CType::Unsupported("__int128"),
        _ => CType::Unsupported("a base type of another size"),
    }
}
