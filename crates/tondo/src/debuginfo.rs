//! The DWARF of a debug build, which gdb and lldb read (toolchain 7.6): a
//! unit with each of the program's functions, its name and its code, and
//! the line each run of its code comes from, as the table of the trace
//! has them. Its addresses are the functions' symbols, which the linker
//! gives.

use cranelift_object::object;
use gimli::write::{
    Address, AttributeValue, DwarfUnit, EndianVec, LineProgram, LineString, Range, RangeList, Sections, Writer,
};
use gimli::{Encoding, Format, LineEncoding, LittleEndian};
use object::write::{Object, Relocation, SymbolId};
use std::collections::HashMap;

/// A function as its DWARF says it: its symbol, its name, the size of its
/// code, and where each run of its code starts.
pub(crate) struct Function {
    pub symbol: SymbolId,
    pub name: String,
    pub size: u32,
    pub rows: Vec<Row>,
}

/// Where a run of a function's code starts, the file, line and column it
/// comes from, and whether a debugger stops there for its line.
#[derive(Clone)]
pub(crate) struct Row {
    pub offset: u32,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub statement: bool,
}

/// A section's bytes, and where they hold the address of a function: its
/// offset, the function's index and what to add.
#[derive(Clone)]
struct Relocated {
    bytes: EndianVec<LittleEndian>,
    addresses: Vec<(usize, usize, i64)>,
}

impl Writer for Relocated {
    type Endian = LittleEndian;

    fn endian(&self) -> LittleEndian {
        LittleEndian
    }

    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn write(&mut self, bytes: &[u8]) -> gimli::write::Result<()> {
        self.bytes.write(bytes)
    }

    fn write_at(&mut self, offset: usize, bytes: &[u8]) -> gimli::write::Result<()> {
        self.bytes.write_at(offset, bytes)
    }

    fn write_address(&mut self, address: Address, size: u8) -> gimli::write::Result<()> {
        match address {
            Address::Constant(value) => self.write_udata(value, size),
            Address::Symbol { symbol, addend } => {
                self.addresses.push((self.len(), symbol, addend));
                self.write_udata(0, size)
            }
        }
    }
}

/// Adds the DWARF of `functions` to the object, as the unit `name`.
pub(crate) fn write(object: &mut Object<'static>, name: &str, functions: &[Function]) -> Result<(), String> {
    let encoding = Encoding { format: Format::Dwarf32, version: 4, address_size: 8 };
    let mut dwarf = DwarfUnit::new(encoding);
    let directory = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
    let mut lines = LineProgram::new(
        encoding,
        LineEncoding::default(),
        LineString::String(directory.clone().into_bytes()),
        None,
        LineString::String(name.as_bytes().to_vec()),
        None,
    );
    let mut files = HashMap::new();
    let mut ranges = Vec::new();
    let root = dwarf.unit.root();
    for (index, function) in functions.iter().enumerate().filter(|(_, f)| !f.rows.is_empty()) {
        let start = Address::Symbol { symbol: index, addend: 0 };
        ranges.push(Range::StartLength { begin: start, length: function.size as u64 });
        lines.begin_sequence(Some(start));
        // The prologue ends where the first statement's code starts, which
        // a breakpoint on the function's first line stops at.
        let mut prologue = true;
        for at in &function.rows {
            let file = *files.entry(at.file.clone()).or_insert_with(|| {
                let directory = lines.default_directory();
                lines.add_file(LineString::String(at.file.clone().into_bytes()), directory, None)
            });
            let row = lines.row();
            (row.address_offset, row.file, row.line, row.column) =
                (at.offset as u64, file, at.line as u64, at.column as u64);
            row.is_statement = at.statement;
            row.prologue_end = prologue && at.statement && at.offset > 0;
            prologue &= !row.prologue_end;
            lines.generate_row();
        }
        lines.end_sequence(function.size as u64);
        let entry = dwarf.unit.add(root, gimli::DW_TAG_subprogram);
        let entry = dwarf.unit.get_mut(entry);
        entry.set(gimli::DW_AT_name, AttributeValue::String(function.name.clone().into_bytes()));
        entry.set(gimli::DW_AT_external, AttributeValue::Flag(true));
        entry.set(gimli::DW_AT_low_pc, AttributeValue::Address(start));
        entry.set(gimli::DW_AT_high_pc, AttributeValue::Udata(function.size as u64));
    }
    dwarf.unit.line_program = lines;
    let ranges = dwarf.unit.ranges.add(RangeList(ranges));
    let unit = dwarf.unit.get_mut(root);
    let producer = concat!("tondo ", env!("CARGO_PKG_VERSION"));
    unit.set(gimli::DW_AT_producer, AttributeValue::String(producer.as_bytes().to_vec()));
    // DWARF has no code for Tondo: gdb reads names and lines as C's.
    unit.set(gimli::DW_AT_language, AttributeValue::Language(gimli::DW_LANG_C99));
    unit.set(gimli::DW_AT_name, AttributeValue::String(name.as_bytes().to_vec()));
    unit.set(gimli::DW_AT_comp_dir, AttributeValue::String(directory.into_bytes()));
    unit.set(gimli::DW_AT_low_pc, AttributeValue::Address(Address::Constant(0)));
    unit.set(gimli::DW_AT_ranges, AttributeValue::RangeListRef(ranges));
    let mut sections = Sections::new(Relocated { bytes: EndianVec::new(LittleEndian), addresses: Vec::new() });
    dwarf.write(&mut sections).map_err(|e| format!("the debug information: {e}"))?;
    sections.for_each(|id, section| -> Result<(), String> {
        if section.len() == 0 {
            return Ok(());
        }
        let at = object.add_section(Vec::new(), id.name().as_bytes().to_vec(), object::SectionKind::Debug);
        object.append_section_data(at, section.bytes.slice(), 1);
        for &(offset, index, addend) in &section.addresses {
            let flags = object::RelocationFlags::Generic {
                kind: object::RelocationKind::Absolute,
                encoding: object::RelocationEncoding::Generic,
                size: 64,
            };
            let relocation = Relocation { offset: offset as u64, symbol: functions[index].symbol, addend, flags };
            object.add_relocation(at, relocation).map_err(|e| e.to_string())?;
        }
        Ok(())
    })
}
