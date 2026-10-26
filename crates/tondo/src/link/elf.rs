//! Links ELF executables and shared libraries: the program's objects and
//! the archives of the runtime and of native libraries, whose members come
//! in as they are needed, against shared libraries the system has
//! installed. The output is position independent, so the system loads it
//! anywhere: when it starts, its own addresses are moved to where it is,
//! packed as `DT_RELR` where the C library reads that, and what shared
//! libraries define is bound, all at once. A static executable relocates
//! itself. Sections nothing reaches are dropped, thread-local accesses are
//! relaxed to an executable's own model or, in a shared library, ask the
//! loader for the library's block, and `.eh_frame_hdr` indexes the unwind
//! tables.

use crate::hash::{FastMap, FastSet};
use object::elf::*;
use object::read::elf::{ElfFile64, SectionHeader as _};
use object::{
    Endianness, Object, ObjectComdat, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SymbolIndex,
    SymbolSection,
};
use std::collections::{HashMap, HashSet};

#[derive(Copy, Clone, PartialEq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    fn machine(self) -> Machine {
        match self {
            Arch::X86_64 => EM_X86_64,
            Arch::Aarch64 => EM_AARCH64,
        }
    }

    /// The largest page the system may use: segments start on one.
    fn page(self) -> u64 {
        match self {
            Arch::X86_64 => 0x1000,
            Arch::Aarch64 => 0x10000,
        }
    }

    fn glob_dat(self) -> RelocationType {
        match self {
            Arch::X86_64 => R_X86_64_GLOB_DAT,
            Arch::Aarch64 => R_AARCH64_GLOB_DAT,
        }
    }

    fn relative(self) -> RelocationType {
        match self {
            Arch::X86_64 => R_X86_64_RELATIVE,
            Arch::Aarch64 => R_AARCH64_RELATIVE,
        }
    }

    /// The offset from the thread pointer, which the loader writes.
    fn tp_off(self) -> RelocationType {
        match self {
            Arch::X86_64 => R_X86_64_TPOFF64,
            Arch::Aarch64 => R_AARCH64_TLS_TPREL,
        }
    }
}

/// Where the executable would be if it were not moved: anywhere will do.
const BASE: u64 = 0;
const PLT_ENTRY: u64 = 16;

/// What the executable links.
pub struct Inputs<'a> {
    pub arch: Arch,
    /// Objects, which come in whole.
    pub objects: Vec<(String, &'a [u8])>,
    /// Archives, whose members come in when they define what is missing.
    pub archives: Vec<(String, &'a [u8])>,
    /// Shared libraries, searched in order for what the rest leaves
    /// undefined; the first ones are always needed.
    pub shared: Vec<(String, &'a [u8])>,
    pub always_needed: usize,
    /// Where the executable looks for shared libraries besides the
    /// system's (`DT_RUNPATH`).
    pub runpath: Vec<String>,
    /// The dynamic loader; none for a static executable.
    pub interpreter: Option<String>,
    pub entry: String,
    /// A shared library instead of an executable: the name it is loaded by
    /// and the symbols it publishes, which keep the rest.
    pub library: Option<(String, Vec<String>)>,
    /// Without a symbol table.
    pub strip: bool,
}

/// The parts of the executable, in the order they are laid out: what is
/// read only, then code, then what is written.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
enum Out {
    Interp,
    Hash,
    Dynsym,
    Dynstr,
    Versym,
    Verneed,
    RelaDyn,
    Relr,
    Rodata,
    EhFrameHdr,
    EhFrame,
    Plt,
    Text,
    Tdata,
    Tbss,
    InitArray,
    FiniArray,
    DataRelRo,
    Dynamic,
    Got,
    Data,
    Bss,
}

const OUTS: [Out; 22] = [
    Out::Interp,
    Out::Hash,
    Out::Dynsym,
    Out::Dynstr,
    Out::Versym,
    Out::Verneed,
    Out::RelaDyn,
    Out::Relr,
    Out::Rodata,
    Out::EhFrameHdr,
    Out::EhFrame,
    Out::Plt,
    Out::Text,
    Out::Tdata,
    Out::Tbss,
    Out::InitArray,
    Out::FiniArray,
    Out::DataRelRo,
    Out::Dynamic,
    Out::Got,
    Out::Data,
    Out::Bss,
];

impl Out {
    fn name(self) -> &'static str {
        match self {
            Out::Interp => ".interp",
            Out::Hash => ".hash",
            Out::Dynsym => ".dynsym",
            Out::Dynstr => ".dynstr",
            Out::Versym => ".gnu.version",
            Out::Verneed => ".gnu.version_r",
            Out::RelaDyn => ".rela.dyn",
            Out::Relr => ".relr.dyn",
            Out::Rodata => ".rodata",
            Out::EhFrameHdr => ".eh_frame_hdr",
            Out::EhFrame => ".eh_frame",
            Out::Plt => ".plt",
            Out::Text => ".text",
            Out::Tdata => ".tdata",
            Out::Tbss => ".tbss",
            Out::InitArray => ".init_array",
            Out::FiniArray => ".fini_array",
            Out::DataRelRo => ".data.rel.ro",
            Out::Dynamic => ".dynamic",
            Out::Got => ".got",
            Out::Data => ".data",
            Out::Bss => ".bss",
        }
    }

    /// 0 read only, 1 code, 2 written.
    fn segment(self) -> usize {
        match self {
            Out::Plt | Out::Text => 1,
            out if out >= Out::Tdata => 2,
            _ => 0,
        }
    }

    fn nobits(self) -> bool {
        matches!(self, Out::Tbss | Out::Bss)
    }

    fn header(self) -> (u32, u64) {
        let (kind, flags) = match self {
            Out::Interp | Out::Rodata | Out::EhFrameHdr => (SHT_PROGBITS, SHF_ALLOC),
            Out::EhFrame => (SHT_PROGBITS, SHF_ALLOC),
            Out::Hash => (SHT_HASH, SHF_ALLOC),
            Out::Dynsym => (SHT_DYNSYM, SHF_ALLOC),
            Out::Dynstr => (SHT_STRTAB, SHF_ALLOC),
            Out::Versym => (SHT_GNU_VERSYM, SHF_ALLOC),
            Out::Verneed => (SHT_GNU_VERNEED, SHF_ALLOC),
            Out::RelaDyn => (SHT_RELA, SHF_ALLOC),
            Out::Relr => (SHT_RELR, SHF_ALLOC),
            Out::Plt | Out::Text => (SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR),
            Out::Tdata => (SHT_PROGBITS, SHF_ALLOC | SHF_WRITE | SHF_TLS),
            Out::Tbss => (SHT_NOBITS, SHF_ALLOC | SHF_WRITE | SHF_TLS),
            Out::InitArray => (SHT_INIT_ARRAY, SHF_ALLOC | SHF_WRITE),
            Out::FiniArray => (SHT_FINI_ARRAY, SHF_ALLOC | SHF_WRITE),
            Out::Dynamic => (SHT_DYNAMIC, SHF_ALLOC | SHF_WRITE),
            Out::DataRelRo | Out::Got | Out::Data => (SHT_PROGBITS, SHF_ALLOC | SHF_WRITE),
            Out::Bss => (SHT_NOBITS, SHF_ALLOC | SHF_WRITE),
        };
        (kind.0, flags.0)
    }
}

/// Where the content of an input section goes.
fn output_of(name: &[u8], kind: SectionType, flags: SectionFlags) -> Option<Out> {
    let starts = |prefix: &[u8]| name == prefix || name.starts_with(&[prefix, b"."].concat());
    let has = |flag: SectionFlags| flags.0 & flag.0 != 0;
    if !has(SHF_ALLOC) || kind == SHT_NOTE || kind == SHT_GROUP {
        return None;
    }
    Some(match () {
        _ if name == b".eh_frame" => Out::EhFrame,
        _ if has(SHF_TLS) => {
            if kind == SHT_NOBITS {
                Out::Tbss
            } else {
                Out::Tdata
            }
        }
        _ if starts(b".init_array") || starts(b".ctors") => Out::InitArray,
        _ if starts(b".fini_array") || starts(b".dtors") => Out::FiniArray,
        _ if starts(b".data.rel.ro") => Out::DataRelRo,
        _ if has(SHF_EXECINSTR) => Out::Text,
        _ if kind == SHT_NOBITS => Out::Bss,
        _ if has(SHF_WRITE) => Out::Data,
        _ => Out::Rodata,
    })
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum Def {
    Undefined,
    Section {
        input: u32,
        section: u32,
        value: u64,
    },
    Absolute(u64),
    /// A C tentative definition, which `.bss` holds.
    Common {
        size: u64,
        align: u64,
    },
    Shared {
        library: u32,
    },
    Linker(Synthetic),
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum Synthetic {
    InitArrayStart,
    InitArrayEnd,
    FiniArrayStart,
    FiniArrayEnd,
    GlobalOffsetTable,
    Dynamic,
    /// The ELF header, where the executable is loaded.
    HeaderStart,
}

fn synthetic(name: &[u8]) -> Option<Synthetic> {
    Some(match name {
        b"__init_array_start" => Synthetic::InitArrayStart,
        b"__init_array_end" => Synthetic::InitArrayEnd,
        b"__fini_array_start" => Synthetic::FiniArrayStart,
        b"__fini_array_end" => Synthetic::FiniArrayEnd,
        b"_GLOBAL_OFFSET_TABLE_" => Synthetic::GlobalOffsetTable,
        b"_DYNAMIC" => Synthetic::Dynamic,
        b"__ehdr_start" => Synthetic::HeaderStart,
        _ => return None,
    })
}

struct Global<'a> {
    name: &'a [u8],
    def: Def,
    /// The definition may give way to another that is not weak.
    weak: bool,
    /// Some reference is not weak.
    required: bool,
    /// Some live code or data refers to it.
    used: bool,
    plt: Option<u32>,
    dynsym: Option<u32>,
    common: u64,
}

/// A symbol of a shared library: its kind, and its default version.
#[derive(Copy, Clone)]
struct Export<'a> {
    kind: SymbolType,
    version: Option<&'a [u8]>,
}

struct Library<'a> {
    soname: String,
    exports: FastMap<&'a [u8], Export<'a>>,
    needed: bool,
    /// Whether it defines `GLIBC_ABI_DT_RELR`: its loader reads `DT_RELR`.
    relr: bool,
}

/// A record of an input's `.eh_frame`: where it is, how long, and for a
/// description of a function, its CIE and the section of the function.
struct Record {
    start: u64,
    len: u64,
    cie: Option<u64>,
    function: Option<u32>,
}

struct Input<'a> {
    name: String,
    file: ElfFile64<'a, Endianness>,
    /// The output and offset of each section that is laid out.
    place: Vec<Option<(Out, u64)>>,
    dropped: Vec<bool>,
    live: Vec<bool>,
    /// The global each symbol is, by index; `None` for local ones.
    globals: Vec<Option<usize>>,
    eh_frame: Option<(u32, Vec<Record>)>,
    /// Each function section's records in `.eh_frame`.
    fdes: HashMap<u32, Vec<usize>>,
    /// The relocations of the sections read so far, by offset.
    relocations: HashMap<u32, Vec<Relocation>>,
}

/// What a relocation refers to.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Target {
    Global(usize),
    Local { input: u32, symbol: u32 },
}

/// What an entry of the GOT holds: an address, or a thread-local offset;
/// in a shared library, the pairs the loader fills for its thread-local
/// block: its module and an offset, or a TLS descriptor.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Slot {
    Address(Target),
    TpOff(Target),
    Module(Option<Target>),
    DtpOff(Option<Target>),
    Descriptor(Target),
    DescriptorArgument(Target),
}

struct Relocation {
    offset: u64,
    kind: RelocationType,
    target: Option<Target>,
    addend: i64,
}

struct Linker<'a> {
    arch: Arch,
    inputs: Vec<Input<'a>>,
    globals: Vec<Global<'a>>,
    names: FastMap<&'a [u8], usize>,
    groups: FastSet<&'a [u8]>,
    libraries: Vec<Library<'a>>,
    got: Vec<Slot>,
    slots: HashMap<Slot, u32>,
    plt: Vec<usize>,
    dynsyms: Vec<usize>,
    /// Data words that point into shared libraries, by input and section.
    dynamic_words: usize,
    /// Each output's address, file offset and size.
    addr: [u64; OUTS.len()],
    offset: [u64; OUTS.len()],
    size: [u64; OUTS.len()],
    align: [u64; OUTS.len()],
    tls: (u64, u64, u64),
    /// Whether shared libraries load it, or it is static.
    dynamic: bool,
    /// The words holding the executable's own addresses, which move with
    /// it: those `DT_RELR` can pack, and the others.
    aligned_words: usize,
    unaligned_words: usize,
    /// Whether the C library reads `DT_RELR`.
    relr: bool,
    /// A shared library, not an executable.
    library: bool,
}

fn error(input: &str, message: impl std::fmt::Display) -> String {
    format!("linking {input}: {message}")
}

/// Links the executable, whose bytes it gives.
pub fn link(inputs: Inputs) -> Result<Vec<u8>, String> {
    let mut linker = Linker {
        arch: inputs.arch,
        inputs: Vec::new(),
        globals: Vec::new(),
        names: FastMap::default(),
        groups: FastSet::default(),
        libraries: Vec::new(),
        got: Vec::new(),
        slots: HashMap::new(),
        plt: Vec::new(),
        dynsyms: Vec::new(),
        dynamic_words: 0,
        addr: [0; OUTS.len()],
        offset: [0; OUTS.len()],
        size: [0; OUTS.len()],
        align: [1; OUTS.len()],
        tls: (0, 0, 1),
        dynamic: true,
        aligned_words: 0,
        unaligned_words: 0,
        relr: false,
        library: inputs.library.is_some(),
    };
    // What the output starts from: the entry, or what a library publishes.
    let roots: Vec<usize> = match &inputs.library {
        Some((_, exports)) => exports.iter().map(|name| linker.intern(name.as_bytes())).collect(),
        None => vec![linker.intern(inputs.entry.as_bytes())],
    };
    for &root in &roots {
        linker.globals[root].required = true;
    }
    for (name, data) in &inputs.objects {
        linker.add(name.clone(), data)?;
    }
    linker.archives(&inputs.archives)?;
    for (index, (name, data)) in inputs.shared.iter().enumerate() {
        let mut library = library(name, data)?;
        library.needed = index < inputs.always_needed;
        linker.libraries.push(library);
    }
    linker.dynamic = inputs.interpreter.is_some() || linker.library;
    linker.relr = linker.dynamic && linker.libraries.iter().any(|l| l.soname == "libc.so.6" && l.relr);
    linker.resolve()?;
    linker.collect(&roots);
    if linker.library {
        for &root in &roots {
            if !matches!(linker.globals[root].def, Def::Section { .. }) {
                let name = String::from_utf8_lossy(linker.globals[root].name);
                return Err(format!("the library does not define `{name}`, which it publishes"));
            }
            linker.dynsyms.push(root);
            linker.globals[root].dynsym = Some(linker.dynsyms.len() as u32);
        }
    }
    linker.scan()?;
    linker.layout(&inputs);
    linker.write(&inputs, roots[0])
}

impl<'a> Linker<'a> {
    fn intern(&mut self, name: &'a [u8]) -> usize {
        if let Some(&id) = self.names.get(name) {
            return id;
        }
        self.globals.push(Global {
            name,
            def: Def::Undefined,
            weak: false,
            required: false,
            used: false,
            plt: None,
            dynsym: None,
            common: 0,
        });
        self.names.insert(name, self.globals.len() - 1);
        self.globals.len() - 1
    }

    /// Adds an object: its sections, and the globals it defines and needs.
    fn add(&mut self, name: String, data: &'a [u8]) -> Result<Vec<usize>, String> {
        let parsed = parse(self.arch, name, data)?;
        self.take(parsed)
    }

    /// Takes in an object `parse` read: what it defines and needs.
    fn take(&mut self, parsed: Parsed<'a>) -> Result<Vec<usize>, String> {
        let Parsed { name, file, comdats, eh_frame, symbols, count: symbol_count } = parsed;
        let count = file.elf_section_table().len();
        let mut dropped = vec![false; count];
        for (signature, sections) in comdats {
            if !self.groups.insert(signature) {
                for section in sections {
                    dropped[section] = true;
                }
            }
        }
        let index = self.inputs.len() as u32;
        let mut globals = vec![None; symbol_count + 1];
        let mut undefined = Vec::new();
        for symbol in symbols {
            let symbol_name = symbol.name;
            let id = self.intern(symbol_name);
            globals[symbol.index] = Some(id);
            let weak = symbol.weak;
            let def = match symbol.section {
                SymbolSection::Section(section) if !dropped[section.0] => {
                    Def::Section { input: index, section: section.0 as u32, value: symbol.address }
                }
                SymbolSection::Absolute => Def::Absolute(symbol.address),
                // A tentative definition keeps its alignment in its value.
                SymbolSection::Common => Def::Common { size: symbol.size, align: symbol.address.max(1) },
                _ => {
                    // `__tls_get_addr` is called only by accesses that
                    // linking relaxes.
                    if !weak && symbol_name != b"__tls_get_addr" {
                        self.globals[id].required = true;
                    }
                    if self.globals[id].def == Def::Undefined {
                        undefined.push(id);
                    }
                    continue;
                }
            };
            let global = &mut self.globals[id];
            let replaces = match global.def {
                Def::Undefined => true,
                Def::Common { size, .. } => !matches!(def, Def::Common { size: other, .. } if other <= size),
                _ => global.weak && !weak,
            };
            if replaces {
                global.def = def;
                global.weak = weak;
            }
        }
        // The unwind descriptions of a group taken from another object go
        // with it.
        let (eh_frame, fdes) = match eh_frame {
            Some((section, records, fdes)) if !dropped[section as usize] => (Some((section, records)), fdes),
            _ => (None, HashMap::new()),
        };
        let input = Input {
            name,
            file,
            place: vec![None; count],
            dropped,
            live: vec![false; count],
            globals,
            eh_frame,
            fdes,
            relocations: HashMap::new(),
        };
        self.inputs.push(input);
        Ok(undefined)
    }

    /// Brings in the members of archives that define what is undefined, as
    /// long as some do.
    fn archives(&mut self, archives: &[(String, &'a [u8])]) -> Result<(), String> {
        let mut index: FastMap<&'a [u8], (usize, u64)> = FastMap::default();
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
        // Room for the names the archives may bring.
        self.names.reserve(index.len());
        self.globals.reserve(index.len());
        let mut loaded: HashSet<(usize, u64)> = HashSet::new();
        let mut pending: Vec<usize> =
            (0..self.globals.len()).filter(|&id| self.globals[id].def == Def::Undefined).collect();
        // In rounds: the members that define what is needed now are read
        // on every core, and taken in, in the order of the archives, while
        // what brought each in is still undefined.
        loop {
            let mut wanted: Vec<((usize, u64), usize)> = Vec::new();
            for id in pending.drain(..) {
                let global = &self.globals[id];
                if global.def != Def::Undefined || !global.required {
                    continue;
                }
                let Some(&member) = index.get(global.name) else { continue };
                if loaded.insert(member) {
                    wanted.push((member, id));
                }
            }
            if wanted.is_empty() {
                break;
            }
            wanted.sort_unstable();
            let read = crate::parallel::map(&wanted, 4, |&((a, offset), _)| {
                let (archive_name, data) = &archives[a];
                let member = parsed[a]
                    .member(object::read::archive::ArchiveOffset(offset))
                    .map_err(|e| error(archive_name, e))?;
                let bytes = member.data(*data).map_err(|e| error(archive_name, e))?;
                parse(self.arch, format!("{archive_name}({})", String::from_utf8_lossy(member.name())), bytes)
            });
            for ((member, id), read) in wanted.into_iter().zip(read) {
                if self.globals[id].def != Def::Undefined {
                    // Another member of this round defined it first.
                    loaded.remove(&member);
                    continue;
                }
                pending.extend(self.take(read?)?);
            }
        }
        Ok(())
    }

    /// Resolves what is left to shared libraries and to the linker's own
    /// symbols; what no one defines and is required is an error.
    fn resolve(&mut self) -> Result<(), String> {
        let mut missing = Vec::new();
        for id in 0..self.globals.len() {
            if self.globals[id].def != Def::Undefined {
                continue;
            }
            let name = self.globals[id].name;
            if let Some(synthetic) = synthetic(name) {
                self.globals[id].def = Def::Linker(synthetic);
                continue;
            }
            let found = self.libraries.iter().position(|l| l.exports.contains_key(name));
            match found {
                Some(library) => {
                    self.globals[id].def = Def::Shared { library: library as u32 };
                }
                None if self.globals[id].required => missing.push(String::from_utf8_lossy(name).into_owned()),
                None => {}
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        missing.sort();
        Err(format!("undefined: {}", missing.join(" ")))
    }

    /// The symbols nothing references keep their sections out: marks what
    /// the roots, the constructors and what they reach need.
    fn collect(&mut self, roots: &[usize]) {
        let mut work: Vec<(u32, u32)> = Vec::new();
        for &root in roots {
            if let Def::Section { input, section, .. } = self.globals[root].def {
                work.push((input, section));
            }
        }
        // Constructors, destructors and what asks to be kept, in every
        // input, looked for on every core.
        let indices: Vec<usize> = (0..self.inputs.len()).collect();
        let kept = crate::parallel::map(&indices, 1, |&i| {
            let input = &self.inputs[i];
            let endian = input.file.endian();
            input
                .file
                .sections()
                .filter(|section| {
                    let header = section.elf_section_header();
                    let (kind, flags) = (header.sh_type(endian), header.sh_flags(endian));
                    let name = section.name_bytes().unwrap_or_default();
                    let root = matches!(output_of(name, kind, flags), Some(Out::InitArray | Out::FiniArray))
                        || flags.0 & SHF_GNU_RETAIN.0 != 0;
                    root && !input.dropped[section.index().0]
                })
                .map(|section| (i as u32, section.index().0 as u32))
                .collect::<Vec<_>>()
        });
        work.extend(kept.into_iter().flatten());
        // In waves: the relocations of the sections reached last, and of
        // their unwind descriptions, are read on every core.
        while !work.is_empty() {
            let mut fresh = Vec::new();
            for (input, section) in work.drain(..) {
                if !std::mem::replace(&mut self.inputs[input as usize].live[section as usize], true) {
                    fresh.push((input, section));
                }
            }
            let mut unread: Vec<(u32, u32)> = Vec::new();
            for &(input, section) in &fresh {
                let i = &self.inputs[input as usize];
                unread.push((input, section));
                if i.fdes.contains_key(&section)
                    && let Some((frames, _)) = &i.eh_frame
                {
                    unread.push((input, *frames));
                }
            }
            unread.sort_unstable();
            unread.dedup();
            unread.retain(|(input, section)| !self.inputs[*input as usize].relocations.contains_key(section));
            let read = crate::parallel::map(&unread, 64, |&(input, section)| {
                read_relocations(&self.inputs[input as usize], input as usize, section)
            });
            for ((input, section), relocations) in unread.into_iter().zip(read) {
                self.inputs[input as usize].relocations.insert(section, relocations);
            }
            for (input, section) in fresh {
                let i = input as usize;
                let mut targets: Vec<Target> = self.relocations(i, section).iter().filter_map(|r| r.target).collect();
                // A function's unwind description, and what it and its CIE
                // refer to besides the function.
                if let Some(records) = self.inputs[i].fdes.get(&section)
                    && let Some((frames, all)) = self.inputs[i].eh_frame.as_ref()
                {
                    let relocations = self.relocations(i, *frames);
                    for &record in records {
                        let fde = &all[record];
                        let cie = fde.cie.and_then(|cie| all.binary_search_by_key(&cie, |r| r.start).ok());
                        for r in std::iter::once(fde).chain(cie.map(|cie| &all[cie])) {
                            let first = relocations.partition_point(|x| x.offset < r.start);
                            for reloc in relocations[first..].iter().take_while(|x| x.offset < r.start + r.len) {
                                if reloc.offset != fde.start + 8 {
                                    targets.extend(reloc.target);
                                }
                            }
                        }
                    }
                }
                for target in targets {
                    match target {
                        Target::Global(id) => {
                            self.globals[id].used = true;
                            if let Def::Section { input, section, .. } = self.globals[id].def {
                                work.push((input, section));
                            }
                        }
                        Target::Local { input, symbol } => {
                            let file = &self.inputs[input as usize].file;
                            if let Ok(symbol) = file.symbol_by_index(SymbolIndex(symbol as usize))
                                && let SymbolSection::Section(section) = symbol.section()
                            {
                                work.push((input, section.0 as u32));
                            }
                        }
                    }
                }
            }
        }
        for &root in roots {
            self.globals[root].used = true;
        }
    }

    /// The relocations of a section that `collect` read, by offset.
    fn relocations(&self, input: usize, section: u32) -> &[Relocation] {
        self.inputs[input].relocations.get(&section).map_or(&[], Vec::as_slice)
    }
}

/// The relocations of a section of `input`, the `index`th, by offset.
fn read_relocations(input_ref: &Input<'_>, input: usize, section: u32) -> Vec<Relocation> {
    {
        let Ok(section_ref) = input_ref.file.section_by_index(object::SectionIndex(section as usize)) else {
            return Vec::new();
        };
        let mut out: Vec<Relocation> = section_ref
            .relocations()
            .map(|(offset, r)| {
                let kind = match r.flags() {
                    RelocationFlags::Elf { r_type } => r_type,
                    _ => R_X86_64_NONE,
                };
                let target = match r.target() {
                    RelocationTarget::Symbol(symbol) => {
                        Some(match input_ref.globals.get(symbol.0).copied().flatten() {
                            Some(id) => Target::Global(id),
                            None => Target::Local { input: input as u32, symbol: symbol.0 as u32 },
                        })
                    }
                    _ => None,
                };
                Relocation { offset, kind, target, addend: r.addend() }
            })
            .collect();
        out.sort_by_key(|r| r.offset);
        out
    }
}

impl<'a> Linker<'a> {
    /// Finds what the live code needs from the linker: entries in the GOT,
    /// calls into shared libraries and words that point into them.
    fn scan(&mut self) -> Result<(), String> {
        for i in 0..self.inputs.len() {
            for section in 0..self.inputs[i].live.len() {
                if !self.inputs[i].live[section] || self.inputs[i].dropped[section] {
                    continue;
                }
                let relocations: Vec<(u64, RelocationType, Option<Target>)> =
                    self.relocations(i, section as u32).iter().map(|r| (r.offset, r.kind, r.target)).collect();
                let mut skip = None;
                for reloc in
                    relocations.iter().map(|&(offset, kind, target)| Relocation { offset, kind, target, addend: 0 })
                {
                    if Some(reloc.offset) == skip {
                        continue;
                    }
                    let Some(target) = reloc.target else { continue };
                    let shared = self.shared(target);
                    match (self.arch, reloc.kind) {
                        // A library keeps the call to `__tls_get_addr`, with
                        // a pair of entries the loader fills.
                        (Arch::X86_64, R_X86_64_TLSGD) if self.library => {
                            self.pair(Slot::Module(Some(target)), Slot::DtpOff(Some(target)))
                        }
                        (Arch::X86_64, R_X86_64_TLSLD) if self.library => {
                            self.pair(Slot::Module(None), Slot::DtpOff(None))
                        }
                        (Arch::Aarch64, R_AARCH64_TLSDESC_ADR_PAGE21) if self.library => {
                            self.pair(Slot::Descriptor(target), Slot::DescriptorArgument(target))
                        }
                        (Arch::X86_64, R_X86_64_TLSGD) => skip = Some(reloc.offset + 8),
                        (Arch::X86_64, R_X86_64_TLSLD) => skip = Some(reloc.offset + 5),
                        (Arch::X86_64, R_X86_64_GOTPCREL | R_X86_64_GOTPCRELX | R_X86_64_REX_GOTPCRELX)
                        | (Arch::Aarch64, R_AARCH64_ADR_GOT_PAGE | R_AARCH64_LD64_GOT_LO12_NC) => {
                            self.slot(Slot::Address(target));
                        }
                        (Arch::X86_64, R_X86_64_GOTTPOFF)
                        | (
                            Arch::Aarch64,
                            R_AARCH64_TLSIE_ADR_GOTTPREL_PAGE21 | R_AARCH64_TLSIE_LD64_GOTTPREL_LO12_NC,
                        ) => {
                            if shared {
                                return Err(error(&self.inputs[i].name, "a thread-local variable of a shared library"));
                            }
                            self.slot(Slot::TpOff(target));
                        }
                        (Arch::X86_64, R_X86_64_64) | (Arch::Aarch64, R_AARCH64_ABS64) if shared => {
                            self.dynamic_words += 1;
                            self.import(target);
                        }
                        (Arch::X86_64, R_X86_64_64) | (Arch::Aarch64, R_AARCH64_ABS64) if self.defined(target) => {
                            let align = self.inputs[i]
                                .file
                                .section_by_index(object::SectionIndex(section))
                                .map_or(1, |s| s.align());
                            if reloc.offset % 8 == 0 && align >= 8 {
                                self.aligned_words += 1;
                            } else {
                                self.unaligned_words += 1;
                            }
                        }
                        _ if shared => {
                            // A call, or the address of a function: its PLT entry.
                            self.import(target);
                            let Target::Global(id) = target else { continue };
                            if self.globals[id].plt.is_none() {
                                self.globals[id].plt = Some(self.plt.len() as u32);
                                self.plt.push(id);
                                self.slot(Slot::Address(target));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        // Each GOT entry for a shared library's symbol is bound at start.
        for slot in self.got.clone() {
            if let Slot::Address(target) = slot
                && self.shared(target)
            {
                self.import(target);
            }
        }
        Ok(())
    }

    /// Whether a target has an address of the executable's: not a shared
    /// library's, nor a weak one nothing defines.
    fn defined(&self, target: Target) -> bool {
        match target {
            Target::Global(id) => !matches!(self.globals[id].def, Def::Shared { .. } | Def::Undefined),
            Target::Local { .. } => true,
        }
    }

    fn shared(&self, target: Target) -> bool {
        matches!(target, Target::Global(id) if matches!(self.globals[id].def, Def::Shared { .. }))
    }

    fn slot(&mut self, slot: Slot) -> u32 {
        if let Some(&index) = self.slots.get(&slot) {
            return index;
        }
        self.got.push(slot);
        self.slots.insert(slot, self.got.len() as u32 - 1);
        self.got.len() as u32 - 1
    }

    /// Two entries next to each other.
    fn pair(&mut self, first: Slot, second: Slot) {
        if !self.slots.contains_key(&first) {
            self.slot(first);
            self.slot(second);
        }
    }

    fn import(&mut self, target: Target) {
        let Target::Global(id) = target else { return };
        if self.globals[id].dynsym.is_some() {
            return;
        }
        if let Def::Shared { library, .. } = self.globals[id].def {
            self.libraries[library as usize].needed = true;
        }
        self.dynsyms.push(id);
        self.globals[id].dynsym = Some(self.dynsyms.len() as u32);
    }

    /// Places each live input section in its output, and the outputs in
    /// the file and in memory.
    fn layout(&mut self, inputs: &Inputs) {
        let mut sizes = [0u64; OUTS.len()];
        // Constructors run in the order of their priority, those without
        // one last.
        let mut placed: Vec<(Out, u64, usize, usize)> = Vec::new();
        for (i, input) in self.inputs.iter().enumerate() {
            let endian = input.file.endian();
            for section in input.file.sections() {
                let s = section.index().0;
                if !input.live[s] || input.dropped[s] {
                    continue;
                }
                let header = section.elf_section_header();
                let name = section.name_bytes().unwrap_or_default();
                let Some(out) = output_of(name, header.sh_type(endian), header.sh_flags(endian)) else { continue };
                if out == Out::EhFrame {
                    continue;
                }
                let priority = match out {
                    Out::InitArray | Out::FiniArray => name
                        .rsplit(|&b| b == b'.')
                        .next()
                        .and_then(|p| std::str::from_utf8(p).ok()?.parse().ok())
                        .unwrap_or(u64::MAX),
                    _ => 0,
                };
                placed.push((out, priority, i, s));
            }
        }
        placed.sort_by_key(|&(out, priority, ..)| (out, priority));
        for (out, _, i, s) in placed {
            let section = self.inputs[i].file.section_by_index(object::SectionIndex(s)).unwrap();
            let align = section.align().max(1);
            let at = sizes[out as usize].next_multiple_of(align);
            sizes[out as usize] = at + section.size();
            self.align[out as usize] = self.align[out as usize].max(align);
            self.inputs[i].place[s] = Some((out, at));
        }
        // Tentative definitions of C.
        for id in 0..self.globals.len() {
            if let Def::Common { size, align } = self.globals[id].def {
                let at = sizes[Out::Bss as usize].next_multiple_of(align);
                self.globals[id].common = at;
                sizes[Out::Bss as usize] = at + size;
                self.align[Out::Bss as usize] = self.align[Out::Bss as usize].max(align);
            }
        }
        let (frames, fde_count) = self.eh_frame_size();
        sizes[Out::EhFrame as usize] = frames;
        sizes[Out::EhFrameHdr as usize] = if fde_count > 0 { 12 + 8 * fde_count } else { 0 };
        let libraries = self.libraries.iter().filter(|l| l.needed).count() as u64;
        let versions = self.versions();
        let dynsyms = self.dynsyms.len() as u64 + 1;
        sizes[Out::Interp as usize] = inputs.interpreter.as_ref().map_or(0, |i| i.len() as u64 + 1);
        sizes[Out::Hash as usize] = 4 * (2 + 1 + dynsyms);
        sizes[Out::Dynsym as usize] = 24 * dynsyms;
        sizes[Out::Dynstr as usize] = self.dynstr(inputs).0.len() as u64;
        sizes[Out::Versym as usize] = 2 * dynsyms;
        sizes[Out::Verneed as usize] = 16 * versions.iter().filter(|(_, v)| !v.is_empty()).count() as u64
            + 16 * versions.iter().map(|(_, v)| v.len() as u64).sum::<u64>();
        let bound = self.got.iter().filter(|s| matches!(s, Slot::Address(t) if self.shared(*t))).count();
        let moved = self.got.iter().filter(|s| matches!(s, Slot::Address(t) if self.defined(*t))).count();
        let (packed, listed) = match self.relr {
            true => (self.aligned_words + moved, self.unaligned_words),
            false => (0, self.aligned_words + self.unaligned_words + moved),
        };
        // What the loader writes of a library's thread-local block.
        let block = self.got.iter().filter(|s| match s {
            Slot::Module(_) | Slot::Descriptor(_) => true,
            Slot::TpOff(_) => self.library,
            _ => false,
        });
        let block = block.count();
        sizes[Out::RelaDyn as usize] = 24 * (bound + block + self.dynamic_words + listed) as u64;
        // At most a word for each; the bitmaps make it less.
        sizes[Out::Relr as usize] = 8 * packed as u64;
        sizes[Out::Plt as usize] = PLT_ENTRY * self.plt.len() as u64;
        sizes[Out::Got as usize] = 8 * self.got.len() as u64;
        sizes[Out::Dynamic as usize] = 16 * (libraries + u64::from(!inputs.runpath.is_empty()) + 24);
        for out in [Out::Hash, Out::Versym, Out::Verneed, Out::Got] {
            self.align[out as usize] = 8;
        }
        for out in [Out::Dynsym, Out::RelaDyn, Out::Relr, Out::Dynamic, Out::Got, Out::EhFrame, Out::EhFrameHdr] {
            self.align[out as usize] = self.align[out as usize].max(8);
        }
        self.align[Out::Plt as usize] = 16;
        if versions.iter().all(|(_, v)| v.is_empty()) {
            sizes[Out::Versym as usize] = 0;
            sizes[Out::Verneed as usize] = 0;
        }
        // A static executable relocates itself: it keeps the relocations
        // and the dynamic section that lists them.
        if !self.dynamic {
            for out in [Out::Hash, Out::Dynsym, Out::Dynstr, Out::Versym, Out::Verneed] {
                sizes[out as usize] = 0;
            }
        }
        self.size = sizes;
        // Memory follows the file, a page between segments; `.tbss` takes
        // no room in either, and `.bss` none in the file.
        let mut offset = 64 + 56 * self.program_headers() as u64;
        let mut segment = 0;
        for out in OUTS {
            let index = out as usize;
            if out.segment() != segment {
                segment = out.segment();
                offset = offset.next_multiple_of(self.arch.page());
            }
            let align = self.align[index];
            if out == Out::Tbss {
                let at = offset.next_multiple_of(align);
                self.offset[index] = at;
                self.addr[index] = BASE + at;
                continue;
            }
            offset = offset.next_multiple_of(align);
            self.offset[index] = offset;
            self.addr[index] = BASE + offset;
            if !out.nobits() {
                offset += sizes[index];
            }
        }
        let start = self.addr[Out::Tdata as usize];
        let end = self.addr[Out::Tbss as usize] + sizes[Out::Tbss as usize];
        let align = self.align[Out::Tdata as usize].max(self.align[Out::Tbss as usize]);
        self.tls = (start, end.max(start + sizes[Out::Tdata as usize]) - start, align);
    }

    fn program_headers(&self) -> usize {
        let tls = self.size[Out::Tdata as usize] + self.size[Out::Tbss as usize] > 0;
        let eh = self.size[Out::EhFrameHdr as usize] > 0;
        // Three loads, the header table, the dynamic section and the stack;
        // the loader unless it is static or a library.
        6 + usize::from(self.dynamic && !self.library) + usize::from(tls) + usize::from(eh)
    }

    /// The versions each needed library is asked for, `DT_RELR`'s among
    /// them when it is used.
    fn versions(&self) -> Vec<(usize, Vec<&'a [u8]>)> {
        let mut out: Vec<(usize, Vec<&'a [u8]>)> = Vec::new();
        if self.relr
            && let Some(libc) = self.libraries.iter().position(|l| l.soname == "libc.so.6")
        {
            out.push((libc, vec![RELR_VERSION]));
        }
        for &id in &self.dynsyms {
            let Def::Shared { library, .. } = self.globals[id].def else { continue };
            let Some(version) = self.libraries[library as usize].exports[self.globals[id].name].version else {
                continue;
            };
            match out.iter_mut().find(|(l, _)| *l == library as usize) {
                Some((_, versions)) if versions.contains(&version) => {}
                Some((_, versions)) => versions.push(version),
                None => out.push((library as usize, vec![version])),
            }
        }
        out
    }

    /// The dynamic string table, and where each name is in it.
    fn dynstr(&self, inputs: &Inputs) -> (Vec<u8>, HashMap<Vec<u8>, u32>) {
        let mut table = vec![0u8];
        let mut at: HashMap<Vec<u8>, u32> = HashMap::new();
        let mut add = |name: &[u8]| {
            if !at.contains_key(name) {
                at.insert(name.to_vec(), table.len() as u32);
                table.extend_from_slice(name);
                table.push(0);
            }
        };
        for library in self.libraries.iter().filter(|l| l.needed) {
            add(library.soname.as_bytes());
        }
        for &id in &self.dynsyms {
            add(self.globals[id].name);
        }
        for (_, versions) in self.versions() {
            versions.iter().for_each(|v| add(v));
        }
        if !inputs.runpath.is_empty() {
            add(inputs.runpath.join(":").as_bytes());
        }
        if let Some((soname, _)) = &inputs.library {
            add(soname.as_bytes());
        }
        (table, at)
    }

    /// The number of an output's section header, as `headers` writes them.
    fn section_index(&self, wanted: Out) -> u16 {
        let written = OUTS.iter().filter(|&&out| {
            self.size[out as usize] > 0 || (self.dynamic && matches!(out, Out::Dynamic | Out::Dynsym | Out::Dynstr))
        });
        written.take_while(|&&out| out != wanted).count() as u16 + 1
    }

    fn section_address(&self, input: u32, section: u32) -> u64 {
        match self.inputs[input as usize].place[section as usize] {
            Some((out, offset)) => self.addr[out as usize] + offset,
            None => 0,
        }
    }

    fn address(&self, target: Target) -> u64 {
        match target {
            Target::Global(id) => self.global_address(id),
            Target::Local { input, symbol } => {
                let file = &self.inputs[input as usize].file;
                let Ok(symbol) = file.symbol_by_index(SymbolIndex(symbol as usize)) else { return 0 };
                match symbol.section() {
                    SymbolSection::Section(section) => self.section_address(input, section.0 as u32) + symbol.address(),
                    SymbolSection::Absolute => symbol.address(),
                    _ => 0,
                }
            }
        }
    }

    fn global_address(&self, id: usize) -> u64 {
        let global = &self.globals[id];
        match global.def {
            Def::Section { input, section, value } => self.section_address(input, section) + value,
            Def::Absolute(value) => value,
            Def::Common { .. } => self.addr[Out::Bss as usize] + global.common,
            Def::Shared { .. } => global.plt.map_or(0, |p| self.addr[Out::Plt as usize] + PLT_ENTRY * p as u64),
            Def::Linker(synthetic) => {
                let (start, end) =
                    (|out: Out| self.addr[out as usize], |out: Out| self.addr[out as usize] + self.size[out as usize]);
                match synthetic {
                    Synthetic::InitArrayStart => start(Out::InitArray),
                    Synthetic::InitArrayEnd => end(Out::InitArray),
                    Synthetic::FiniArrayStart => start(Out::FiniArray),
                    Synthetic::FiniArrayEnd => end(Out::FiniArray),
                    Synthetic::GlobalOffsetTable => self.addr[Out::Got as usize],
                    Synthetic::Dynamic => self.addr[Out::Dynamic as usize],
                    Synthetic::HeaderStart => BASE,
                }
            }
            Def::Undefined => 0,
        }
    }

    /// The offset of a thread-local variable from the thread pointer.
    fn tp_offset(&self, target: Target) -> i64 {
        let (start, size, align) = self.tls;
        let offset = self.address(target).wrapping_sub(start) as i64;
        match self.arch {
            Arch::X86_64 => offset - size.next_multiple_of(align) as i64,
            Arch::Aarch64 => offset + 16u64.next_multiple_of(align) as i64,
        }
    }

    /// The offset of a thread-local variable in the block of its module.
    fn dtp_offset(&self, target: Target) -> i64 {
        self.address(target).wrapping_sub(self.tls.0) as i64
    }

    fn slot_address(&self, slot: Slot) -> u64 {
        self.addr[Out::Got as usize] + 8 * self.slots[&slot] as u64
    }
}

/// An object read on its own, before the linker takes in its symbols: the
/// sections of each COMDAT group, by its signature, and its `.eh_frame`
/// section with its records and each function section's records there.
struct Parsed<'a> {
    name: String,
    file: ElfFile64<'a, Endianness>,
    comdats: Vec<(&'a [u8], Vec<usize>)>,
    eh_frame: EhFrame,
    /// Its symbols that are not local, and how many symbols it has.
    symbols: Vec<Symbol<'a>>,
    count: usize,
}

/// A symbol of an object that is not local: for a common one, its value is
/// its alignment.
struct Symbol<'a> {
    index: usize,
    name: &'a [u8],
    section: SymbolSection,
    weak: bool,
    address: u64,
    size: u64,
}

type EhFrame = Option<(u32, Vec<Record>, HashMap<u32, Vec<usize>>)>;

/// Reads an object for `take`, which can happen on any thread.
fn parse<'a>(arch: Arch, name: String, data: &'a [u8]) -> Result<Parsed<'a>, String> {
    let file = ElfFile64::<Endianness>::parse(data).map_err(|e| error(&name, e))?;
    if file.elf_header().e_machine.get(file.endian()) != arch.machine() {
        return Err(error(&name, "it is for another architecture"));
    }
    let mut comdats = Vec::new();
    for comdat in file.comdats() {
        let signature = comdat.name_bytes().map_err(|e| error(&name, e))?;
        comdats.push((signature, comdat.sections().map(|section| section.0).collect()));
    }
    let eh_frame = eh_frame(&name, &file)?;
    let mut symbols = Vec::new();
    for symbol in file.symbols().filter(|symbol| !symbol.is_local()) {
        symbols.push(Symbol {
            index: symbol.index().0,
            name: symbol.name_bytes().map_err(|e| error(&name, e))?,
            section: symbol.section(),
            weak: symbol.is_weak(),
            address: symbol.elf_symbol().st_value.get(file.endian()),
            size: symbol.size(),
        });
    }
    let count = file.symbols().count();
    Ok(Parsed { name, file, comdats, eh_frame, symbols, count })
}

/// The records of an object's `.eh_frame`: which CIE each description of
/// a function uses, and which section the function is in.
fn eh_frame(name: &str, file: &ElfFile64<'_, Endianness>) -> Result<EhFrame, String> {
    let Some(section) = file.section_by_name(".eh_frame") else { return Ok(None) };
    let index = section.index().0 as u32;
    let data = section.data().map_err(|e| error(name, e))?;
    let mut functions: HashMap<u64, u32> = HashMap::new();
    for (offset, r) in section.relocations() {
        if let RelocationTarget::Symbol(symbol) = r.target()
            && let Ok(symbol) = file.symbol_by_index(symbol)
            && let SymbolSection::Section(target) = symbol.section()
        {
            functions.insert(offset, target.0 as u32);
        }
    }
    let mut records = Vec::new();
    let mut fdes: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut at = 0u64;
    while at + 8 <= data.len() as u64 {
        let word = |o: u64| u32::from_le_bytes(data[o as usize..o as usize + 4].try_into().unwrap());
        let length = word(at) as u64;
        if length == 0 {
            break;
        }
        if length == 0xffff_ffff {
            return Err(error(name, "a 64-bit `.eh_frame` record"));
        }
        let id = word(at + 4) as u64;
        let cie = (id != 0).then(|| at + 4 - id);
        let function = cie.and_then(|_| functions.get(&(at + 8)).copied());
        if let Some(function) = function {
            fdes.entry(function).or_default().push(records.len());
        }
        records.push(Record { start: at, len: length + 4, cie, function });
        at += length + 4;
    }
    Ok(Some((index, records, fdes)))
}

/// Reads what a shared library exports, with its default versions.
fn library<'a>(name: &str, data: &'a [u8]) -> Result<Library<'a>, String> {
    let file = ElfFile64::<Endianness>::parse(data).map_err(|e| error(name, e))?;
    let endian = file.endian();
    let versions = file.elf_section_table().versions(endian, data).map_err(|e| error(name, e))?;
    let mut exports = FastMap::default();
    for symbol in file.dynamic_symbols() {
        if symbol.is_undefined() || symbol.is_local() {
            continue;
        }
        let version = match &versions {
            Some(table) => {
                let index = table.version_index(endian, symbol.index());
                if index.is_hidden() {
                    continue;
                }
                table.version(index.index()).ok().flatten().map(|v| v.name())
            }
            None => None,
        };
        let kind = symbol.elf_symbol().st_type();
        let symbol_name = symbol.name_bytes().map_err(|e| error(name, e))?;
        exports.insert(symbol_name, Export { kind, version });
    }
    // Its name for the dynamic loader, which may differ from the file's.
    let mut soname =
        std::path::Path::new(name).file_name().map_or(name.to_string(), |n| n.to_string_lossy().into_owned());
    if let (Some(dynamic), Some(strings)) = (file.section_by_name(".dynamic"), file.section_by_name(".dynstr"))
        && let (Ok(dynamic), Ok(strings)) = (dynamic.data(), strings.data())
    {
        for entry in dynamic.as_chunks::<16>().0 {
            let tag = u64::from_le_bytes(entry[..8].try_into().unwrap());
            let value = u64::from_le_bytes(entry[8..].try_into().unwrap()) as usize;
            if tag == DT_SONAME.0 as u64
                && let Some(text) = strings.get(value..)
            {
                let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
                soname = String::from_utf8_lossy(&text[..end]).into_owned();
            }
        }
    }
    let relr = file
        .section_by_name(".dynstr")
        .and_then(|s| s.data().ok())
        .is_some_and(|strings| strings.windows(RELR_VERSION.len()).any(|w| w == RELR_VERSION));
    Ok(Library { soname, exports, needed: false, relr })
}

/// The version a program that packs its relocations as `DT_RELR` asks
/// glibc for, so that an older one refuses it.
const RELR_VERSION: &[u8] = b"GLIBC_ABI_DT_RELR";

/// The ELF hash of a name.
fn elf_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &c in name {
        h = (h << 4).wrapping_add(c as u32);
        let g = h & 0xf000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

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

    fn u16(&mut self, at: u64, v: u16) {
        self.put(at, &v.to_le_bytes());
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
}

impl<'a> Linker<'a> {
    /// The size of the rebuilt `.eh_frame`, and how many descriptions of
    /// functions it keeps.
    fn eh_frame_size(&self) -> (u64, u64) {
        let (mut size, mut fdes) = (0, 0);
        for input in &self.inputs {
            for record in self.kept_records(input) {
                size += record.len;
                fdes += u64::from(record.cie.is_some());
            }
        }
        (if size > 0 { size + 4 } else { 0 }, fdes)
    }

    /// The records of an input's `.eh_frame` the executable keeps: those
    /// describing live functions, and the CIEs they use.
    fn kept_records<'i>(&self, input: &'i Input) -> Vec<&'i Record> {
        let Some((_, records)) = &input.eh_frame else { return Vec::new() };
        let live = |r: &Record| r.function.is_some_and(|f| input.live[f as usize] && !input.dropped[f as usize]);
        let cies: HashSet<u64> = records.iter().filter(|r| live(r)).filter_map(|r| r.cie).collect();
        records.iter().filter(|r| live(r) || (r.cie.is_none() && cies.contains(&r.start))).collect()
    }

    fn write(&self, inputs: &Inputs, entry: usize) -> Result<Vec<u8>, String> {
        let mut image = Image { bytes: Vec::new() };
        let at = |out: Out| self.offset[out as usize];
        let addr = |out: Out| self.addr[out as usize];
        let (dynstr, strings) = self.dynstr(inputs);
        let versions = self.versions();
        // The interpreter, and the dynamic symbols with their versions.
        if let Some(interpreter) = &inputs.interpreter {
            image.put(at(Out::Interp), interpreter.as_bytes());
        }
        image.put(at(Out::Dynstr), &dynstr);
        let mut version_index: HashMap<(usize, &[u8]), u16> = HashMap::new();
        let mut next = 2u16;
        for (library, names) in &versions {
            for name in names {
                version_index.insert((*library, name), next);
                next += 1;
            }
        }
        for (n, &id) in self.dynsyms.iter().enumerate() {
            let global = &self.globals[id];
            let entry_at = at(Out::Dynsym) + 24 * (n as u64 + 1);
            // What a library publishes: its functions, in its code.
            let Def::Shared { library, .. } = global.def else {
                image.u32(entry_at, strings[global.name]);
                image.put(entry_at + 4, &[(STB_GLOBAL.0 << 4) | STT_FUNC.0, 0]);
                image.u16(entry_at + 6, self.section_index(Out::Text));
                image.u64(entry_at + 8, self.global_address(id));
                if self.size[Out::Versym as usize] > 0 {
                    image.u16(at(Out::Versym) + 2 * (n as u64 + 1), VER_NDX_GLOBAL.0);
                }
                continue;
            };
            let export = self.libraries[library as usize].exports[global.name];
            let binding = if global.required { STB_GLOBAL } else { STB_WEAK };
            let kind = if export.kind == STT_GNU_IFUNC { STT_FUNC } else { export.kind };
            image.u32(entry_at, strings[global.name]);
            image.put(entry_at + 4, &[(binding.0 << 4) | kind.0, 0]);
            if self.size[Out::Versym as usize] > 0 {
                let version = export.version.map_or(1, |v| version_index[&(library as usize, v)]);
                image.u16(at(Out::Versym) + 2 * (n as u64 + 1), version);
            }
        }
        let dynsyms = self.dynsyms.len() as u32 + 1;
        // One bucket, which a library's few functions share.
        let hash = at(Out::Hash);
        image.u32(hash, 1);
        image.u32(hash + 4, dynsyms);
        image.u32(hash + 8, dynsyms.saturating_sub(1));
        for n in 1..dynsyms {
            image.u32(hash + 12 + 4 * n as u64, n - 1);
        }
        let mut verneed = at(Out::Verneed);
        let needed: Vec<&(usize, Vec<&[u8]>)> = versions.iter().filter(|(_, v)| !v.is_empty()).collect();
        for (n, (library, names)) in needed.iter().enumerate() {
            let soname = self.libraries[*library].soname.as_bytes();
            image.u16(verneed, 1);
            image.u16(verneed + 2, names.len() as u16);
            image.u32(verneed + 4, strings[soname]);
            image.u32(verneed + 8, 16);
            image.u32(verneed + 12, if n + 1 < needed.len() { 16 + 16 * names.len() as u32 } else { 0 });
            for (v, name) in names.iter().enumerate() {
                let aux = verneed + 16 + 16 * v as u64;
                image.u32(aux, elf_hash(name));
                image.u16(aux + 4, 0);
                image.u16(aux + 6, version_index[&(*library, *name)]);
                image.u32(aux + 8, strings[*name]);
                image.u32(aux + 12, if v + 1 < names.len() { 16 } else { 0 });
            }
            verneed += 16 + 16 * names.len() as u64;
        }
        // The GOT and the PLT.
        let mut relocations: Vec<(u64, RelocationType, u32, i64)> = Vec::new();
        for (n, slot) in self.got.iter().enumerate() {
            let entry_addr = addr(Out::Got) + 8 * n as u64;
            let value = match *slot {
                Slot::Address(target) if self.shared(target) => {
                    let Target::Global(id) = target else { unreachable!() };
                    relocations.push((entry_addr, self.arch.glob_dat(), self.globals[id].dynsym.unwrap(), 0));
                    0
                }
                Slot::Address(target) => {
                    if self.defined(target) {
                        relocations.push((entry_addr, self.arch.relative(), 0, self.address(target) as i64));
                    }
                    self.address(target)
                }
                // A library's thread-local variables are where the loader
                // puts its block.
                Slot::TpOff(target) if self.library => {
                    relocations.push((entry_addr, self.arch.tp_off(), 0, self.dtp_offset(target)));
                    0
                }
                Slot::TpOff(target) => self.tp_offset(target) as u64,
                Slot::Module(_) => {
                    relocations.push((entry_addr, R_X86_64_DTPMOD64, 0, 0));
                    0
                }
                Slot::DtpOff(target) => target.map_or(0, |t| self.dtp_offset(t) as u64),
                Slot::Descriptor(target) => {
                    relocations.push((entry_addr, R_AARCH64_TLSDESC, 0, self.dtp_offset(target)));
                    0
                }
                Slot::DescriptorArgument(_) => 0,
            };
            image.u64(at(Out::Got) + 8 * n as u64, value);
        }
        for (n, &id) in self.plt.iter().enumerate() {
            let stub = addr(Out::Plt) + PLT_ENTRY * n as u64;
            let slot = self.slots.get(&Slot::Address(Target::Global(id))).copied();
            let slot = match slot {
                Some(slot) => addr(Out::Got) + 8 * slot as u64,
                None => return Err(format!("no GOT entry for {}", String::from_utf8_lossy(self.globals[id].name))),
            };
            let code = plt_entry(self.arch, stub, slot);
            image.put(at(Out::Plt) + PLT_ENTRY * n as u64, &code);
        }
        // The sections of the inputs, relocated on every core, each apart,
        // and put in their order.
        let mut sections: Vec<(usize, usize, Out, u64)> = Vec::new();
        for (i, input) in self.inputs.iter().enumerate() {
            for (s, place) in input.place.iter().enumerate() {
                if let Some((out, offset)) = *place
                    && !out.nobits()
                {
                    sections.push((i, s, out, offset));
                }
            }
        }
        let relocated = crate::parallel::map(&sections, 16, |&(i, s, out, offset)| {
            let input = &self.inputs[i];
            let section = input.file.section_by_index(object::SectionIndex(s)).map_err(|e| error(&input.name, e))?;
            let data = section.data().map_err(|e| error(&input.name, e))?;
            let mut alone = Image { bytes: data.to_vec() };
            let mut dynamic = Vec::new();
            self.relocate(&mut alone, i, s as u32, 0, addr(out) + offset, &mut dynamic, None)?;
            Ok::<_, String>((alone.bytes, dynamic))
        });
        for (&(_, _, out, offset), relocated) in sections.iter().zip(relocated) {
            let (bytes, dynamic) = relocated?;
            image.put(at(out) + offset, &bytes);
            relocations.extend(dynamic);
        }
        let fdes = self.write_eh_frame(&mut image, &mut relocations)?;
        if self.size[Out::EhFrameHdr as usize] > 0 {
            let hdr = addr(Out::EhFrameHdr);
            let mut table: Vec<(i32, i32)> =
                fdes.iter().map(|&(function, fde)| ((function - hdr) as i32, (fde - hdr) as i32)).collect();
            table.sort();
            let h = at(Out::EhFrameHdr);
            image.put(h, &[1, DW_EH_PE_PCREL_SDATA4, DW_EH_PE_UDATA4, DW_EH_PE_DATAREL_SDATA4]);
            image.u32(h + 4, (addr(Out::EhFrame) as i64 - (hdr + 4) as i64) as u32);
            image.u32(h + 8, table.len() as u32);
            for (n, (function, fde)) in table.iter().enumerate() {
                image.u32(h + 12 + 8 * n as u64, *function as u32);
                image.u32(h + 16 + 8 * n as u64, *fde as u32);
            }
        }
        // The executable's own addresses move with it: packed where the C
        // library reads `DT_RELR` and the word is aligned, listed first
        // otherwise; then what is bound.
        let relative = self.arch.relative();
        let (moved, mut relocations): (Vec<_>, Vec<_>) = relocations.into_iter().partition(|r| r.1 == relative);
        let (packed, listed): (Vec<_>, Vec<_>) = moved.into_iter().partition(|r| self.relr && r.0 % 8 == 0);
        let relr = relr(&packed.iter().map(|r| r.0).collect::<Vec<_>>());
        if relr.len() as u64 * 8 > self.size[Out::Relr as usize] {
            return Err(format!("{} packed relocations take more than the room left for them", relr.len()));
        }
        for (n, word) in relr.iter().enumerate() {
            image.u64(at(Out::Relr) + 8 * n as u64, *word);
        }
        let listed_count = listed.len();
        relocations.splice(0..0, listed);
        if relocations.len() as u64 * 24 > self.size[Out::RelaDyn as usize] {
            return Err(format!("{} relocations take more than the room left for them", relocations.len()));
        }
        for (n, &(offset, kind, symbol, addend)) in relocations.iter().enumerate() {
            let entry_at = at(Out::RelaDyn) + 24 * n as u64;
            image.u64(entry_at, offset);
            image.u64(entry_at + 8, ((symbol as u64) << 32) | kind.0 as u64);
            image.u64(entry_at + 16, addend as u64);
        }
        // The dynamic section.
        let _ = listed_count;
        let mut dynamic: Vec<(DynamicTag, u64)> = Vec::new();
        for library in self.libraries.iter().filter(|l| l.needed) {
            dynamic.push((DT_NEEDED, strings[library.soname.as_bytes()] as u64));
        }
        if !inputs.runpath.is_empty() {
            dynamic.push((DT_RUNPATH, strings[inputs.runpath.join(":").as_bytes()] as u64));
        }
        if let Some((soname, _)) = &inputs.library {
            dynamic.push((DT_SONAME, strings[soname.as_bytes()] as u64));
        }
        if self.dynamic {
            dynamic.extend([
                (DT_HASH, addr(Out::Hash)),
                (DT_STRTAB, addr(Out::Dynstr)),
                (DT_SYMTAB, addr(Out::Dynsym)),
                (DT_STRSZ, dynstr.len() as u64),
                (DT_SYMENT, 24),
            ]);
        }
        if !relr.is_empty() {
            dynamic.extend([(DT_RELR, addr(Out::Relr)), (DT_RELRSZ, 8 * relr.len() as u64), (DT_RELRENT, 8)]);
        }
        dynamic.extend([
            (DT_RELA, addr(Out::RelaDyn)),
            (DT_RELASZ, 24 * relocations.len() as u64),
            (DT_RELAENT, 24),
            (DT_INIT_ARRAY, addr(Out::InitArray)),
            (DT_INIT_ARRAYSZ, self.size[Out::InitArray as usize]),
            (DT_FINI_ARRAY, addr(Out::FiniArray)),
            (DT_FINI_ARRAYSZ, self.size[Out::FiniArray as usize]),
        ]);
        // A library stays loaded: its runtime's threads run its code.
        let static_tls = self.library && self.got.iter().any(|s| matches!(s, Slot::TpOff(_)));
        dynamic.extend(match self.library {
            true => [
                (DT_FLAGS, DF_BIND_NOW.0 | if static_tls { DF_STATIC_TLS.0 } else { 0 }),
                (DT_FLAGS_1, DF_1_NOW.0 | DF_1_NODELETE.0),
            ]
            .to_vec(),
            false => [(DT_DEBUG, 0), (DT_FLAGS, DF_BIND_NOW.0), (DT_FLAGS_1, DF_1_NOW.0 | DF_1_PIE.0)].to_vec(),
        });
        if self.size[Out::Versym as usize] > 0 {
            dynamic.extend([
                (DT_VERSYM, addr(Out::Versym)),
                (DT_VERNEED, addr(Out::Verneed)),
                (DT_VERNEEDNUM, needed.len() as u64),
            ]);
        }
        dynamic.push((DT_NULL, 0));
        for (n, (tag, value)) in dynamic.iter().enumerate() {
            image.u64(at(Out::Dynamic) + 16 * n as u64, tag.0 as u64);
            image.u64(at(Out::Dynamic) + 16 * n as u64 + 8, *value);
        }
        let loaded_end =
            OUTS.iter().map(|&o| at(o) + if o.nobits() { 0 } else { self.size[o as usize] }).max().unwrap();
        image.bytes.resize(loaded_end as usize, 0);
        self.headers(&mut image, inputs, entry)?;
        Ok(image.bytes)
    }

    /// Applies the relocations of an input section copied to `file_at`,
    /// which the program sees at `base`. With `map`, the section is
    /// `.eh_frame`, whose kept records moved: the offsets they now have.
    #[allow(clippy::too_many_arguments)]
    fn relocate(
        &self,
        image: &mut Image,
        input: usize,
        section: u32,
        file_at: u64,
        base: u64,
        dynamic: &mut Vec<(u64, RelocationType, u32, i64)>,
        map: Option<&dyn Fn(u64) -> Option<u64>>,
    ) -> Result<(), String> {
        let relocations = self.relocations(input, section);
        let name = &self.inputs[input].name;
        let mut skip = None;
        for reloc in relocations {
            if Some(reloc.offset) == skip {
                continue;
            }
            let offset = match map {
                Some(map) => match map(reloc.offset) {
                    Some(offset) => offset,
                    None => continue,
                },
                None => reloc.offset,
            };
            let (f, p) = (file_at + offset, base + offset);
            let Some(target) = reloc.target else { continue };
            let s = self.address(target) as i64;
            let a = reloc.addend;
            let fail = |what: &str| {
                let target = match target {
                    Target::Global(id) => String::from_utf8_lossy(self.globals[id].name).into_owned(),
                    Target::Local { .. } => "a local symbol".into(),
                };
                error(name, format!("{what} (relocation {} to {target})", reloc.kind.0))
            };
            let pc32 = |image: &mut Image, value: i64| -> Result<(), String> {
                let v = i32::try_from(value).map_err(|_| fail("out of range"))?;
                image.u32(f, v as u32);
                Ok(())
            };
            match self.arch {
                Arch::X86_64 => match reloc.kind {
                    R_X86_64_NONE => {}
                    R_X86_64_64 if self.shared(target) => {
                        let Target::Global(id) = target else { unreachable!() };
                        dynamic.push((p, R_X86_64_64, self.globals[id].dynsym.unwrap(), a));
                        image.u64(f, 0);
                    }
                    R_X86_64_64 => {
                        if self.defined(target) {
                            dynamic.push((p, R_X86_64_RELATIVE, 0, s + a));
                        }
                        image.u64(f, (s + a) as u64);
                    }
                    R_X86_64_PC32 | R_X86_64_PLT32 => pc32(image, s + a - p as i64)?,
                    R_X86_64_PC64 => image.u64(f, (s + a - p as i64) as u64),
                    R_X86_64_32 | R_X86_64_32S => return Err(fail(PIC)),
                    R_X86_64_GOTPCREL | R_X86_64_GOTPCRELX | R_X86_64_REX_GOTPCRELX => {
                        pc32(image, self.slot_address(Slot::Address(target)) as i64 + a - p as i64)?
                    }
                    R_X86_64_GOTTPOFF => pc32(image, self.slot_address(Slot::TpOff(target)) as i64 + a - p as i64)?,
                    R_X86_64_TLSGD if self.library => {
                        let pair = self.slot_address(Slot::Module(Some(target)));
                        pc32(image, pair as i64 + a - p as i64)?
                    }
                    R_X86_64_TLSLD if self.library => {
                        pc32(image, self.slot_address(Slot::Module(None)) as i64 + a - p as i64)?
                    }
                    R_X86_64_DTPOFF32 if self.library => pc32(image, self.dtp_offset(target) + a)?,
                    R_X86_64_DTPOFF64 if self.library => image.u64(f, (self.dtp_offset(target) + a) as u64),
                    R_X86_64_TPOFF32 | R_X86_64_TPOFF64 if self.library => return Err(fail(TLS)),
                    R_X86_64_TPOFF32 | R_X86_64_DTPOFF32 => pc32(image, self.tp_offset(target) + a)?,
                    R_X86_64_TPOFF64 => image.u64(f, (self.tp_offset(target) + a) as u64),
                    R_X86_64_GOTPC32 => pc32(image, addr_got(self) as i64 + a - p as i64)?,
                    R_X86_64_GOTOFF64 => image.u64(f, (s + a - addr_got(self) as i64) as u64),
                    R_X86_64_SIZE32 | R_X86_64_SIZE64 => return Err(fail("unsupported")),
                    // `lea x@tlsgd(%rip), %rdi; call __tls_get_addr`
                    // becomes `mov %fs:0, %rax; lea x@tpoff(%rax), %rax`.
                    R_X86_64_TLSGD => {
                        let mut code = vec![0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0, 0x48, 0x8d, 0x80];
                        code.extend_from_slice(&(self.tp_offset(target) as i32).to_le_bytes());
                        image.put(f - 4, &code);
                        skip = Some(reloc.offset + 8);
                    }
                    // `lea x@tlsld(%rip), %rdi; call __tls_get_addr` becomes
                    // `mov %fs:0, %rax`; the offsets that follow are from
                    // the thread pointer.
                    R_X86_64_TLSLD => {
                        image.put(f - 3, &[0x66, 0x66, 0x66, 0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0]);
                        skip = Some(reloc.offset + 5);
                    }
                    _ => return Err(fail("unsupported")),
                },
                Arch::Aarch64 => {
                    let page = |x: i64| x & !0xfff;
                    let insn = image.read32(f);
                    let set = |image: &mut Image, value: u32| image.u32(f, value);
                    let adrp = |image: &mut Image, delta: i64| -> Result<(), String> {
                        let imm = delta >> 12;
                        if !(-(1 << 20)..(1 << 20)).contains(&imm) {
                            return Err(fail("out of range"));
                        }
                        let imm = imm as u32;
                        image.u32(f, (insn & 0x9f00_001f) | ((imm & 3) << 29) | (((imm >> 2) & 0x7ffff) << 5));
                        Ok(())
                    };
                    let lo12 = |image: &mut Image, value: i64, shift: u32| {
                        let imm = ((value & 0xfff) >> shift) as u32;
                        image.u32(f, (insn & !(0xfff << 10)) | (imm << 10));
                    };
                    let branch = |image: &mut Image, bits: u32, at: u32| -> Result<(), String> {
                        let delta = (s + a - p as i64) >> 2;
                        if delta >= 1 << (bits - 1) || delta < -(1 << (bits - 1)) {
                            return Err(fail("out of range"));
                        }
                        let mask = ((1u32 << bits) - 1) << at;
                        image.u32(f, (insn & !mask) | (((delta as u32) << at) & mask));
                        Ok(())
                    };
                    match reloc.kind {
                        R_AARCH64_NONE => {}
                        R_AARCH64_ABS64 if self.shared(target) => {
                            let Target::Global(id) = target else { unreachable!() };
                            dynamic.push((p, R_AARCH64_ABS64, self.globals[id].dynsym.unwrap(), a));
                            image.u64(f, 0);
                        }
                        R_AARCH64_ABS64 => {
                            if self.defined(target) {
                                dynamic.push((p, R_AARCH64_RELATIVE, 0, s + a));
                            }
                            image.u64(f, (s + a) as u64);
                        }
                        R_AARCH64_ABS32 => return Err(fail(PIC)),
                        R_AARCH64_PREL64 => image.u64(f, (s + a - p as i64) as u64),
                        R_AARCH64_PREL32 => pc32(image, s + a - p as i64)?,
                        R_AARCH64_CALL26 | R_AARCH64_JUMP26 => branch(image, 26, 0)?,
                        R_AARCH64_CONDBR19 | R_AARCH64_LD_PREL_LO19 => branch(image, 19, 5)?,
                        R_AARCH64_TSTBR14 => branch(image, 14, 5)?,
                        R_AARCH64_ADR_PREL_PG_HI21 | R_AARCH64_ADR_PREL_PG_HI21_NC => {
                            adrp(image, page(s + a) - page(p as i64))?
                        }
                        R_AARCH64_ADR_PREL_LO21 => {
                            let imm = (s + a - p as i64) as u32;
                            set(image, (insn & 0x9f00_001f) | ((imm & 3) << 29) | (((imm >> 2) & 0x7ffff) << 5));
                        }
                        R_AARCH64_ADD_ABS_LO12_NC | R_AARCH64_LDST8_ABS_LO12_NC => lo12(image, s + a, 0),
                        R_AARCH64_LDST16_ABS_LO12_NC => lo12(image, s + a, 1),
                        R_AARCH64_LDST32_ABS_LO12_NC => lo12(image, s + a, 2),
                        R_AARCH64_LDST64_ABS_LO12_NC => lo12(image, s + a, 3),
                        R_AARCH64_LDST128_ABS_LO12_NC => lo12(image, s + a, 4),
                        R_AARCH64_MOVW_UABS_G0
                        | R_AARCH64_MOVW_UABS_G0_NC
                        | R_AARCH64_MOVW_UABS_G1
                        | R_AARCH64_MOVW_UABS_G1_NC
                        | R_AARCH64_MOVW_UABS_G2
                        | R_AARCH64_MOVW_UABS_G2_NC
                        | R_AARCH64_MOVW_UABS_G3 => return Err(fail(PIC)),
                        R_AARCH64_ADR_GOT_PAGE => {
                            adrp(image, page(self.slot_address(Slot::Address(target)) as i64) - page(p as i64))?
                        }
                        R_AARCH64_LD64_GOT_LO12_NC => lo12(image, self.slot_address(Slot::Address(target)) as i64, 3),
                        R_AARCH64_TLSIE_ADR_GOTTPREL_PAGE21 => {
                            adrp(image, page(self.slot_address(Slot::TpOff(target)) as i64) - page(p as i64))?
                        }
                        R_AARCH64_TLSIE_LD64_GOTTPREL_LO12_NC => {
                            lo12(image, self.slot_address(Slot::TpOff(target)) as i64, 3)
                        }
                        R_AARCH64_TLSLE_ADD_TPREL_HI12
                        | R_AARCH64_TLSLE_ADD_TPREL_LO12
                        | R_AARCH64_TLSLE_ADD_TPREL_LO12_NC
                            if self.library =>
                        {
                            return Err(fail(TLS));
                        }
                        // A library keeps the descriptor's call: the loader
                        // fills the descriptor.
                        R_AARCH64_TLSDESC_ADR_PAGE21 if self.library => {
                            adrp(image, page(self.slot_address(Slot::Descriptor(target)) as i64) - page(p as i64))?
                        }
                        R_AARCH64_TLSDESC_LD64_LO12 if self.library => {
                            lo12(image, self.slot_address(Slot::Descriptor(target)) as i64, 3)
                        }
                        R_AARCH64_TLSDESC_ADD_LO12 if self.library => {
                            lo12(image, self.slot_address(Slot::Descriptor(target)) as i64, 0)
                        }
                        R_AARCH64_TLSDESC_CALL if self.library => {}
                        R_AARCH64_TLSLE_ADD_TPREL_HI12 => lo12(image, (self.tp_offset(target) + a) >> 12, 0),
                        R_AARCH64_TLSLE_ADD_TPREL_LO12 | R_AARCH64_TLSLE_ADD_TPREL_LO12_NC => {
                            lo12(image, self.tp_offset(target) + a, 0)
                        }
                        // The descriptor's call becomes the offset from the
                        // thread pointer, in `x0`.
                        R_AARCH64_TLSDESC_ADR_PAGE21 => {
                            let imm = ((self.tp_offset(target) + a) as u64 >> 16) as u32 & 0xffff;
                            set(image, 0xd2a0_0000 | (imm << 5));
                        }
                        R_AARCH64_TLSDESC_LD64_LO12 => {
                            let imm = (self.tp_offset(target) + a) as u32 & 0xffff;
                            set(image, 0xf280_0000 | (imm << 5));
                        }
                        R_AARCH64_TLSDESC_ADD_LO12 | R_AARCH64_TLSDESC_CALL => set(image, 0xd503_201f),
                        _ => return Err(fail("unsupported")),
                    }
                }
            }
        }
        Ok(())
    }

    /// Writes the kept records of every `.eh_frame`, each description of a
    /// function pointing to its CIE where that now is; gives each
    /// description's function and address.
    fn write_eh_frame(
        &self,
        image: &mut Image,
        dynamic: &mut Vec<(u64, RelocationType, u32, i64)>,
    ) -> Result<Vec<(u64, u64)>, String> {
        let mut out = 0u64;
        let base = self.addr[Out::EhFrame as usize];
        let file = self.offset[Out::EhFrame as usize];
        let mut fdes = Vec::new();
        for (i, input) in self.inputs.iter().enumerate() {
            let Some((section, _)) = &input.eh_frame else { continue };
            let kept = self.kept_records(input);
            if kept.is_empty() {
                continue;
            }
            let data = input.file.section_by_index(object::SectionIndex(*section as usize)).unwrap().data().unwrap();
            let mut moved: HashMap<u64, u64> = HashMap::new();
            for record in &kept {
                moved.insert(record.start, out);
                image.put(file + out, &data[record.start as usize..(record.start + record.len) as usize]);
                if let Some(cie) = record.cie {
                    let pointer = (out + 4) - moved[&cie];
                    image.u32(file + out + 4, pointer as u32);
                }
                out += record.len;
            }
            let map = |offset: u64| {
                let record = kept.iter().find(|r| offset >= r.start && offset < r.start + r.len)?;
                Some(moved[&record.start] + offset - record.start)
            };
            self.relocate(image, i, *section, file, base, dynamic, Some(&map))?;
            let relocations = self.relocations(i, *section);
            for record in kept.iter().filter(|r| r.cie.is_some()) {
                let at = relocations.partition_point(|r| r.offset < record.start + 8);
                let function = relocations.get(at).filter(|r| r.offset == record.start + 8);
                if let Some(reloc) = function
                    && let Some(target) = reloc.target
                {
                    let start = (self.address(target) as i64 + reloc.addend) as u64;
                    fdes.push((start, base + moved[&record.start]));
                }
            }
        }
        Ok(fdes)
    }

    /// The file header, the program headers and the section headers, with
    /// the symbol table unless stripped.
    fn headers(&self, image: &mut Image, inputs: &Inputs, entry: usize) -> Result<(), String> {
        let at = |out: Out| self.offset[out as usize];
        let addr = |out: Out| self.addr[out as usize];
        let size = |out: Out| self.size[out as usize];
        let phnum = self.program_headers();
        // The program headers.
        let mut headers: Vec<(ProgramType, ProgramFlags, u64, u64, u64, u64)> = Vec::new();
        headers.push((PT_PHDR, PF_R, 64, BASE + 64, 56 * phnum as u64, 8));
        if self.dynamic && !self.library {
            headers.push((PT_INTERP, PF_R, at(Out::Interp), addr(Out::Interp), size(Out::Interp), 1));
        }
        let segments: [(ProgramFlags, Out, Out); 3] = [
            (PF_R, Out::Interp, Out::EhFrame),
            (PF_R | PF_X, Out::Plt, Out::Text),
            (PF_R | PF_W, Out::Tdata, Out::Bss),
        ];
        let mut loads = Vec::new();
        for (n, (flags, first, last)) in segments.iter().enumerate() {
            let start = if n == 0 { 0 } else { at(*first) };
            let end_file = OUTS
                .iter()
                .filter(|o| o.segment() == n && !o.nobits())
                .map(|&o| at(o) + size(o))
                .max()
                .unwrap_or(start)
                .max(start);
            let end_memory = if n == 2 { addr(*last) + size(*last) } else { BASE + end_file };
            loads.push((PT_LOAD, *flags, start, BASE + start, end_file - start, end_memory - (BASE + start)));
        }
        for (kind, flags, offset, vaddr, filesz, memsz) in loads {
            headers.push((kind, flags, offset, vaddr, filesz, memsz));
        }
        headers.push((PT_DYNAMIC, PF_R | PF_W, at(Out::Dynamic), addr(Out::Dynamic), size(Out::Dynamic), 8));
        let (tls_start, tls_size, tls_align) = self.tls;
        if size(Out::Tdata) + size(Out::Tbss) > 0 {
            let filesz = size(Out::Tdata);
            headers.push((PT_TLS, PF_R, at(Out::Tdata), tls_start, filesz, tls_size | (tls_align << 48)));
        }
        if size(Out::EhFrameHdr) > 0 {
            headers.push((PT_GNU_EH_FRAME, PF_R, at(Out::EhFrameHdr), addr(Out::EhFrameHdr), size(Out::EhFrameHdr), 4));
        }
        headers.push((PT_GNU_STACK, PF_R | PF_W, 0, 0, 0, 0));
        for (n, &(kind, flags, offset, vaddr, filesz, memsz)) in headers.iter().enumerate() {
            let h = 64 + 56 * n as u64;
            // The fields after the sizes: TLS packs its alignment in the
            // memory size's top bits above; others align as loads do.
            let (memsz, align) = match kind {
                PT_LOAD => (memsz, self.arch.page()),
                PT_TLS => (memsz & 0xffff_ffff_ffff, memsz >> 48),
                PT_PHDR | PT_DYNAMIC => (filesz, 8),
                PT_GNU_EH_FRAME => (filesz, 4),
                PT_GNU_STACK => (0, 16),
                _ => (filesz, 1),
            };
            image.u32(h, kind.0);
            image.u32(h + 4, flags.0);
            image.u64(h + 8, offset);
            image.u64(h + 16, vaddr);
            image.u64(h + 24, vaddr);
            image.u64(h + 32, filesz);
            image.u64(h + 40, memsz);
            image.u64(h + 48, align);
        }
        // The section headers, after a symbol table unless stripped.
        let mut shstrtab = vec![0u8];
        let mut name = |text: &str| {
            let at = shstrtab.len() as u32;
            shstrtab.extend_from_slice(text.as_bytes());
            shstrtab.push(0);
            at
        };
        let mut sections: Vec<[u64; 10]> = vec![[0; 10]];
        let mut index: HashMap<Out, u32> = HashMap::new();
        for out in OUTS {
            if size(out) == 0 && !(self.dynamic && matches!(out, Out::Dynamic | Out::Dynsym | Out::Dynstr)) {
                continue;
            }
            let (kind, flags) = out.header();
            index.insert(out, sections.len() as u32);
            let entsize = match out {
                Out::Dynsym => 24,
                Out::RelaDyn => 24,
                Out::Dynamic => 16,
                Out::Hash => 4,
                Out::Versym => 2,
                Out::Got | Out::InitArray | Out::FiniArray | Out::Relr => 8,
                _ => 0,
            };
            sections.push([
                name(out.name()) as u64,
                kind as u64,
                flags,
                addr(out),
                at(out),
                size(out),
                0,
                0,
                self.align[out as usize],
                entsize,
            ]);
        }
        let link = |out: Out| index.get(&out).copied().unwrap_or(0) as u64;
        for (out, linked, info) in [
            (Out::Dynsym, Out::Dynstr, 1),
            (Out::Hash, Out::Dynsym, 0),
            (Out::Versym, Out::Dynsym, 0),
            (Out::Verneed, Out::Dynstr, self.versions().iter().filter(|(_, v)| !v.is_empty()).count() as u64),
            (Out::RelaDyn, Out::Dynsym, 0),
            (Out::Dynamic, Out::Dynstr, 0),
        ] {
            if let Some(&i) = index.get(&out) {
                sections[i as usize][6] = link(linked);
                sections[i as usize][7] = info;
            }
        }
        let mut offset = image.bytes.len() as u64;
        if !inputs.strip {
            let (symtab, strtab, locals) = self.symbol_table(&index);
            offset = offset.next_multiple_of(8);
            let symtab_index = sections.len() as u64;
            sections.push([
                name(".symtab") as u64,
                SHT_SYMTAB.0 as u64,
                0,
                0,
                offset,
                symtab.len() as u64,
                symtab_index + 1,
                locals,
                8,
                24,
            ]);
            image.put(offset, &symtab);
            offset += symtab.len() as u64;
            sections.push([name(".strtab") as u64, SHT_STRTAB.0 as u64, 0, 0, offset, strtab.len() as u64, 0, 0, 1, 0]);
            image.put(offset, &strtab);
            offset += strtab.len() as u64;
        }
        let shstrndx = sections.len();
        let shstrtab_name = name(".shstrtab") as u64;
        sections.push([shstrtab_name, SHT_STRTAB.0 as u64, 0, 0, offset, 0, 0, 0, 1, 0]);
        sections[shstrndx][5] = shstrtab.len() as u64;
        image.put(offset, &shstrtab);
        offset = (offset + shstrtab.len() as u64).next_multiple_of(8);
        for (n, s) in sections.iter().enumerate() {
            let h = offset + 64 * n as u64;
            image.u32(h, s[0] as u32);
            image.u32(h + 4, s[1] as u32);
            image.u64(h + 8, s[2]);
            image.u64(h + 16, s[3]);
            image.u64(h + 24, s[4]);
            image.u64(h + 32, s[5]);
            image.u32(h + 40, s[6] as u32);
            image.u32(h + 44, s[7] as u32);
            image.u64(h + 48, s[8]);
            image.u64(h + 56, s[9]);
        }
        // The file header.
        let mut header = vec![0x7f, b'E', b'L', b'F', ELFCLASS64.0, ELFDATA2LSB.0, EV_CURRENT.0, ELFOSABI_NONE.0];
        header.resize(16, 0);
        image.put(0, &header);
        image.u16(16, ET_DYN.0);
        image.u16(18, self.arch.machine().0);
        image.u32(20, EV_CURRENT.0 as u32);
        image.u64(24, if self.library { 0 } else { self.global_address(entry) });
        image.u64(32, 64);
        image.u64(40, offset);
        image.u32(48, 0);
        image.u16(52, 64);
        image.u16(54, 56);
        image.u16(56, phnum as u16);
        image.u16(58, 64);
        image.u16(60, sections.len() as u16);
        image.u16(62, shstrndx as u16);
        Ok(())
    }

    /// The symbols of the live sections, locals first: the table, its
    /// strings and the number of locals.
    fn symbol_table(&self, index: &HashMap<Out, u32>) -> (Vec<u8>, Vec<u8>, u64) {
        let mut strtab = vec![0u8];
        let mut locals: Vec<u8> = vec![0; 24];
        let mut globals: Vec<u8> = Vec::new();
        let entry =
            |table: &mut Vec<u8>, strtab: &mut Vec<u8>, name: &[u8], info: u8, out: Out, value: u64, size: u64| {
                let at = strtab.len() as u32;
                strtab.extend_from_slice(name);
                strtab.push(0);
                table.extend_from_slice(&at.to_le_bytes());
                table.extend_from_slice(&[info, 0]);
                table.extend_from_slice(&(index.get(&out).copied().unwrap_or(0) as u16).to_le_bytes());
                table.extend_from_slice(&value.to_le_bytes());
                table.extend_from_slice(&size.to_le_bytes());
            };
        for (i, input) in self.inputs.iter().enumerate() {
            for symbol in input.file.symbols() {
                let SymbolSection::Section(section) = symbol.section() else { continue };
                let Some((out, _)) = input.place[section.0] else { continue };
                let kind = symbol.elf_symbol().st_type();
                if !matches!(kind, STT_FUNC | STT_OBJECT | STT_TLS | STT_NOTYPE) {
                    continue;
                }
                let Ok(name) = symbol.name_bytes() else { continue };
                if name.is_empty() || name.starts_with(b".L") {
                    continue;
                }
                let value = self.section_address(i as u32, section.0 as u32) + symbol.address();
                let value = if kind == STT_TLS { value - self.tls.0 } else { value };
                if symbol.is_local() {
                    entry(&mut locals, &mut strtab, name, (STB_LOCAL.0 << 4) | kind.0, out, value, symbol.size());
                } else if let Some(id) = input.globals[symbol.index().0]
                    && self.globals[id].def
                        == (Def::Section { input: i as u32, section: section.0 as u32, value: symbol.address() })
                {
                    let binding = if symbol.is_weak() { STB_WEAK } else { STB_GLOBAL };
                    entry(&mut globals, &mut strtab, name, (binding.0 << 4) | kind.0, out, value, symbol.size());
                }
            }
        }
        let count = locals.len() as u64 / 24;
        locals.extend_from_slice(&globals);
        (locals, strtab, count)
    }
}

fn addr_got(linker: &Linker) -> u64 {
    linker.addr[Out::Got as usize]
}

/// Why a library cannot hold a thread-local access.
const TLS: &str = "a shared library reaches its thread-local variables through the loader; compile the object as position-independent (`-fPIC`)";

/// Why an absolute address cannot be relocated in code.
const PIC: &str = "a position-independent executable cannot hold this absolute address; compile the object as position-independent (`-fPIC`)";

const DW_EH_PE_PCREL_SDATA4: u8 = 0x1b;
const DW_EH_PE_UDATA4: u8 = 0x03;
const DW_EH_PE_DATAREL_SDATA4: u8 = 0x3b;

/// The addresses of words to move, in `DT_RELR`'s form: an address, then
/// bitmaps of which of the next 63 words also move.
fn relr(addresses: &[u64]) -> Vec<u64> {
    let mut sorted = addresses.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        out.push(sorted[i]);
        let mut next = sorted[i] + 8;
        i += 1;
        loop {
            let mut bitmap = 0u64;
            while i < sorted.len() && sorted[i] >= next && sorted[i] - next < 63 * 8 {
                bitmap |= 1 << ((sorted[i] - next) / 8);
                i += 1;
            }
            if bitmap == 0 {
                break;
            }
            out.push((bitmap << 1) | 1);
            next += 63 * 8;
        }
    }
    out
}

/// A PLT entry: an indirect jump through the GOT entry at `slot`.
fn plt_entry(arch: Arch, stub: u64, slot: u64) -> Vec<u8> {
    match arch {
        Arch::X86_64 => {
            let mut code = vec![0xff, 0x25];
            code.extend_from_slice(&((slot as i64 - (stub + 6) as i64) as i32).to_le_bytes());
            code.resize(PLT_ENTRY as usize, 0xcc);
            code
        }
        Arch::Aarch64 => {
            let pages = ((slot & !0xfff) as i64 - (stub & !0xfff) as i64) >> 12;
            let pages = pages as u32;
            let adrp = 0x9000_0010 | ((pages & 3) << 29) | (((pages >> 2) & 0x7ffff) << 5);
            let ldr = 0xf940_0211 | ((((slot & 0xfff) >> 3) as u32) << 10);
            [adrp, ldr, 0xd61f_0220, 0xd503_201f].iter().flat_map(|w| w.to_le_bytes()).collect()
        }
    }
}
