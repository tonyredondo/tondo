//! Links Mach-O executables for macOS. Sections of objects built with
//! subsections are cut into atoms at their symbols, and only the atoms the
//! entry reaches stay. What the system's libraries define is bound when
//! the program starts by name, looked up in every library it loads (flat
//! lookup), so linking needs no SDK. The executable is position
//! independent, as macOS requires: every pointer it holds is rebased when
//! it loads. `__unwind_info` indexes how to unwind each function, and an
//! ad-hoc signature, which arm64 requires, covers the file.

use object::read::macho::{MachHeader as _, MachOFile64, Nlist as _, Section as _};
use raw::*;

/// The values of the Mach-O constants the linker reads and writes.
mod raw {
    pub const SECTION_TYPE: u32 = 0xff;
    pub const S_REGULAR: u8 = 0x0;
    pub const S_ZEROFILL: u8 = 0x1;
    pub const S_CSTRING_LITERALS: u8 = 0x2;
    pub const S_4BYTE_LITERALS: u8 = 0x3;
    pub const S_8BYTE_LITERALS: u8 = 0x4;
    pub const S_NON_LAZY_SYMBOL_POINTERS: u8 = 0x6;
    pub const S_SYMBOL_STUBS: u8 = 0x8;
    pub const S_MOD_INIT_FUNC_POINTERS: u8 = 0x9;
    pub const S_COALESCED: u8 = 0xb;
    pub const S_GB_ZEROFILL: u8 = 0xc;
    pub const S_16BYTE_LITERALS: u8 = 0xe;
    pub const S_THREAD_LOCAL_REGULAR: u8 = 0x11;
    pub const S_THREAD_LOCAL_ZEROFILL: u8 = 0x12;
    pub const S_THREAD_LOCAL_VARIABLES: u8 = 0x13;
    pub const S_THREAD_LOCAL_VARIABLE_POINTERS: u8 = 0x14;
    pub const S_ATTR_PURE_INSTRUCTIONS: u32 = 0x8000_0000;
    pub const S_ATTR_NO_TOC: u32 = 0x4000_0000;
    pub const S_ATTR_STRIP_STATIC_SYMS: u32 = 0x2000_0000;
    pub const S_ATTR_NO_DEAD_STRIP: u32 = 0x1000_0000;
    pub const S_ATTR_LIVE_SUPPORT: u32 = 0x0800_0000;
    pub const S_ATTR_SOME_INSTRUCTIONS: u32 = 0x400;
    pub const N_STAB: u8 = 0xe0;
    pub const N_TYPE: u8 = 0x0e;
    pub const N_EXT: u8 = 0x01;
    pub const N_UNDF: u8 = 0x0;
    pub const N_ABS: u8 = 0x2;
    pub const N_SECT: u8 = 0xe;
    pub const NO_SECT: u8 = 0;
    pub const N_NO_DEAD_STRIP: u16 = 0x20;
    pub const N_WEAK_REF: u16 = 0x40;
    pub const N_WEAK_DEF: u16 = 0x80;
    pub const DYNAMIC_LOOKUP_ORDINAL: u8 = 0xfe;
    pub const MH_MAGIC_64: u32 = 0xfeed_facf;
    pub const MH_EXECUTE: u32 = 0x2;
    pub const MH_DYLDLINK: u32 = 0x4;
    pub const MH_TWOLEVEL: u32 = 0x80;
    pub const MH_SUBSECTIONS_VIA_SYMBOLS: u32 = 0x2000;
    pub const MH_PIE: u32 = 0x20_0000;
    pub const MH_HAS_TLV_DESCRIPTORS: u32 = 0x80_0000;
    pub const CPU_TYPE_X86_64: u32 = 0x0100_0007;
    pub const CPU_TYPE_ARM64: u32 = 0x0100_000c;
    pub const CPU_SUBTYPE_X86_64_ALL: u32 = 3;
    pub const CPU_SUBTYPE_ARM64_ALL: u32 = 0;
    pub const LC_SYMTAB: u32 = 0x2;
    pub const LC_DYSYMTAB: u32 = 0xb;
    pub const LC_LOAD_DYLIB: u32 = 0xc;
    pub const LC_LOAD_DYLINKER: u32 = 0xe;
    pub const LC_SEGMENT_64: u32 = 0x19;
    pub const LC_UUID: u32 = 0x1b;
    pub const LC_CODE_SIGNATURE: u32 = 0x1d;
    pub const LC_RPATH: u32 = 0x8000_001c;
    pub const LC_DYLD_INFO_ONLY: u32 = 0x8000_0022;
    pub const LC_MAIN: u32 = 0x8000_0028;
    pub const LC_BUILD_VERSION: u32 = 0x32;
    pub const SG_READ_ONLY: u32 = 0x10;
    pub const PLATFORM_MACOS: u32 = 1;
    pub const INDIRECT_SYMBOL_LOCAL: u32 = 0x8000_0000;
    pub const REBASE_TYPE_POINTER: u8 = 1;
    pub const REBASE_OPCODE_DONE: u8 = 0x00;
    pub const REBASE_OPCODE_SET_TYPE_IMM: u8 = 0x10;
    pub const REBASE_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB: u8 = 0x20;
    pub const REBASE_OPCODE_DO_REBASE_IMM_TIMES: u8 = 0x50;
    pub const BIND_TYPE_POINTER: u8 = 1;
    pub const BIND_SPECIAL_DYLIB_FLAT_LOOKUP: i8 = -2;
    pub const BIND_SYMBOL_FLAGS_WEAK_IMPORT: u8 = 0x1;
    pub const BIND_IMMEDIATE_MASK: u8 = 0x0f;
    pub const BIND_OPCODE_DONE: u8 = 0x00;
    pub const BIND_OPCODE_SET_DYLIB_SPECIAL_IMM: u8 = 0x30;
    pub const BIND_OPCODE_SET_SYMBOL_TRAILING_FLAGS_IMM: u8 = 0x40;
    pub const BIND_OPCODE_SET_TYPE_IMM: u8 = 0x50;
    pub const BIND_OPCODE_SET_ADDEND_SLEB: u8 = 0x60;
    pub const BIND_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB: u8 = 0x70;
    pub const BIND_OPCODE_DO_BIND: u8 = 0x90;
    pub const ARM64_RELOC_BRANCH26: u8 = 2;
    pub const ARM64_RELOC_SUBTRACTOR: u8 = 1;
    pub const ARM64_RELOC_PAGE21: u8 = 3;
    pub const ARM64_RELOC_PAGEOFF12: u8 = 4;
    pub const ARM64_RELOC_GOT_LOAD_PAGE21: u8 = 5;
    pub const ARM64_RELOC_GOT_LOAD_PAGEOFF12: u8 = 6;
    pub const ARM64_RELOC_POINTER_TO_GOT: u8 = 7;
    pub const ARM64_RELOC_TLVP_LOAD_PAGE21: u8 = 8;
    pub const ARM64_RELOC_TLVP_LOAD_PAGEOFF12: u8 = 9;
    pub const ARM64_RELOC_ADDEND: u8 = 10;
    pub const X86_64_RELOC_SIGNED: u8 = 1;
    pub const X86_64_RELOC_BRANCH: u8 = 2;
    pub const X86_64_RELOC_GOT_LOAD: u8 = 3;
    pub const X86_64_RELOC_GOT: u8 = 4;
    pub const X86_64_RELOC_SUBTRACTOR: u8 = 5;
    pub const X86_64_RELOC_SIGNED_1: u8 = 6;
    pub const X86_64_RELOC_SIGNED_2: u8 = 7;
    pub const X86_64_RELOC_SIGNED_4: u8 = 8;
    pub const X86_64_RELOC_TLV: u8 = 9;
}
use object::{Endianness, Object, ObjectSection};
use std::collections::{HashMap, HashSet};

#[derive(Copy, Clone, PartialEq)]
pub enum Arch {
    Arm64,
    X86_64,
}

/// What the executable links.
pub struct Inputs<'a> {
    pub arch: Arch,
    pub objects: Vec<(String, &'a [u8])>,
    pub archives: Vec<(String, &'a [u8])>,
    /// The install names of the libraries it loads, the system's first.
    pub dylibs: Vec<String>,
    pub rpaths: Vec<String>,
    pub entry: String,
    pub strip: bool,
    /// The file's name, which the signature identifies it by.
    pub identifier: String,
}

const PAGE: u64 = 0x4000;
const BASE: u64 = 0x1_0000_0000;

/// A piece of an input section that stays or goes as a whole.
struct Atom {
    section: usize,
    /// Its range in the object's address space.
    start: u64,
    end: u64,
    live: bool,
    /// Its output section and offset there.
    place: Option<(usize, u64)>,
}

struct Sect<'a> {
    segname: Vec<u8>,
    sectname: Vec<u8>,
    addr: u64,
    size: u64,
    align: u64,
    kind: u8,
    attributes: u32,
    data: &'a [u8],
    relocations: Vec<Reloc>,
    /// Its atoms, in order, by index in the input's.
    atoms: Vec<usize>,
}

#[derive(Copy, Clone, Debug)]
struct Reloc {
    offset: u64,
    kind: u8,
    pcrel: bool,
    length: u8,
    external: bool,
    symbol: u32,
    addend: i64,
    /// The symbol a `SUBTRACTOR` pair subtracts.
    minus: Option<(bool, u32)>,
}

#[derive(Copy, Clone)]
struct Sym {
    sect: u8,
    value: u64,
    global: Option<usize>,
    no_dead_strip: bool,
}

struct Input<'a> {
    name: String,
    sections: Vec<Sect<'a>>,
    symbols: Vec<Sym>,
    atoms: Vec<Atom>,
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum Def {
    Undefined,
    Atom {
        input: u32,
        atom: u32,
        offset: u64,
    },
    Absolute(u64),
    Common {
        size: u64,
        align: u64,
    },
    /// The address of the Mach header.
    Header,
}

struct Global<'a> {
    name: &'a [u8],
    def: Def,
    weak: bool,
    required: bool,
    common: u64,
}

/// What a relocation refers to.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Target {
    Global(usize),
    /// An address in an input's address space, inside one of its sections.
    Local {
        input: u32,
        section: u32,
        address: u64,
    },
    Absolute(u64),
}

/// The output sections, in the order of their segments.
#[derive(Clone)]
struct OutSection {
    segname: &'static str,
    sectname: Vec<u8>,
    kind: u8,
    attributes: u32,
    align: u64,
    size: u64,
    addr: u64,
    offset: u64,
}

const TEXT: &str = "__TEXT";
const DATA_CONST: &str = "__DATA_CONST";
const DATA: &str = "__DATA";

struct Linker<'a> {
    arch: Arch,
    inputs: Vec<Input<'a>>,
    globals: Vec<Global<'a>>,
    names: HashMap<&'a [u8], usize>,
    outs: Vec<OutSection>,
    out_index: HashMap<(&'static str, Vec<u8>), usize>,
    got: Vec<Target>,
    slots: HashMap<Target, u32>,
    /// Pointers to thread-local descriptors, which TLV accesses load.
    tlv: Vec<Target>,
    tlv_slots: HashMap<Target, u32>,
    stubs: Vec<usize>,
    stub_of: HashMap<usize, u32>,
    imports: Vec<usize>,
    import_of: HashMap<usize, u32>,
    /// What each function's unwind descriptions refer to: personality,
    /// LSDA and what its `__eh_frame` records use.
    unwind_refs: HashMap<(u32, usize), Vec<Target>>,
}

fn error(input: &str, message: impl std::fmt::Display) -> String {
    format!("linking {input}: {message}")
}

fn name16(bytes: &[u8; 16]) -> Vec<u8> {
    bytes.iter().copied().take_while(|&b| b != 0).collect()
}

/// Links the executable, whose bytes it gives.
pub fn link(inputs: Inputs) -> Result<Vec<u8>, String> {
    let mut linker = Linker {
        arch: inputs.arch,
        inputs: Vec::new(),
        globals: Vec::new(),
        names: HashMap::new(),
        outs: Vec::new(),
        out_index: HashMap::new(),
        got: Vec::new(),
        slots: HashMap::new(),
        tlv: Vec::new(),
        tlv_slots: HashMap::new(),
        stubs: Vec::new(),
        stub_of: HashMap::new(),
        imports: Vec::new(),
        import_of: HashMap::new(),
        unwind_refs: HashMap::new(),
    };
    let entry = linker.intern(inputs.entry.as_bytes());
    linker.globals[entry].required = true;
    for (name, data) in &inputs.objects {
        linker.add(name.clone(), data)?;
    }
    linker.archives(&inputs.archives)?;
    for name in [b"__mh_execute_header".as_slice(), b"___dso_handle"] {
        if let Some(&id) = linker.names.get(name)
            && linker.globals[id].def == Def::Undefined
        {
            linker.globals[id].def = Def::Header;
        }
    }
    linker.unwind_references();
    linker.collect(entry);
    linker.scan()?;
    let image = linker.layout_and_write(&inputs, entry)?;
    Ok(image)
}

impl<'a> Linker<'a> {
    fn intern(&mut self, name: &'a [u8]) -> usize {
        if let Some(&id) = self.names.get(name) {
            return id;
        }
        self.globals.push(Global { name, def: Def::Undefined, weak: false, required: false, common: 0 });
        self.names.insert(name, self.globals.len() - 1);
        self.globals.len() - 1
    }

    /// Adds an object: its sections cut into atoms, and its symbols.
    fn add(&mut self, name: String, data: &'a [u8]) -> Result<Vec<usize>, String> {
        let file = MachOFile64::<Endianness>::parse(data).map_err(|e| error(&name, e))?;
        let endian = file.endian();
        let cpu = file.macho_header().cputype(endian).0;
        let wanted = match self.arch {
            Arch::Arm64 => CPU_TYPE_ARM64,
            Arch::X86_64 => CPU_TYPE_X86_64,
        };
        if cpu != wanted {
            return Err(error(&name, "it is for another architecture"));
        }
        let subsections = file.macho_header().flags(endian).0 & MH_SUBSECTIONS_VIA_SYMBOLS != 0;
        let index = self.inputs.len() as u32;
        let mut sections = Vec::new();
        for section in file.sections() {
            let header = section.macho_section();
            let flags = header.flags(endian).0;
            let kind = flags & SECTION_TYPE;
            let raw = header.relocations(endian, data).map_err(|e| error(&name, e))?;
            let mut relocations = Vec::new();
            let mut addend = 0i64;
            let mut minus = None;
            for r in raw {
                let info = r.info(endian);
                match (self.arch, info.r_type.0) {
                    (Arch::Arm64, ARM64_RELOC_ADDEND) => {
                        addend = ((info.r_symbolnum as i64) << 40) >> 40;
                        continue;
                    }
                    (Arch::Arm64, ARM64_RELOC_SUBTRACTOR) | (Arch::X86_64, X86_64_RELOC_SUBTRACTOR) => {
                        minus = Some((info.r_extern, info.r_symbolnum));
                        continue;
                    }
                    _ => {}
                }
                relocations.push(Reloc {
                    offset: info.r_address as u64,
                    kind: info.r_type.0,
                    pcrel: info.r_pcrel,
                    length: info.r_length,
                    external: info.r_extern,
                    symbol: info.r_symbolnum,
                    addend: std::mem::take(&mut addend),
                    minus: minus.take(),
                });
            }
            relocations.sort_by_key(|r| r.offset);
            let zerofill = matches!(kind as u8, S_ZEROFILL | S_GB_ZEROFILL | S_THREAD_LOCAL_ZEROFILL);
            sections.push(Sect {
                segname: name16(&header.segname),
                sectname: name16(&header.sectname),
                addr: header.addr(endian),
                size: header.size(endian),
                align: 1u64 << header.align(endian),
                kind: kind as u8,
                attributes: flags & !SECTION_TYPE,
                data: if zerofill { &[] } else { section.data().map_err(|e| error(&name, e))? },
                relocations,
                atoms: Vec::new(),
            });
        }
        // The symbols, and the globals they define or need.
        let table = file.macho_symbol_table();
        let mut symbols = Vec::new();
        let mut names = Vec::new();
        let mut undefined = Vec::new();
        for nlist in table.iter() {
            let n_type = nlist.n_type().0;
            let symbol_name = nlist.name(endian, table.strings()).unwrap_or_default();
            names.push(symbol_name);
            let stab = n_type & N_STAB != 0;
            let external = n_type & N_EXT != 0;
            let kind = n_type & N_TYPE;
            let desc = nlist.n_desc(endian).0;
            let mut sym = Sym {
                sect: nlist.n_sect(),
                value: nlist.n_value(endian),
                global: None,
                no_dead_strip: desc & N_NO_DEAD_STRIP != 0,
            };
            if !stab && external {
                let id = self.intern(symbol_name);
                sym.global = Some(id);
                let def = match kind {
                    N_SECT => Some(Def::Atom { input: index, atom: u32::MAX, offset: sym.value }),
                    N_ABS => Some(Def::Absolute(sym.value)),
                    N_UNDF if sym.value != 0 => Some(Def::Common { size: sym.value, align: 1 << ((desc >> 8) & 0xf) }),
                    _ => None,
                };
                let weak = desc & (N_WEAK_DEF | N_WEAK_REF) != 0;
                match def {
                    Some(def) => {
                        let global = &mut self.globals[id];
                        let replaces = match global.def {
                            Def::Undefined => true,
                            Def::Common { .. } => !matches!(def, Def::Common { .. }),
                            _ => global.weak && !weak,
                        };
                        if replaces {
                            global.def = def;
                            global.weak = weak;
                        }
                    }
                    None => {
                        if !weak {
                            self.globals[id].required = true;
                        }
                        if self.globals[id].def == Def::Undefined {
                            undefined.push(id);
                        }
                    }
                }
            }
            symbols.push(sym);
        }
        // Atoms: a section starts one, and so does each symbol in it when
        // the object allows cutting there; literals, `__compact_unwind`
        // and `__eh_frame` stay whole.
        let mut atoms = Vec::new();
        for (s, section) in sections.iter_mut().enumerate() {
            let whole = !subsections
                || matches!(section.kind, S_CSTRING_LITERALS | S_4BYTE_LITERALS | S_8BYTE_LITERALS | S_16BYTE_LITERALS)
                || section.sectname == b"__eh_frame"
                || section.sectname == b"__compact_unwind";
            let mut starts: Vec<u64> = vec![section.addr];
            if !whole {
                for (n, sym) in symbols.iter().enumerate() {
                    let name = names[n];
                    // `L` labels are the assembler's; `l` ones, private
                    // to the linker, still start atoms.
                    let temporary = name.starts_with(b"L");
                    if sym.sect as usize == s + 1
                        && !temporary
                        && sym.value > section.addr
                        && sym.value < section.addr + section.size
                    {
                        starts.push(sym.value);
                    }
                }
            }
            starts.sort();
            starts.dedup();
            if section.size == 0 {
                starts.clear();
            }
            for (n, &start) in starts.iter().enumerate() {
                let end = starts.get(n + 1).copied().unwrap_or(section.addr + section.size);
                section.atoms.push(atoms.len());
                atoms.push(Atom { section: s, start, end, live: false, place: None });
            }
        }
        let input = Input { name, sections, symbols, atoms };
        // Each defined global points at its atom.
        for sym in &input.symbols {
            if let Some(id) = sym.global
                && let Def::Atom { input: i, atom, offset } = self.globals[id].def
                && i == index
                && atom == u32::MAX
            {
                let (atom, start) = input.atom_at(sym.sect as usize - 1, offset).unwrap_or((0, offset));
                self.globals[id].def = Def::Atom { input: index, atom: atom as u32, offset: offset - start };
            }
        }
        self.inputs.push(input);
        Ok(undefined)
    }

    /// Brings in the members of archives that define what is undefined.
    fn archives(&mut self, archives: &[(String, &'a [u8])]) -> Result<(), String> {
        let mut index: HashMap<&'a [u8], (usize, u64)> = HashMap::new();
        let mut parsed = Vec::new();
        for (a, (name, data)) in archives.iter().enumerate() {
            let archive = object::read::archive::ArchiveFile::parse(*data).map_err(|e| error(name, e))?;
            if let Some(symbols) = archive.symbols().map_err(|e| error(name, e))? {
                for symbol in symbols {
                    let symbol = symbol.map_err(|e| error(name, e))?;
                    index.entry(symbol.name()).or_insert((a, symbol.offset().0));
                }
            }
            parsed.push(archive);
        }
        let mut loaded: HashSet<(usize, u64)> = HashSet::new();
        let mut pending: Vec<usize> =
            (0..self.globals.len()).filter(|&id| self.globals[id].def == Def::Undefined).collect();
        while let Some(id) = pending.pop() {
            let global = &self.globals[id];
            if global.def != Def::Undefined {
                continue;
            }
            let Some(&(a, offset)) = index.get(global.name) else { continue };
            if !loaded.insert((a, offset)) {
                continue;
            }
            let (archive_name, data) = &archives[a];
            let member =
                parsed[a].member(object::read::archive::ArchiveOffset(offset)).map_err(|e| error(archive_name, e))?;
            let bytes = member.data(*data).map_err(|e| error(archive_name, e))?;
            let member_name = format!("{archive_name}({})", String::from_utf8_lossy(member.name()));
            pending.extend(self.add(member_name, bytes)?);
        }
        Ok(())
    }

    /// The target of a relocation's symbol field.
    fn target(&self, input: usize, external: bool, symbol: u32, site: Option<(u32, u64)>) -> Option<Target> {
        let inp = &self.inputs[input];
        if external {
            let sym = inp.symbols.get(symbol as usize)?;
            if let Some(id) = sym.global {
                return Some(Target::Global(id));
            }
            if sym.sect == NO_SECT {
                return Some(Target::Absolute(sym.value));
            }
            return Some(Target::Local { input: input as u32, section: sym.sect as u32 - 1, address: sym.value });
        }
        // A section ordinal: the address is in the content, which the
        // caller reads.
        let (section, address) = site?;
        let _ = section;
        Some(Target::Local { input: input as u32, section: symbol - 1, address })
    }

    /// The live roots: the entry, initializers and what must not go.
    fn collect(&mut self, entry: usize) {
        let mut work: Vec<(u32, usize)> = Vec::new();
        if let Def::Atom { input, atom, .. } = self.globals[entry].def {
            work.push((input, atom as usize));
        }
        for (i, input) in self.inputs.iter().enumerate() {
            for section in &input.sections {
                let keep = section.kind == S_MOD_INIT_FUNC_POINTERS || section.attributes & S_ATTR_NO_DEAD_STRIP != 0;
                if keep {
                    work.extend(section.atoms.iter().map(|&a| (i as u32, a)));
                }
            }
            for sym in &input.symbols {
                if sym.no_dead_strip
                    && sym.sect != NO_SECT
                    && let Some((atom, _)) = input.atom_at(sym.sect as usize - 1, sym.value)
                {
                    work.push((i as u32, atom));
                }
            }
        }
        while let Some((input, atom)) = work.pop() {
            let i = input as usize;
            if std::mem::replace(&mut self.inputs[i].atoms[atom].live, true) {
                continue;
            }
            let targets = self.atom_targets(i, atom);
            // What its unwind descriptions refer to: personality and LSDA.
            let mut more = self.unwind_targets(i, atom);
            more.extend(targets);
            for target in more {
                if let Some((input, atom)) = self.atom_of(target) {
                    work.push((input, atom));
                }
            }
        }
    }

    /// The input and atom a target is in, if it is in one.
    fn atom_of(&self, target: Target) -> Option<(u32, usize)> {
        match target {
            Target::Global(id) => match self.globals[id].def {
                Def::Atom { input, atom, .. } => Some((input, atom as usize)),
                _ => None,
            },
            Target::Local { input, section, address } => {
                let (atom, _) = self.inputs[input as usize].atom_at(section as usize, address)?;
                Some((input, atom))
            }
            Target::Absolute(_) => None,
        }
    }

    /// What an atom's relocations refer to.
    fn atom_targets(&self, input: usize, atom: usize) -> Vec<Target> {
        let inp = &self.inputs[input];
        let a = &inp.atoms[atom];
        let section = &inp.sections[a.section];
        let mut out = Vec::new();
        for reloc in section.relocations_in(a.start - section.addr, a.end - section.addr) {
            out.extend(self.reloc_target(input, a.section, reloc));
            if let Some((external, symbol)) = reloc.minus {
                out.extend(self.target(input, external, symbol, None));
            }
        }
        out
    }

    /// The target of a relocation, reading a section-relative one's
    /// address from the content.
    fn reloc_target(&self, input: usize, section: usize, reloc: &Reloc) -> Option<Target> {
        if reloc.external {
            return self.target(input, true, reloc.symbol, None);
        }
        let inp = &self.inputs[input];
        let sect = &inp.sections[section];
        let at = reloc.offset as usize;
        let content = match reloc.length {
            3 => i64::from_le_bytes(sect.data.get(at..at + 8)?.try_into().ok()?),
            2 => i32::from_le_bytes(sect.data.get(at..at + 4)?.try_into().ok()?) as i64,
            _ => return None,
        };
        // In a difference, a section-relative minuend is the content plus
        // the subtrahend's address.
        let minus = match reloc.minus {
            Some((true, symbol)) => inp.symbols.get(symbol as usize).map_or(0, |s| s.value as i64),
            _ => 0,
        };
        let address = if reloc.minus.is_some() {
            (content + minus) as u64
        } else if reloc.pcrel {
            // x86_64: from the end of the field, and the bytes after it.
            let after = match reloc.kind {
                k if k == X86_64_RELOC_SIGNED_1 => 1,
                k if k == X86_64_RELOC_SIGNED_2 => 2,
                k if k == X86_64_RELOC_SIGNED_4 => 4,
                _ => 0,
            };
            (sect.addr as i64 + reloc.offset as i64 + 4 + after + content) as u64
        } else {
            content as u64
        };
        self.target(input, false, reloc.symbol, Some((section as u32, address)))
    }

    /// The personalities and LSDAs the unwind descriptions of an atom use.
    fn unwind_targets(&self, input: usize, atom: usize) -> Vec<Target> {
        self.unwind_refs.get(&(input as u32, atom)).cloned().unwrap_or_default()
    }

    /// Finds, for each function, what its unwind descriptions refer to.
    fn unwind_references(&mut self) {
        let mut refs: HashMap<(u32, usize), Vec<Target>> = HashMap::new();
        for i in 0..self.inputs.len() {
            for entry in self.compact_entries(i) {
                if let Some(function) = entry.function {
                    refs.entry(function).or_default().extend(entry.personality.into_iter().chain(entry.lsda));
                }
            }
        }
        let (frames, _) = self.eh_frame_records_all();
        for frame in frames {
            let Some(function) = frame.function else { continue };
            let input = &self.inputs[frame.input];
            let s = input.sections.iter().position(|s| s.sectname == b"__eh_frame").unwrap();
            let section = &input.sections[s];
            let mut ranges = vec![(frame.start, frame.start + frame.len)];
            if let Some(cie) = frame.cie {
                ranges.push((cie, cie + 8));
                let data = section.data;
                let length = u32::from_le_bytes(data[cie as usize..cie as usize + 4].try_into().unwrap()) as u64;
                ranges[1] = (cie, cie + length + 4);
            }
            for (start, end) in ranges {
                for reloc in section.relocations_in(start, end) {
                    if reloc.offset == frame.start + 8 {
                        continue;
                    }
                    refs.entry(function).or_default().extend(self.reloc_target(frame.input, s, reloc));
                }
            }
        }
        self.unwind_refs = refs;
    }

    /// Finds what live code needs from the linker: GOT entries, stubs for
    /// calls into libraries, and pointers to thread-local descriptors.
    fn scan(&mut self) -> Result<(), String> {
        for i in 0..self.inputs.len() {
            for s in 0..self.inputs[i].sections.len() {
                let section = &self.inputs[i].sections[s];
                if section.sectname == b"__compact_unwind" || section.segname == b"__DWARF" {
                    continue;
                }
                let atoms: Vec<usize> =
                    section.atoms.iter().copied().filter(|&a| self.inputs[i].atoms[a].live).collect();
                for atom in atoms {
                    let a = &self.inputs[i].atoms[atom];
                    let section = &self.inputs[i].sections[a.section];
                    let relocations: Vec<Reloc> =
                        section.relocations_in(a.start - section.addr, a.end - section.addr).copied().collect();
                    for reloc in relocations {
                        let Some(target) = self.reloc_target(i, s, &reloc) else { continue };
                        let imported = self.imported(target);
                        match (self.arch, reloc.kind) {
                            (Arch::Arm64, k)
                                if k == ARM64_RELOC_GOT_LOAD_PAGE21
                                    || k == ARM64_RELOC_GOT_LOAD_PAGEOFF12
                                    || k == ARM64_RELOC_POINTER_TO_GOT =>
                            {
                                self.slot(target);
                            }
                            (Arch::X86_64, k) if k == X86_64_RELOC_GOT_LOAD || k == X86_64_RELOC_GOT => {
                                self.slot(target);
                            }
                            (Arch::Arm64, k)
                                if k == ARM64_RELOC_TLVP_LOAD_PAGE21 || k == ARM64_RELOC_TLVP_LOAD_PAGEOFF12 =>
                            {
                                self.tlv_slot(target);
                            }
                            (Arch::X86_64, k) if k == X86_64_RELOC_TLV => {
                                self.tlv_slot(target);
                            }
                            (Arch::Arm64, k) if k == ARM64_RELOC_BRANCH26 && imported => self.stub(target),
                            (Arch::X86_64, k) if k == X86_64_RELOC_BRANCH && imported => self.stub(target),
                            _ if imported => self.import(target),
                            _ => {}
                        }
                    }
                }
            }
        }
        // The records of `__eh_frame` that stay point through the GOT too.
        let (frames, _) = self.eh_frame_records();
        for frame in &frames {
            let input = &self.inputs[frame.input];
            let s = input.sections.iter().position(|s| s.sectname == b"__eh_frame").unwrap();
            let relocations: Vec<Reloc> =
                input.sections[s].relocations_in(frame.start, frame.start + frame.len).copied().collect();
            for reloc in relocations {
                let Some(target) = self.reloc_target(frame.input, s, &reloc) else { continue };
                let got = match self.arch {
                    Arch::Arm64 => reloc.kind == ARM64_RELOC_POINTER_TO_GOT,
                    Arch::X86_64 => reloc.kind == X86_64_RELOC_GOT,
                };
                if got {
                    self.slot(target);
                } else if self.imported(target) {
                    self.import(target);
                }
            }
        }
        // Personalities are read through the GOT.
        for i in 0..self.inputs.len() {
            for entry in self.compact_entries(i) {
                if entry.function.is_some_and(|(input, atom)| self.inputs[input as usize].atoms[atom].live)
                    && let Some(personality) = entry.personality
                {
                    self.slot(personality);
                }
            }
        }
        Ok(())
    }

    fn imported(&self, target: Target) -> bool {
        matches!(target, Target::Global(id) if self.globals[id].def == Def::Undefined)
    }

    fn slot(&mut self, target: Target) {
        if !self.slots.contains_key(&target) {
            self.slots.insert(target, self.got.len() as u32);
            self.got.push(target);
            self.import(target);
        }
    }

    fn tlv_slot(&mut self, target: Target) {
        if !self.tlv_slots.contains_key(&target) {
            self.tlv_slots.insert(target, self.tlv.len() as u32);
            self.tlv.push(target);
        }
    }

    fn stub(&mut self, target: Target) {
        let Target::Global(id) = target else { return };
        if !self.stub_of.contains_key(&id) {
            self.stub_of.insert(id, self.stubs.len() as u32);
            self.stubs.push(id);
            self.slot(target);
        }
    }

    fn import(&mut self, target: Target) {
        let Target::Global(id) = target else { return };
        if self.globals[id].def == Def::Undefined && !self.import_of.contains_key(&id) {
            self.import_of.insert(id, self.imports.len() as u32);
            self.imports.push(id);
        }
    }
}

impl Input<'_> {
    /// The atom of a section that holds an address, and where it starts.
    fn atom_at(&self, section: usize, address: u64) -> Option<(usize, u64)> {
        let sect = self.sections.get(section)?;
        let at = sect.atoms.partition_point(|&a| self.atoms[a].start <= address);
        let atom = *sect.atoms.get(at.checked_sub(1)?)?;
        Some((atom, self.atoms[atom].start))
    }
}

impl Sect<'_> {
    /// The relocations with offsets in `[start, end)`.
    fn relocations_in(&self, start: u64, end: u64) -> impl Iterator<Item = &Reloc> {
        let first = self.relocations.partition_point(|r| r.offset < start);
        self.relocations[first..].iter().take_while(move |r| r.offset < end)
    }
}

/// An entry of `__compact_unwind`: its function, length, encoding,
/// personality and LSDA.
struct Compact {
    function: Option<(u32, usize)>,
    offset: u64,
    length: u32,
    encoding: u32,
    personality: Option<Target>,
    lsda: Option<Target>,
}

impl Linker<'_> {
    fn compact_entries(&self, input: usize) -> Vec<Compact> {
        let inp = &self.inputs[input];
        let Some(s) = inp.sections.iter().position(|s| s.sectname == b"__compact_unwind") else { return Vec::new() };
        let section = &inp.sections[s];
        let mut out = Vec::new();
        for n in 0..section.data.len() / 32 {
            let at = n * 32;
            let field = |offset: usize| section.relocations_in((at + offset) as u64, (at + offset + 1) as u64).next();
            let read = |offset: usize, size: usize| {
                let mut bytes = [0u8; 8];
                bytes[..size].copy_from_slice(&section.data[at + offset..at + offset + size]);
                u64::from_le_bytes(bytes)
            };
            let resolve = |offset: usize| -> Option<Target> {
                let reloc = field(offset)?;
                self.reloc_target(input, s, reloc)
            };
            let function = resolve(0);
            let function_offset = match function {
                Some(Target::Global(_)) => read(0, 8),
                _ => 0,
            };
            out.push(Compact {
                function: function.and_then(|t| self.atom_of(t)),
                offset: function_offset,
                length: read(8, 4) as u32,
                encoding: read(12, 4) as u32,
                personality: resolve(16),
                lsda: resolve(24),
            });
        }
        out
    }
}

/// Writing the image.
struct Image {
    bytes: Vec<u8>,
}

impl Image {
    fn put(&mut self, at: u64, data: &[u8]) {
        let at = at as usize;
        if self.bytes.len() < at + data.len() {
            self.bytes.resize(at + data.len(), 0);
        }
        self.bytes[at..at + data.len()].copy_from_slice(data);
    }

    fn u32(&mut self, at: u64, v: u32) {
        self.put(at, &v.to_le_bytes());
    }

    fn u64(&mut self, at: u64, v: u64) {
        self.put(at, &v.to_le_bytes());
    }

    fn read32(&self, at: u64) -> u32 {
        u32::from_le_bytes(self.bytes[at as usize..at as usize + 4].try_into().unwrap())
    }

    fn read64(&self, at: u64) -> u64 {
        u64::from_le_bytes(self.bytes[at as usize..at as usize + 8].try_into().unwrap())
    }
}

fn uleb(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn sleb(out: &mut Vec<u8>, mut value: i64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            return;
        }
    }
}

/// Where output sections go: their segment, in order.
fn output_segment(segname: &[u8], sectname: &[u8], kind: u8) -> Option<&'static str> {
    if segname == b"__DWARF" || segname == b"__LLVM" || sectname == b"__compact_unwind" || segname == b"__LD" {
        return None;
    }
    Some(match segname {
        b"__TEXT" => TEXT,
        _ if kind == S_MOD_INIT_FUNC_POINTERS || kind == S_NON_LAZY_SYMBOL_POINTERS => DATA_CONST,
        _ => DATA,
    })
}

/// The rank of a section in its segment: code first, zero-filled last,
/// thread-local data before thread-local zero fill.
fn rank(segment: &str, sectname: &[u8], kind: u8) -> u32 {
    match (segment, kind) {
        (TEXT, _) if sectname == b"__text" => 0,
        (TEXT, _) if sectname == b"__stubs" => 1,
        (TEXT, _) if sectname == b"__unwind_info" => 8,
        (TEXT, _) if sectname == b"__eh_frame" => 9,
        (TEXT, _) => 2,
        (_, S_THREAD_LOCAL_VARIABLES) => 3,
        (_, S_THREAD_LOCAL_REGULAR) => 4,
        (_, S_THREAD_LOCAL_ZEROFILL) => 5,
        (_, S_ZEROFILL | S_GB_ZEROFILL) => 6,
        _ => 2,
    }
}

impl<'a> Linker<'a> {
    fn out(&mut self, segment: &'static str, sectname: &[u8], kind: u8, attributes: u32) -> usize {
        let key = (segment, sectname.to_vec());
        if let Some(&i) = self.out_index.get(&key) {
            return i;
        }
        self.outs.push(OutSection {
            segname: segment,
            sectname: sectname.to_vec(),
            kind,
            attributes,
            align: 1,
            size: 0,
            addr: 0,
            offset: 0,
        });
        self.out_index.insert(key, self.outs.len() - 1);
        self.outs.len() - 1
    }

    fn address(&self, target: Target) -> u64 {
        match target {
            Target::Global(id) => self.global_address(id),
            Target::Local { input, section, address } => {
                let inp = &self.inputs[input as usize];
                match inp.atom_at(section as usize, address) {
                    Some((atom, start)) => match inp.atoms[atom].place {
                        Some((out, offset)) => self.outs[out].addr + offset + (address - start),
                        None => 0,
                    },
                    None => 0,
                }
            }
            Target::Absolute(value) => value,
        }
    }

    fn global_address(&self, id: usize) -> u64 {
        match self.globals[id].def {
            Def::Atom { input, atom, offset } => match self.inputs[input as usize].atoms[atom as usize].place {
                Some((out, at)) => self.outs[out].addr + at + offset,
                None => 0,
            },
            Def::Absolute(value) => value,
            Def::Common { .. } => {
                let bss = self.out_index[&(DATA, b"__common".to_vec())];
                self.outs[bss].addr + self.globals[id].common
            }
            Def::Header => BASE,
            Def::Undefined => match self.stub_of.get(&id) {
                Some(&stub) => {
                    let stubs = self.out_index[&(TEXT, b"__stubs".to_vec())];
                    self.outs[stubs].addr + stub as u64 * self.stub_size()
                }
                None => 0,
            },
        }
    }

    fn stub_size(&self) -> u64 {
        match self.arch {
            Arch::Arm64 => 12,
            Arch::X86_64 => 6,
        }
    }

    fn slot_address(&self, target: Target) -> u64 {
        let got = self.out_index[&(DATA_CONST, b"__got".to_vec())];
        self.outs[got].addr + 8 * self.slots[&target] as u64
    }

    fn tlv_slot_address(&self, target: Target) -> u64 {
        let ptrs = self.out_index[&(DATA_CONST, b"__thread_ptrs".to_vec())];
        self.outs[ptrs].addr + 8 * self.tlv_slots[&target] as u64
    }

    /// Lays out the atoms in output sections, the sections in segments, and
    /// writes the executable.
    fn layout_and_write(&mut self, inputs: &Inputs, entry: usize) -> Result<Vec<u8>, String> {
        // The atoms that stay, in input order, in their output sections.
        let mut placements: Vec<(usize, usize, usize)> = Vec::new();
        for i in 0..self.inputs.len() {
            for s in 0..self.inputs[i].sections.len() {
                let (segname, sectname, kind, attributes, align) = {
                    let section = &self.inputs[i].sections[s];
                    (section.segname.clone(), section.sectname.clone(), section.kind, section.attributes, section.align)
                };
                let Some(segment) = output_segment(&segname, &sectname, kind) else { continue };
                let out = self.out(segment, &sectname, kind, attributes);
                self.outs[out].align = self.outs[out].align.max(align);
                for &atom in &self.inputs[i].sections[s].atoms {
                    if self.inputs[i].atoms[atom].live {
                        placements.push((out, i, atom));
                    }
                }
            }
        }
        // Synthesized sections.
        self.out(TEXT, b"__text", S_REGULAR, S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS);
        if !self.stubs.is_empty() {
            let stubs = self.out(TEXT, b"__stubs", S_SYMBOL_STUBS, S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS);
            self.outs[stubs].size = self.stubs.len() as u64 * self.stub_size();
            self.outs[stubs].align = 4;
        }
        let got = self.out(DATA_CONST, b"__got", S_NON_LAZY_SYMBOL_POINTERS, 0);
        self.outs[got].size = 8 * self.got.len() as u64;
        self.outs[got].align = 8;
        if !self.tlv.is_empty() {
            let ptrs = self.out(DATA_CONST, b"__thread_ptrs", S_THREAD_LOCAL_VARIABLE_POINTERS, 0);
            self.outs[ptrs].size = 8 * self.tlv.len() as u64;
            self.outs[ptrs].align = 8;
        }
        let commons: Vec<usize> =
            (0..self.globals.len()).filter(|&id| matches!(self.globals[id].def, Def::Common { .. })).collect();
        if !commons.is_empty() {
            let common = self.out(DATA, b"__common", S_ZEROFILL, 0);
            for id in commons {
                let Def::Common { size, align } = self.globals[id].def else { continue };
                let at = self.outs[common].size.next_multiple_of(align);
                self.globals[id].common = at;
                self.outs[common].size = at + size;
                self.outs[common].align = self.outs[common].align.max(align);
            }
        }
        for (out, i, atom) in placements {
            let a = &self.inputs[i].atoms[atom];
            let align = self.inputs[i].sections[a.section].align;
            // The first atom of a section keeps the section's alignment;
            // later ones keep what their offset in it gave them.
            let natural = if a.start == self.inputs[i].sections[a.section].addr {
                align
            } else {
                (1u64 << (a.start - self.inputs[i].sections[a.section].addr).trailing_zeros().min(12)).min(align)
            };
            let at = self.outs[out].size.next_multiple_of(natural.max(1));
            self.outs[out].size = at + (a.end - a.start);
            self.inputs[i].atoms[atom].place = Some((out, at));
        }
        // `__unwind_info` and `__eh_frame` are rebuilt.
        let unwind = self.out(TEXT, b"__unwind_info", S_REGULAR, 0);
        self.outs[unwind].align = 4;
        let (frames, frame_size) = self.eh_frame_records();
        if frame_size > 0 {
            let eh = self.out(
                TEXT,
                b"__eh_frame",
                S_COALESCED,
                S_ATTR_NO_TOC | S_ATTR_STRIP_STATIC_SYMS | S_ATTR_LIVE_SUPPORT,
            );
            self.outs[eh].size = frame_size;
            self.outs[eh].align = 8;
        }
        let unwind_size = self.unwind_info_size(&frames);
        self.outs[unwind].size = unwind_size;

        // Segments and their sections.
        let mut order: Vec<usize> = (0..self.outs.len()).collect();
        let segment_rank = |s: &str| match s {
            TEXT => 0,
            DATA_CONST => 1,
            _ => 2,
        };
        order.sort_by_key(|&o| {
            let out = &self.outs[o];
            (segment_rank(out.segname), rank(out.segname, &out.sectname, out.kind), o)
        });
        let order: Vec<usize> =
            order.into_iter().filter(|&o| self.outs[o].size > 0 || self.outs[o].sectname == b"__text").collect();
        let dylibs = &inputs.dylibs;
        let commands_size = self.commands_size(&order, dylibs, &inputs.rpaths);
        let header_size = 32 + commands_size;
        let mut offset = header_size as u64;
        let mut addr = BASE + offset;
        let mut segments: Vec<(&'static str, u64, u64, u64, u64)> = Vec::new();
        for segment in [TEXT, DATA_CONST, DATA] {
            let sections: Vec<usize> = order.iter().copied().filter(|&o| self.outs[o].segname == segment).collect();
            if sections.is_empty() && segment != TEXT {
                continue;
            }
            if segment != TEXT {
                offset = offset.next_multiple_of(PAGE);
                addr = addr.next_multiple_of(PAGE);
            }
            let (start_offset, start_addr) = if segment == TEXT { (0, BASE) } else { (offset, addr) };
            for o in sections {
                let out = &mut self.outs[o];
                let zerofill = matches!(out.kind, S_ZEROFILL | S_GB_ZEROFILL | S_THREAD_LOCAL_ZEROFILL);
                addr = addr.next_multiple_of(out.align);
                if !zerofill {
                    offset = offset.next_multiple_of(out.align);
                    // A section's offset and address agree modulo pages.
                    addr = addr.max(start_addr + (offset - start_offset));
                    offset = start_offset + (addr - start_addr);
                }
                out.addr = addr;
                out.offset = if zerofill { 0 } else { offset };
                addr += out.size;
                if !zerofill {
                    offset += out.size;
                }
            }
            let file_end = offset.next_multiple_of(PAGE);
            let vm_end = addr.next_multiple_of(PAGE);
            segments.push((segment, start_addr, vm_end - start_addr, start_offset, file_end - start_offset));
            offset = file_end;
            addr = vm_end;
        }
        let linkedit_offset = offset;
        let linkedit_addr = addr;

        let mut image = Image { bytes: vec![0; linkedit_offset as usize] };
        // Content of atoms, relocated; the pointers to rebase and bind.
        let mut rebases: Vec<u64> = Vec::new();
        let mut binds: Vec<(u64, usize, i64)> = Vec::new();
        for i in 0..self.inputs.len() {
            for atom in 0..self.inputs[i].atoms.len() {
                let a = &self.inputs[i].atoms[atom];
                let Some((out, at)) = a.place else { continue };
                let o = &self.outs[out];
                if matches!(o.kind, S_ZEROFILL | S_GB_ZEROFILL | S_THREAD_LOCAL_ZEROFILL) {
                    continue;
                }
                let section = &self.inputs[i].sections[a.section];
                let from = (a.start - section.addr) as usize;
                let to = (a.end - section.addr) as usize;
                let file = o.offset + at;
                let base = o.addr + at;
                image.put(file, &section.data[from..to]);
                let relocations: Vec<Reloc> = section.relocations_in(from as u64, to as u64).copied().collect();
                for reloc in relocations {
                    self.apply(
                        &mut image,
                        i,
                        a.section,
                        &reloc,
                        // Where offset 0 of the section would be: only the
                        // sums with the atom's offsets are addresses.
                        file.wrapping_sub(from as u64),
                        base.wrapping_sub(from as u64),
                        &mut rebases,
                        &mut binds,
                    )?;
                }
            }
        }
        // The thread-local descriptors: `offset` from the template's start.
        let template = order
            .iter()
            .copied()
            .filter(|&o| matches!(self.outs[o].kind, S_THREAD_LOCAL_REGULAR | S_THREAD_LOCAL_ZEROFILL))
            .map(|o| self.outs[o].addr)
            .min();
        if let Some(template) = template {
            for &o in &order {
                if self.outs[o].kind != S_THREAD_LOCAL_VARIABLES {
                    continue;
                }
                let (start, size, file) = (self.outs[o].addr, self.outs[o].size, self.outs[o].offset);
                let mut offsets = HashSet::new();
                for n in 0..size / 24 {
                    let field = file + 24 * n + 16;
                    let value = image.read64(field);
                    image.u64(field, value.wrapping_sub(template));
                    offsets.insert(start + 24 * n + 16);
                }
                rebases.retain(|r| !offsets.contains(r));
            }
        }
        // The GOT, TLV pointers and stubs.
        for (n, &target) in self.got.iter().enumerate() {
            let at = self.outs[self.out_index[&(DATA_CONST, b"__got".to_vec())]].offset + 8 * n as u64;
            let address = self.slot_address(target);
            if self.imported(target) {
                let Target::Global(id) = target else { unreachable!() };
                binds.push((address, id, 0));
            } else {
                image.u64(at, self.address(target));
                rebases.push(address);
            }
        }
        for (n, &target) in self.tlv.iter().enumerate() {
            let at = self.outs[self.out_index[&(DATA_CONST, b"__thread_ptrs".to_vec())]].offset + 8 * n as u64;
            image.u64(at, self.address(target));
            rebases.push(self.tlv_slot_address(target));
        }
        if !self.stubs.is_empty() {
            let stubs = self.out_index[&(TEXT, b"__stubs".to_vec())];
            for (n, &id) in self.stubs.iter().enumerate() {
                let stub = self.outs[stubs].addr + n as u64 * self.stub_size();
                let slot = self.slot_address(Target::Global(id));
                let code = stub_code(self.arch, stub, slot);
                image.put(self.outs[stubs].offset + n as u64 * self.stub_size(), &code);
            }
        }
        // Unwind information.
        let eh_frame = self.out_index.get(&(TEXT, b"__eh_frame".to_vec())).copied();
        let fde_at = if let Some(eh) = eh_frame {
            self.write_eh_frame(&mut image, &frames, eh, &mut rebases, &mut binds)?
        } else {
            HashMap::new()
        };
        let unwind = self.out_index[&(TEXT, b"__unwind_info".to_vec())];
        let info = self.unwind_info(&frames, &fde_at);
        image.put(self.outs[unwind].offset, &info);

        // __LINKEDIT: rebases, binds, the symbol table and the signature.
        let segment_of = |address: u64| -> (u8, u64) {
            let n = segments.iter().position(|s| address >= s.1 && address < s.1 + s.2).unwrap_or(0);
            ((n + 1) as u8, address - segments[n].1)
        };
        rebases.sort();
        rebases.dedup();
        let mut rebase = vec![REBASE_OPCODE_SET_TYPE_IMM | REBASE_TYPE_POINTER];
        for &address in &rebases {
            let (segment, offset) = segment_of(address);
            rebase.push(REBASE_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB | segment);
            uleb(&mut rebase, offset);
            rebase.push(REBASE_OPCODE_DO_REBASE_IMM_TIMES | 1);
        }
        rebase.push(REBASE_OPCODE_DONE);
        let mut bind = Vec::new();
        binds.sort_by_key(|b| b.0);
        for &(address, id, addend) in &binds {
            let (segment, offset) = segment_of(address);
            bind.push(BIND_OPCODE_SET_DYLIB_SPECIAL_IMM | (BIND_SPECIAL_DYLIB_FLAT_LOOKUP as u8 & BIND_IMMEDIATE_MASK));
            let weak = !self.globals[id].required;
            bind.push(BIND_OPCODE_SET_SYMBOL_TRAILING_FLAGS_IMM | if weak { BIND_SYMBOL_FLAGS_WEAK_IMPORT } else { 0 });
            bind.extend_from_slice(self.globals[id].name);
            bind.push(0);
            bind.push(BIND_OPCODE_SET_TYPE_IMM | BIND_TYPE_POINTER);
            if addend != 0 {
                bind.push(BIND_OPCODE_SET_ADDEND_SLEB);
                sleb(&mut bind, addend);
            }
            bind.push(BIND_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB | segment);
            uleb(&mut bind, offset);
            bind.push(BIND_OPCODE_DO_BIND);
        }
        bind.push(BIND_OPCODE_DONE);
        let pad8 = |v: &mut Vec<u8>| v.resize(v.len().next_multiple_of(8), 0);
        pad8(&mut rebase);
        pad8(&mut bind);
        // The symbol table: what the program defines, unless stripped, and
        // what it imports, which the indirect table refers to.
        let (symtab, strtab, locals, externals, undefined, indirect) = self.symbol_table(inputs.strip);
        let mut linkedit = Vec::new();
        let rebase_off = linkedit_offset;
        linkedit.extend_from_slice(&rebase);
        let bind_off = linkedit_offset + linkedit.len() as u64;
        linkedit.extend_from_slice(&bind);
        let symtab_off = linkedit_offset + linkedit.len() as u64;
        linkedit.extend_from_slice(&symtab);
        let indirect_off = linkedit_offset + linkedit.len() as u64;
        for index in &indirect {
            linkedit.extend_from_slice(&index.to_le_bytes());
        }
        let strtab_off = linkedit_offset + linkedit.len() as u64;
        linkedit.extend_from_slice(&strtab);
        linkedit.resize(linkedit.len().next_multiple_of(16), 0);
        let signature_off = linkedit_offset + linkedit.len() as u64;
        let pages = signature_off.div_ceil(4096);
        let identifier = inputs.identifier.as_bytes();
        let signature_size = (12 + 8 + 88 + identifier.len() as u64 + 1 + 32 * pages).next_multiple_of(16);
        let linkedit_size = linkedit.len() as u64 + signature_size;
        image.put(linkedit_offset, &linkedit);
        segments.push((
            "__LINKEDIT",
            linkedit_addr,
            linkedit_size.next_multiple_of(PAGE),
            linkedit_offset,
            linkedit_size,
        ));

        // The header and load commands.
        let text_segment = segments[0];
        let entry_offset = self.global_address(entry) - BASE;
        let tlv = template.is_some();
        let mut commands = Vec::new();
        let mut ncmds = 0u32;
        let mut command = |bytes: Vec<u8>| {
            commands.extend_from_slice(&bytes);
            ncmds += 1;
        };
        command(segment_command("__PAGEZERO", 0, BASE, 0, 0, 0, 0, &[]));
        for &(name, vmaddr, vmsize, fileoff, filesize) in &segments {
            let sections: Vec<&OutSection> =
                order.iter().map(|&o| &self.outs[o]).filter(|o| o.segname == name).collect();
            let (maxprot, initprot) = match name {
                TEXT => (5, 5),
                "__LINKEDIT" => (1, 1),
                _ => (3, 3),
            };
            let flags = if name == DATA_CONST { SG_READ_ONLY } else { 0 };
            command(segment_command(
                name,
                vmaddr,
                vmsize,
                fileoff,
                filesize,
                maxprot | (initprot << 8),
                flags,
                &sections_with_indirect(&sections, &self.stubs, &self.got, self.stub_size()),
            ));
        }
        command(dyld_info(rebase_off, rebase.len(), bind_off, bind.len()));
        command(symtab_command(symtab_off, (symtab.len() / 16) as u32, strtab_off, strtab.len() as u32));
        command(dysymtab_command(locals, externals, undefined, indirect_off, indirect.len() as u32));
        command(path_command(LC_LOAD_DYLINKER, "/usr/lib/dyld"));
        let mut main = vec![0u8; 24];
        main[0..4].copy_from_slice(&LC_MAIN.to_le_bytes());
        main[4..8].copy_from_slice(&24u32.to_le_bytes());
        main[8..16].copy_from_slice(&entry_offset.to_le_bytes());
        command(main);
        command(build_version());
        command(uuid(&image.bytes));
        for dylib in dylibs {
            command(dylib_command(dylib));
        }
        for rpath in &inputs.rpaths {
            command(path_command(LC_RPATH, rpath));
        }
        let mut signature_command = vec![0u8; 16];
        signature_command[0..4].copy_from_slice(&LC_CODE_SIGNATURE.to_le_bytes());
        signature_command[4..8].copy_from_slice(&16u32.to_le_bytes());
        signature_command[8..12].copy_from_slice(&(signature_off as u32).to_le_bytes());
        signature_command[12..16].copy_from_slice(&(signature_size as u32).to_le_bytes());
        command(signature_command);
        if commands.len() != commands_size {
            return Err(format!("load commands take {} bytes, not the {commands_size} reserved", commands.len()));
        }
        let (cputype, cpusubtype) = match self.arch {
            Arch::Arm64 => (CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64_ALL),
            Arch::X86_64 => (CPU_TYPE_X86_64, CPU_SUBTYPE_X86_64_ALL),
        };
        let mut flags = MH_DYLDLINK | MH_TWOLEVEL | MH_PIE;
        if tlv {
            flags |= MH_HAS_TLV_DESCRIPTORS;
        }
        image.u32(0, MH_MAGIC_64);
        image.u32(4, cputype);
        image.u32(8, cpusubtype);
        image.u32(12, MH_EXECUTE);
        image.u32(16, ncmds);
        image.u32(20, commands.len() as u32);
        image.u32(24, flags);
        image.u32(28, 0);
        image.put(32, &commands);
        let _ = text_segment;
        image.bytes.resize(signature_off as usize, 0);
        let signature = sign(&image.bytes, identifier, segments[0].3, segments[0].4, signature_size as usize);
        image.bytes.extend_from_slice(&signature);
        Ok(image.bytes)
    }
}

/// A stub: a jump through the GOT entry at `slot`.
fn stub_code(arch: Arch, stub: u64, slot: u64) -> Vec<u8> {
    match arch {
        Arch::Arm64 => {
            let pages = (((slot & !0xfff) as i64 - (stub & !0xfff) as i64) >> 12) as u32;
            let adrp = 0x9000_0010 | ((pages & 3) << 29) | (((pages >> 2) & 0x7ffff) << 5);
            let ldr = 0xf940_0210 | ((((slot & 0xfff) >> 3) as u32) << 10);
            [adrp, ldr, 0xd61f_0200].iter().flat_map(|w| w.to_le_bytes()).collect()
        }
        Arch::X86_64 => {
            let mut code = vec![0xff, 0x25];
            code.extend_from_slice(&((slot as i64 - (stub + 6) as i64) as i32).to_le_bytes());
            code
        }
    }
}

impl Linker<'_> {
    /// Applies a relocation of an atom's content copied so that the
    /// section's offset 0 would be at `file`, and seen at `base`.
    #[allow(clippy::too_many_arguments)]
    fn apply(
        &self,
        image: &mut Image,
        input: usize,
        section: usize,
        reloc: &Reloc,
        file: u64,
        base: u64,
        rebases: &mut Vec<u64>,
        binds: &mut Vec<(u64, usize, i64)>,
    ) -> Result<(), String> {
        let (f, p) = (file.wrapping_add(reloc.offset), base.wrapping_add(reloc.offset));
        let Some(target) = self.reloc_target(input, section, reloc) else { return Ok(()) };
        let name = &self.inputs[input].name;
        let fail = |what: &str| error(name, format!("{what} (relocation {} {:?})", reloc.kind, target));
        let insn = image.read32(f);
        // An external relocation keeps its addend in the content, besides
        // `ARM64_RELOC_ADDEND`.
        let content = |image: &Image| match reloc.length {
            3 => image.read64(f) as i64,
            _ => image.read32(f) as i32 as i64,
        };
        let s = self.address(target) as i64;
        match (self.arch, reloc.kind) {
            // A pointer, or the difference of two addresses.
            (_, 0) if !reloc.pcrel => {
                let addend = if reloc.external { content(image) } else { 0 };
                let mut value = s + addend;
                if let Some((external, symbol)) = reloc.minus {
                    // `A - B`: the content holds the addend, besides what a
                    // section-relative `A` already was.
                    let minus = self.target(input, external, symbol, None).ok_or_else(|| fail("no subtrahend"))?;
                    value = s + addend - self.address(minus) as i64;
                } else if reloc.length == 3 {
                    if self.imported(target) {
                        let Target::Global(id) = target else { unreachable!() };
                        binds.push((p, id, addend));
                        value = 0;
                    } else {
                        rebases.push(p);
                    }
                }
                match reloc.length {
                    3 => image.u64(f, value as u64),
                    _ => image.u32(f, value as u32),
                }
            }
            (Arch::Arm64, k) if k == ARM64_RELOC_BRANCH26 => {
                let delta = (s + reloc.addend - p as i64) >> 2;
                if !(-(1 << 25)..(1 << 25)).contains(&delta) {
                    return Err(fail("out of range"));
                }
                image.u32(f, (insn & 0xfc00_0000) | (delta as u32 & 0x03ff_ffff));
            }
            (Arch::Arm64, k)
                if k == ARM64_RELOC_PAGE21 || k == ARM64_RELOC_GOT_LOAD_PAGE21 || k == ARM64_RELOC_TLVP_LOAD_PAGE21 =>
            {
                let target_address = match k {
                    _ if k == ARM64_RELOC_GOT_LOAD_PAGE21 => self.slot_address(target) as i64,
                    _ if k == ARM64_RELOC_TLVP_LOAD_PAGE21 => self.tlv_slot_address(target) as i64,
                    _ => s + reloc.addend,
                };
                let pages = ((target_address & !0xfff) - (p as i64 & !0xfff)) >> 12;
                let pages = pages as u32;
                image.u32(f, (insn & 0x9f00_001f) | ((pages & 3) << 29) | (((pages >> 2) & 0x7ffff) << 5));
            }
            (Arch::Arm64, k)
                if k == ARM64_RELOC_PAGEOFF12
                    || k == ARM64_RELOC_GOT_LOAD_PAGEOFF12
                    || k == ARM64_RELOC_TLVP_LOAD_PAGEOFF12 =>
            {
                let target_address = match k {
                    _ if k == ARM64_RELOC_GOT_LOAD_PAGEOFF12 => self.slot_address(target) as i64,
                    _ if k == ARM64_RELOC_TLVP_LOAD_PAGEOFF12 => self.tlv_slot_address(target) as i64,
                    _ => s + reloc.addend,
                };
                let low = (target_address & 0xfff) as u32;
                // A load or store scales its offset by its size.
                let scale = if insn & 0x3b00_0000 == 0x3900_0000 {
                    let size = insn >> 30;
                    if insn & 0x0480_0000 == 0x0480_0000 { 4 } else { size }
                } else {
                    0
                };
                image.u32(f, (insn & !(0xfff << 10)) | ((low >> scale) << 10));
            }
            (Arch::Arm64, k) if k == ARM64_RELOC_POINTER_TO_GOT => {
                let value = self.slot_address(target) as i64 - p as i64;
                image.u32(f, value as u32);
            }
            (Arch::X86_64, k)
                if [
                    X86_64_RELOC_SIGNED,
                    X86_64_RELOC_SIGNED_1,
                    X86_64_RELOC_SIGNED_2,
                    X86_64_RELOC_SIGNED_4,
                    X86_64_RELOC_BRANCH,
                ]
                .contains(&k) =>
            {
                // External: the content is the addend less the bytes after
                // the field, so the distance is from the field's end; local:
                // the content was the distance to the target, which
                // `reloc_target` read, from the instruction's end.
                let value = if reloc.external {
                    s + content(image) - (p as i64 + 4)
                } else {
                    let after = match k {
                        _ if k == X86_64_RELOC_SIGNED_1 => 1,
                        _ if k == X86_64_RELOC_SIGNED_2 => 2,
                        _ if k == X86_64_RELOC_SIGNED_4 => 4,
                        _ => 0,
                    };
                    s - (p as i64 + 4 + after)
                };
                image.u32(f, i32::try_from(value).map_err(|_| fail("out of range"))? as u32);
            }
            (Arch::X86_64, k) if k == X86_64_RELOC_GOT_LOAD || k == X86_64_RELOC_GOT => {
                let value = self.slot_address(target) as i64 + content(image) - (p as i64 + 4);
                image.u32(f, value as u32);
            }
            (Arch::X86_64, k) if k == X86_64_RELOC_TLV => {
                let value = self.tlv_slot_address(target) as i64 + content(image) - (p as i64 + 4);
                image.u32(f, value as u32);
            }
            _ => return Err(fail("unsupported")),
        }
        Ok(())
    }
}

/// The size of the load commands.
impl Linker<'_> {
    fn commands_size(&self, order: &[usize], dylibs: &[String], rpaths: &[String]) -> usize {
        let segments = [TEXT, DATA_CONST, DATA];
        let mut size = 72; // __PAGEZERO
        for segment in segments {
            let count = order.iter().filter(|&&o| self.outs[o].segname == segment).count();
            if count > 0 || segment == TEXT {
                size += 72 + 80 * count;
            }
        }
        size += 72; // __LINKEDIT
        // DYLD_INFO, SYMTAB, DYSYMTAB, the loader, MAIN, BUILD_VERSION, UUID
        // and CODE_SIGNATURE.
        size += 48 + 24 + 80 + path_command(LC_LOAD_DYLINKER, "/usr/lib/dyld").len() + 24 + 24 + 24 + 16;
        size += dylibs.iter().map(|d| dylib_command(d).len()).sum::<usize>();
        size += rpaths.iter().map(|r| path_command(LC_RPATH, r).len()).sum::<usize>();
        size
    }
}

fn pad_name(name: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..name.len().min(16)].copy_from_slice(&name[..name.len().min(16)]);
    out
}

/// The sections with the index of their first indirect symbol.
fn sections_with_indirect(
    sections: &[&OutSection],
    stubs: &[usize],
    got: &[Target],
    stub_size: u64,
) -> Vec<(OutSection, u32, u32)> {
    sections
        .iter()
        .map(|s| {
            let (reserved1, reserved2) = match s.kind {
                S_SYMBOL_STUBS => (0, stub_size as u32),
                S_NON_LAZY_SYMBOL_POINTERS => (stubs.len() as u32, 0),
                _ => (0, 0),
            };
            let _ = got;
            ((*s).clone(), reserved1, reserved2)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn segment_command(
    name: &str,
    vmaddr: u64,
    vmsize: u64,
    fileoff: u64,
    filesize: u64,
    prot: u32,
    flags: u32,
    sections: &[(OutSection, u32, u32)],
) -> Vec<u8> {
    let mut c = Vec::new();
    c.extend_from_slice(&LC_SEGMENT_64.to_le_bytes());
    c.extend_from_slice(&((72 + 80 * sections.len()) as u32).to_le_bytes());
    c.extend_from_slice(&pad_name(name.as_bytes()));
    c.extend_from_slice(&vmaddr.to_le_bytes());
    c.extend_from_slice(&vmsize.to_le_bytes());
    c.extend_from_slice(&fileoff.to_le_bytes());
    c.extend_from_slice(&filesize.to_le_bytes());
    c.extend_from_slice(&(prot & 0xff).to_le_bytes());
    c.extend_from_slice(&(prot >> 8).to_le_bytes());
    c.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    c.extend_from_slice(&flags.to_le_bytes());
    for (s, reserved1, reserved2) in sections {
        c.extend_from_slice(&pad_name(&s.sectname));
        c.extend_from_slice(&pad_name(s.segname.as_bytes()));
        c.extend_from_slice(&s.addr.to_le_bytes());
        c.extend_from_slice(&s.size.to_le_bytes());
        c.extend_from_slice(&(s.offset as u32).to_le_bytes());
        c.extend_from_slice(&s.align.trailing_zeros().to_le_bytes());
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(&(s.kind as u32 | s.attributes).to_le_bytes());
        c.extend_from_slice(&reserved1.to_le_bytes());
        c.extend_from_slice(&reserved2.to_le_bytes());
        c.extend_from_slice(&0u32.to_le_bytes());
    }
    c
}

fn dyld_info(rebase_off: u64, rebase_size: usize, bind_off: u64, bind_size: usize) -> Vec<u8> {
    let mut c = Vec::new();
    for v in [
        LC_DYLD_INFO_ONLY,
        48,
        rebase_off as u32,
        rebase_size as u32,
        bind_off as u32,
        bind_size as u32,
        0,
        0,
        0,
        0,
        0,
        0,
    ] {
        c.extend_from_slice(&v.to_le_bytes());
    }
    c
}

fn symtab_command(symoff: u64, nsyms: u32, stroff: u64, strsize: u32) -> Vec<u8> {
    let mut c = Vec::new();
    for v in [LC_SYMTAB, 24, symoff as u32, nsyms, stroff as u32, strsize] {
        c.extend_from_slice(&v.to_le_bytes());
    }
    c
}

fn dysymtab_command(
    locals: (u32, u32),
    externals: (u32, u32),
    undefined: (u32, u32),
    indirect_off: u64,
    nindirect: u32,
) -> Vec<u8> {
    let mut c = Vec::new();
    let values = [
        LC_DYSYMTAB,
        80,
        locals.0,
        locals.1,
        externals.0,
        externals.1,
        undefined.0,
        undefined.1,
        0,
        0,
        0,
        0,
        0,
        0,
        indirect_off as u32,
        nindirect,
        0,
        0,
        0,
        0,
    ];
    for v in values {
        c.extend_from_slice(&v.to_le_bytes());
    }
    c
}

/// A command naming a path, padded to 8 bytes.
fn path_command(cmd: u32, path: &str) -> Vec<u8> {
    let size = (12 + path.len() + 1).next_multiple_of(8);
    let mut c = Vec::new();
    c.extend_from_slice(&cmd.to_le_bytes());
    c.extend_from_slice(&(size as u32).to_le_bytes());
    c.extend_from_slice(&12u32.to_le_bytes());
    c.extend_from_slice(path.as_bytes());
    c.resize(size, 0);
    c
}

fn dylib_command(name: &str) -> Vec<u8> {
    let size = (24 + name.len() + 1).next_multiple_of(8);
    let mut c = Vec::new();
    c.extend_from_slice(&LC_LOAD_DYLIB.to_le_bytes());
    c.extend_from_slice(&(size as u32).to_le_bytes());
    c.extend_from_slice(&24u32.to_le_bytes());
    // Timestamp, current and compatibility versions: any library will do.
    c.extend_from_slice(&2u32.to_le_bytes());
    c.extend_from_slice(&0x10000u32.to_le_bytes());
    c.extend_from_slice(&0x10000u32.to_le_bytes());
    c.extend_from_slice(name.as_bytes());
    c.resize(size, 0);
    c
}

fn build_version() -> Vec<u8> {
    let mut c = Vec::new();
    // macOS 11, with the SDK of macOS 14, and no tools.
    for v in [LC_BUILD_VERSION, 24, PLATFORM_MACOS, 0x000b_0000, 0x000e_0000, 0] {
        c.extend_from_slice(&v.to_le_bytes());
    }
    c
}

fn uuid(bytes: &[u8]) -> Vec<u8> {
    let digest = crate::hash::sha256(bytes);
    let mut c = Vec::new();
    c.extend_from_slice(&LC_UUID.to_le_bytes());
    c.extend_from_slice(&24u32.to_le_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    // A version 4 UUID, as the content gives it.
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    c.extend_from_slice(&id);
    c
}

/// An ad-hoc code signature, as a linker writes one: a code directory of
/// SHA-256 hashes of each 4 KiB page before it, in a superblob.
fn sign(file: &[u8], identifier: &[u8], text_offset: u64, text_size: u64, size: usize) -> Vec<u8> {
    let pages = file.len().div_ceil(4096);
    let directory_length = 88 + identifier.len() + 1 + 32 * pages;
    let mut blob = Vec::new();
    let be = |blob: &mut Vec<u8>, v: u32| blob.extend_from_slice(&v.to_be_bytes());
    be(&mut blob, 0xfade_0cc0);
    be(&mut blob, (12 + 8 + directory_length) as u32);
    be(&mut blob, 1);
    be(&mut blob, 0);
    be(&mut blob, 20);
    // The code directory.
    be(&mut blob, 0xfade_0c02);
    be(&mut blob, directory_length as u32);
    be(&mut blob, 0x20400);
    // Ad hoc, and signed by the linker.
    be(&mut blob, 0x2_0002);
    be(&mut blob, (88 + identifier.len() + 1) as u32);
    be(&mut blob, 88);
    be(&mut blob, 0);
    be(&mut blob, pages as u32);
    be(&mut blob, file.len() as u32);
    blob.extend_from_slice(&[32, 2, 0, 12]);
    be(&mut blob, 0);
    be(&mut blob, 0);
    be(&mut blob, 0);
    be(&mut blob, 0);
    blob.extend_from_slice(&0u64.to_be_bytes());
    blob.extend_from_slice(&text_offset.to_be_bytes());
    blob.extend_from_slice(&text_size.to_be_bytes());
    // The main executable.
    blob.extend_from_slice(&1u64.to_be_bytes());
    blob.extend_from_slice(identifier);
    blob.push(0);
    for page in file.chunks(4096) {
        blob.extend_from_slice(&crate::hash::sha256(page));
    }
    blob.resize(size, 0);
    blob
}

// What the parts of `__LINKEDIT` refer to.
impl Linker<'_> {
    /// The symbol table, its strings, the ranges of local, external and
    /// undefined symbols, and the indirect symbols of stubs and the GOT.
    #[allow(clippy::type_complexity)]
    fn symbol_table(&self, strip: bool) -> (Vec<u8>, Vec<u8>, (u32, u32), (u32, u32), (u32, u32), Vec<u32>) {
        let mut strtab = vec![b' ', 0];
        let mut entries: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut counts = [0u32; 3];
        let mut add = |entries: &mut [Vec<u8>; 3],
                       counts: &mut [u32; 3],
                       group: usize,
                       name: &[u8],
                       n_type: u8,
                       sect: u8,
                       desc: u16,
                       value: u64| {
            let at = strtab.len() as u32;
            strtab.extend_from_slice(name);
            strtab.push(0);
            let e = &mut entries[group];
            e.extend_from_slice(&at.to_le_bytes());
            e.push(n_type);
            e.push(sect);
            e.extend_from_slice(&desc.to_le_bytes());
            e.extend_from_slice(&value.to_le_bytes());
            counts[group] += 1;
        };
        let section_number: HashMap<usize, u8> = {
            let mut order: Vec<usize> =
                (0..self.outs.len()).filter(|&o| self.outs[o].size > 0 || self.outs[o].sectname == b"__text").collect();
            let segment_rank = |s: &str| match s {
                TEXT => 0,
                DATA_CONST => 1,
                _ => 2,
            };
            order.sort_by_key(|&o| {
                (
                    segment_rank(self.outs[o].segname),
                    rank(self.outs[o].segname, &self.outs[o].sectname, self.outs[o].kind),
                    o,
                )
            });
            order.into_iter().enumerate().map(|(n, o)| (o, (n + 1) as u8)).collect()
        };
        if !strip {
            for global in 0..self.globals.len() {
                if let Def::Atom { input, atom, .. } = self.globals[global].def
                    && let Some((out, _)) = self.inputs[input as usize].atoms[atom as usize].place
                {
                    let sect = section_number.get(&out).copied().unwrap_or(0);
                    add(
                        &mut entries,
                        &mut counts,
                        1,
                        self.globals[global].name,
                        N_SECT | N_EXT,
                        sect,
                        0,
                        self.global_address(global),
                    );
                }
            }
        }
        let mut undefined_index: HashMap<usize, u32> = HashMap::new();
        let first_undefined = counts[0] + counts[1];
        for &id in &self.imports {
            undefined_index.insert(id, first_undefined + counts[2]);
            let weak = if self.globals[id].required { 0 } else { N_WEAK_REF };
            // Looked up in every library loaded.
            add(
                &mut entries,
                &mut counts,
                2,
                self.globals[id].name,
                N_UNDF | N_EXT,
                0,
                weak | ((DYNAMIC_LOOKUP_ORDINAL as u16) << 8),
                0,
            );
        }
        let mut indirect = Vec::new();
        for &id in &self.stubs {
            indirect.push(undefined_index[&id]);
        }
        for target in &self.got {
            indirect.push(match target {
                Target::Global(id) if undefined_index.contains_key(id) => undefined_index[id],
                _ => INDIRECT_SYMBOL_LOCAL,
            });
        }
        let table = [entries[0].clone(), entries[1].clone(), entries[2].clone()].concat();
        strtab.resize(strtab.len().next_multiple_of(8), 0);
        (table, strtab, (0, counts[0]), (counts[0], counts[1]), (counts[0] + counts[1], counts[2]), indirect)
    }
}

/// A function's start, length and unwind encoding, with its personality
/// and LSDA.
type UnwindEntry = (u64, u32, u32, Option<Target>, Option<Target>);

// Unwinding: `__eh_frame` keeps the descriptions of live functions, and
// `__unwind_info` indexes every live function's way to unwind.
struct Frame {
    input: usize,
    start: u64,
    len: u64,
    cie: Option<u64>,
    function: Option<(u32, usize)>,
}

impl Linker<'_> {
    /// The records of `__eh_frame` that stay, and their size.
    fn eh_frame_records(&self) -> (Vec<Frame>, u64) {
        let (all, _) = self.eh_frame_records_all();
        let mut out = Vec::new();
        let mut by_input: HashMap<usize, Vec<Frame>> = HashMap::new();
        for frame in all {
            by_input.entry(frame.input).or_default().push(frame);
        }
        let mut inputs: Vec<usize> = by_input.keys().copied().collect();
        inputs.sort();
        for i in inputs {
            let records = by_input.remove(&i).unwrap();
            let live = |f: &Frame| f.function.is_some_and(|(input, atom)| self.inputs[input as usize].atoms[atom].live);
            let cies: HashSet<u64> = records.iter().filter(|r| live(r)).filter_map(|r| r.cie).collect();
            out.extend(records.into_iter().filter(|r| live(r) || (r.cie.is_none() && cies.contains(&r.start))));
        }
        let size: u64 = out.iter().map(|r| r.len).sum();
        (out, size)
    }

    /// Every record of every `__eh_frame`, with the function it describes.
    fn eh_frame_records_all(&self) -> (Vec<Frame>, u64) {
        let mut out = Vec::new();
        for (i, input) in self.inputs.iter().enumerate() {
            let Some(s) = input.sections.iter().position(|s| s.sectname == b"__eh_frame") else { continue };
            let section = &input.sections[s];
            let data = section.data;
            let mut records = Vec::new();
            let mut at = 0u64;
            while at + 8 <= data.len() as u64 {
                let word = |o: u64| u32::from_le_bytes(data[o as usize..o as usize + 4].try_into().unwrap());
                let length = word(at) as u64;
                if length == 0 || length == 0xffff_ffff {
                    break;
                }
                let id = word(at + 4) as u64;
                let cie = (id != 0).then(|| at + 4 - id);
                // The function: the relocated or pc-relative start field.
                let function = cie.and_then(|_| {
                    let reloc = section.relocations_in(at + 8, at + 9).next();
                    let address = match reloc {
                        Some(r) => match self.reloc_target(i, s, r) {
                            Some(t) => return self.atom_of(t),
                            None => return None,
                        },
                        None => {
                            (section.addr as i64
                                + at as i64
                                + 8
                                + i32::from_le_bytes(data[(at + 8) as usize..(at + 12) as usize].try_into().unwrap())
                                    as i64) as u64
                        }
                    };
                    let target = (0..input.sections.len()).find(|&n| {
                        let t = &input.sections[n];
                        address >= t.addr && address < t.addr + t.size && t.sectname != b"__eh_frame"
                    })?;
                    self.atom_of(Target::Local { input: i as u32, section: target as u32, address })
                });
                records.push(Frame { input: i, start: at, len: length + 4, cie, function });
                at += length + 4;
            }
            out.extend(records);
        }
        (out, 0)
    }

    /// Writes the kept records, each description pointing to where its CIE
    /// now is and to where its function went; gives each function's
    /// description offset in the section.
    fn write_eh_frame(
        &self,
        image: &mut Image,
        frames: &[Frame],
        eh: usize,
        rebases: &mut Vec<u64>,
        binds: &mut Vec<(u64, usize, i64)>,
    ) -> Result<HashMap<(u32, usize), u64>, String> {
        let (file, base) = (self.outs[eh].offset, self.outs[eh].addr);
        let mut out = 0u64;
        let mut moved: HashMap<(usize, u64), u64> = HashMap::new();
        let mut fdes = HashMap::new();
        for frame in frames {
            let input = &self.inputs[frame.input];
            let s = input.sections.iter().position(|s| s.sectname == b"__eh_frame").unwrap();
            let section = &input.sections[s];
            moved.insert((frame.input, frame.start), out);
            image.put(file + out, &section.data[frame.start as usize..(frame.start + frame.len) as usize]);
            if let Some(cie) = frame.cie {
                let cie_at = moved
                    .get(&(frame.input, cie))
                    .copied()
                    .ok_or_else(|| error(&input.name, "a description before its CIE"))?;
                image.u32(file + out + 4, ((out + 4) - cie_at) as u32);
                if let Some(function) = frame.function {
                    fdes.insert(function, out);
                }
            }
            // Its relocations, at their new place; the start field of a
            // description without one is pc-relative and moved with it.
            let relocations: Vec<Reloc> =
                section.relocations_in(frame.start, frame.start + frame.len).copied().collect();
            let shift = out as i64 - frame.start as i64;
            for reloc in &relocations {
                self.apply(
                    image,
                    frame.input,
                    s,
                    reloc,
                    (file as i64 + shift) as u64,
                    (base as i64 + shift) as u64,
                    rebases,
                    binds,
                )?;
            }
            if frame.cie.is_some()
                && !relocations.iter().any(|r| r.offset == frame.start + 8)
                && let Some((input, atom)) = frame.function
            {
                let a = &self.inputs[input as usize].atoms[atom];
                let old = section.addr as i64 + frame.start as i64 + 8 + image.read32(file + out + 8) as i32 as i64;
                let target =
                    self.address(Target::Local { input, section: a.section as u32, address: old as u64 }) as i64;
                image.u32(file + out + 8, (target - (base + out + 8) as i64) as u32);
            }
            out += frame.len;
        }
        Ok(fdes)
    }

    /// Each live function's start, length and unwind encoding, with its
    /// personality and LSDA.
    fn unwind_entries(&self, fde_at: &HashMap<(u32, usize), u64>) -> Vec<UnwindEntry> {
        let mut entries = Vec::new();
        let mut covered: HashSet<(u32, usize)> = HashSet::new();
        for i in 0..self.inputs.len() {
            for entry in self.compact_entries(i) {
                let Some(function) = entry.function else { continue };
                let a = &self.inputs[function.0 as usize].atoms[function.1];
                if !a.live {
                    continue;
                }
                let start =
                    self.address(Target::Local { input: function.0, section: a.section as u32, address: a.start })
                        + entry.offset;
                let dwarf = match self.arch {
                    Arch::Arm64 => entry.encoding & 0x0f00_0000 == 0x0300_0000,
                    Arch::X86_64 => entry.encoding & 0x0f00_0000 == 0x0400_0000,
                };
                let encoding = match (dwarf, fde_at.get(&function)) {
                    (true, Some(&fde)) => (entry.encoding & 0xff00_0000) | (fde as u32 & 0x00ff_ffff),
                    _ => entry.encoding,
                };
                covered.insert(function);
                entries.push((start, entry.length, encoding, entry.personality, entry.lsda));
            }
        }
        // Functions only `__eh_frame` describes.
        for (&function, &fde) in fde_at {
            if covered.contains(&function) {
                continue;
            }
            let a = &self.inputs[function.0 as usize].atoms[function.1];
            let start = self.address(Target::Local { input: function.0, section: a.section as u32, address: a.start });
            let mode = match self.arch {
                Arch::Arm64 => 0x0300_0000,
                Arch::X86_64 => 0x0400_0000,
            };
            entries.push((start, (a.end - a.start) as u32, mode | (fde as u32 & 0x00ff_ffff), None, None));
        }
        entries.sort_by_key(|e| e.0);
        entries
    }

    fn unwind_info_size(&self, frames: &[Frame]) -> u64 {
        // The descriptions' offsets do not change the size.
        let fake: HashMap<(u32, usize), u64> = frames.iter().filter_map(|f| f.function.map(|fun| (fun, 0))).collect();
        self.unwind_info(frames, &fake).len() as u64
    }

    /// `__unwind_info`: a first-level index over pages of regular entries,
    /// with the personalities and the LSDAs of the functions that have
    /// them.
    fn unwind_info(&self, _frames: &[Frame], fde_at: &HashMap<(u32, usize), u64>) -> Vec<u8> {
        let entries = self.unwind_entries(fde_at);
        let mut personalities: Vec<Target> = Vec::new();
        let mut encoded: Vec<(u32, u32)> = Vec::new();
        let mut lsdas: Vec<(u32, u32)> = Vec::new();
        for &(start, _, encoding, personality, lsda) in &entries {
            let function = start.wrapping_sub(BASE) as u32;
            let mut encoding = encoding;
            if let Some(personality) = personality {
                let index = match personalities.iter().position(|&p| p == personality) {
                    Some(index) => index,
                    None => {
                        personalities.push(personality);
                        personalities.len() - 1
                    }
                };
                encoding = (encoding & !0x3000_0000) | (((index + 1) as u32) << 28);
            }
            if let Some(lsda) = lsda {
                encoding |= 0x4000_0000;
                lsdas.push((function, self.address(lsda).wrapping_sub(BASE) as u32));
            }
            encoded.push((function, encoding));
        }
        const PER_PAGE: usize = 511;
        let pages: Vec<&[(u32, u32)]> = encoded.chunks(PER_PAGE).collect();
        let header = 28u32;
        let personality_offset = header;
        let index_offset = personality_offset + 4 * personalities.len() as u32;
        let index_count = pages.len() as u32 + 1;
        let lsda_offset = index_offset + 12 * index_count;
        let pages_offset = lsda_offset + 8 * lsdas.len() as u32;
        let mut out = Vec::new();
        let w = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        w(&mut out, 1);
        w(&mut out, header);
        w(&mut out, 0);
        w(&mut out, personality_offset);
        w(&mut out, personalities.len() as u32);
        w(&mut out, index_offset);
        w(&mut out, index_count);
        for &personality in &personalities {
            let slot =
                self.slots.get(&personality).map_or(0, |_| self.slot_address(personality).wrapping_sub(BASE) as u32);
            w(&mut out, slot);
        }
        let mut page_offset = pages_offset;
        let mut lsda_index = 0usize;
        for page in &pages {
            w(&mut out, page[0].0);
            w(&mut out, page_offset);
            w(&mut out, lsda_offset + 8 * lsda_index as u32);
            lsda_index += lsdas.iter().filter(|l| l.0 >= page[0].0 && l.0 <= page.last().unwrap().0).count();
            page_offset += 8 + 8 * page.len() as u32;
        }
        let end = entries.last().map_or(0, |e| (e.0.wrapping_sub(BASE) as u32).wrapping_add(e.1));
        w(&mut out, end);
        w(&mut out, 0);
        w(&mut out, lsda_offset + 8 * lsdas.len() as u32);
        for (function, lsda) in &lsdas {
            w(&mut out, *function);
            w(&mut out, *lsda);
        }
        for page in &pages {
            w(&mut out, 2);
            out.extend_from_slice(&8u16.to_le_bytes());
            out.extend_from_slice(&(page.len() as u16).to_le_bytes());
            for (function, encoding) in page.iter() {
                w(&mut out, *function);
                w(&mut out, *encoding);
            }
        }
        out
    }
}
