//! A core the system wrote of a program, on Linux or macOS, read as a dump
//! (toolchain 7.5): the signal that ended it, and where each of its tasks and threads
//! was, by the trace table of the program's executable, and the numbers and
//! strings in its frames. A core is the memory of a process with no runtime
//! left to ask: what only the program's code can show, and counters, are not
//! read.

use crate::roots::Roots;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

/// What starts the trace table in a program's data, as the runtime's
/// `trace::Table`.
pub const MAGIC: &[u8; 16] = b"TONDOTRACE1\n\0\0\0\0";

/// What a Mach-O file starts with, 64 bits, little-endian.
const MACH_O: [u8; 4] = [0xcf, 0xfa, 0xed, 0xfe];
const MACH_ARM64: u32 = 0x0100_000c;
const MACH_X86_64: u32 = 0x0100_0007;
const MACH_SEGMENT: u32 = 0x19;
const MACH_THREAD: u32 = 4;

type Commands = Vec<(u32, Vec<u8>)>;

/// The load commands of a Mach-O file, by a reader of it: each one's kind
/// and bytes, and its processor, by the number ELF gives it.
fn mach_commands(header: &[u8], read: impl Fn(u64, usize) -> Option<Vec<u8>>) -> Option<(u16, Commands)> {
    let machine = match u32_at(header, 4)? {
        MACH_ARM64 => 183,
        MACH_X86_64 => 62,
        _ => return None,
    };
    let (count, size) = (u32_at(header, 16)? as usize, u32_at(header, 20)? as usize);
    let bytes = read(32, size)?;
    let (mut at, mut commands) = (0, Vec::with_capacity(count));
    for _ in 0..count {
        let (kind, length) = (u32_at(&bytes, at)?, u32_at(&bytes, at + 4)? as usize);
        commands.push((kind, bytes.get(at..at + length)?.to_vec()));
        at += length;
    }
    Some((machine, commands))
}

/// A segment of a load command.
fn mach_segment(command: &[u8]) -> Option<Segment> {
    Some(Segment { address: u64_at(command, 24)?, offset: u64_at(command, 40)?, size: u64_at(command, 48)? })
}

/// The threads and memory of a core of macOS.
fn mach_core(header: &[u8], read: impl Fn(u64, usize) -> Option<Vec<u8>>) -> Option<(u16, Notes, Vec<Segment>)> {
    if u32_at(header, 12)? != 4 {
        return None;
    }
    let (machine, commands) = mach_commands(header, read)?;
    let (mut found, mut kept) = (Notes::default(), Vec::new());
    for (kind, command) in &commands {
        match *kind {
            MACH_SEGMENT => kept.extend(mach_segment(command)),
            MACH_THREAD => {
                // Threads have no number here, and tasks count from one.
                let id = 1_000_000 + found.threads.len() as u32;
                found.threads.extend(mach_thread(command, machine, id));
            }
            _ => {}
        }
    }
    Some((machine, found, kept))
}

/// The state of a thread, of those a command holds, as Mach-O says it.
fn mach_thread(command: &[u8], machine: u16, id: u32) -> Option<Thread> {
    let mut at = 8;
    while at + 8 <= command.len() {
        let (flavor, count) = (u32_at(command, at)?, u32_at(command, at + 4)? as usize);
        let state = command.get(at + 8..at + 8 + count * 4)?;
        at += 8 + count * 4;
        let word = |index: usize| u64_at(state, index * 8);
        let mut registers = [None; 32];
        match (machine, flavor) {
            (183, 6) => {
                (0..31).for_each(|number| registers[number] = word(number));
                return Some(Thread { id, pc: word(32)?, sp: word(31)?, fp: word(29)?, link: word(30), registers });
            }
            (62, 4) => {
                let order = [0, 3, 2, 1, 5, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
                order.iter().enumerate().for_each(|(index, &number)| registers[number] = word(index));
                return Some(Thread { id, pc: word(16)?, sp: word(7)?, fp: word(6)?, link: None, registers });
            }
            _ => {}
        }
    }
    None
}

/// The processor and segments of an executable of macOS.
fn mach_program(bytes: &[u8]) -> Option<(u16, Vec<Segment>)> {
    if !bytes.starts_with(&MACH_O) {
        return None;
    }
    let read = |at: u64, length: usize| bytes.get(at as usize..at as usize + length).map(<[u8]>::to_vec);
    let (machine, commands) = mach_commands(bytes, read)?;
    let segments = commands.iter().filter(|(kind, _)| *kind == MACH_SEGMENT);
    Some((machine, segments.filter_map(|(_, command)| mach_segment(command)).collect()))
}

/// What starts a live task in memory, as the runtime's `tasks::Task`, and
/// where its stack pointer, number, state and whether it is the main task
/// are from there.
const TASK: u64 = u64::from_le_bytes(*b"TONDOTSK");
const TASK_FIELDS: usize = 40;
const PARKED: u8 = 2;

/// What starts the events in memory, as the runtime's `events::Text`: how
/// much it wrote, and that many bytes of a ring of `EVENTS`.
const EVENTS_MARK: &[u8; 16] = b"TONDOEVENTS1\n\0\0\0";
const EVENTS: usize = 1 << 16;

/// What starts the text of the bindings of a build without probes
/// (`codegen::dump_table`), in a string of the runtime: its length is two
/// words before, and what is shown of a string is at most `SHOWN` long.
pub const BINDINGS: &str = "TONDOBINDINGS1\n";
const STRING_HEADER: u64 = 24;
const SHOWN: u64 = 4096;

/// The longest stack a dump keeps.
const FRAMES: usize = 256;
const CALL: usize = 24;
const RECORD: usize = 24;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// A segment of an ELF file: where it goes in memory, where it is in the
/// file and how much of it the file holds.
#[derive(Clone, Copy)]
struct Segment {
    address: u64,
    offset: u64,
    size: u64,
}

/// The segments of a type in the ELF header `header` starts with.
fn segments(header: &[u8], read: impl Fn(u64, usize) -> Option<Vec<u8>>, kind: u32) -> Option<Vec<Segment>> {
    let (at, size, count) = (u64_at(header, 0x20)?, u16_at(header, 0x36)? as u64, u16_at(header, 0x38)? as u64);
    let table = read(at, (size * count) as usize)?;
    let mut found = Vec::new();
    for entry in (0..count as usize).map(|index| index * size as usize) {
        if u32_at(&table, entry)? == kind {
            let (offset, address) = (u64_at(&table, entry + 8)?, u64_at(&table, entry + 16)?);
            found.push(Segment { address, offset, size: u64_at(&table, entry + 32)? });
        }
    }
    Some(found)
}

/// A thread as the core has it.
struct Thread {
    id: u32,
    pc: u64,
    sp: u64,
    fp: u64,
    /// Where a call on aarch64 leaves its return address.
    link: Option<u64>,
    /// Its registers, by their DWARF numbers.
    registers: [Option<u64>; 32],
}

/// A file mapped into the process: its addresses, where they are in it,
/// and its path.
struct Mapped {
    start: u64,
    end: u64,
    offset: u64,
    path: String,
}

/// What the notes of a core say.
#[derive(Default)]
struct Notes {
    threads: Vec<Thread>,
    signal: u32,
    code: i32,
    address: u64,
    entry: u64,
    files: Vec<Mapped>,
    command: String,
}

/// The notes in `bytes`, for a machine of ELF number `machine`.
fn notes(bytes: &[u8], machine: u16, found: &mut Notes) -> Option<()> {
    let mut at = 0;
    while at + 12 <= bytes.len() {
        let (name, size, kind) = (u32_at(bytes, at)? as usize, u32_at(bytes, at + 4)? as usize, u32_at(bytes, at + 8)?);
        let start = at + 12 + name.next_multiple_of(4);
        let note = bytes.get(start..start + size)?;
        at = start + size.next_multiple_of(4);
        match kind {
            1 => {
                let register = |index: usize| u64_at(note, 112 + index * 8);
                let id = u32_at(note, 32)?;
                // Not all that write cores say the signal elsewhere.
                if found.threads.is_empty() {
                    found.signal = u16_at(note, 12)? as u32;
                }
                let mut registers = [None; 32];
                if machine == 62 {
                    let order = [10, 12, 11, 5, 13, 14, 4, 9, 8, 7, 6, 3, 2, 1, 0];
                    let numbers = [0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15];
                    numbers.iter().zip(order).for_each(|(&number, index)| registers[number] = register(index));
                } else {
                    (0..31).for_each(|number| registers[number] = register(number));
                }
                found.threads.push(match machine {
                    62 => Thread { id, pc: register(16)?, sp: register(19)?, fp: register(4)?, link: None, registers },
                    _ => Thread {
                        id,
                        pc: register(32)?,
                        sp: register(31)?,
                        fp: register(29)?,
                        link: register(30),
                        registers,
                    },
                });
            }
            3 => {
                let text = note.get(56..136)?;
                let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
                found.command = String::from_utf8_lossy(&text[..end]).trim().to_string();
            }
            6 => {
                let pairs = note.as_chunks::<16>().0.iter();
                let pairs = pairs.filter_map(|pair| Some((u64_at(pair, 0)?, u64_at(pair, 8)?)));
                found.entry = pairs.into_iter().find(|&(key, _)| key == 9).map_or(0, |(_, value)| value);
            }
            0x4649_4c45 => {
                let (count, page) = (u64_at(note, 0)? as usize, u64_at(note, 8)?);
                let mut names = note.get(16 + count * 24..)?.split(|&b| b == 0);
                for entry in (0..count).map(|index| 16 + index * 24) {
                    found.files.push(Mapped {
                        start: u64_at(note, entry)?,
                        end: u64_at(note, entry + 8)?,
                        offset: u64_at(note, entry + 16)? * page,
                        path: String::from_utf8_lossy(names.next()?).into_owned(),
                    });
                }
            }
            0x5349_4749 => {
                found.signal = u32_at(note, 0)?;
                found.code = u32_at(note, 8)? as i32;
                found.address = u64_at(note, 16)?;
            }
            _ => {}
        }
    }
    Some(())
}

/// The memory of the process: what the core kept, and what it did not
/// because it is in the executable, placed `bias` from where the file says.
struct Memory<'a> {
    core: &'a std::fs::File,
    kept: Vec<Segment>,
    program: &'a [u8],
    loads: Vec<Segment>,
    bias: u64,
}

impl<'a> Memory<'a> {
    /// Reads from `at` into `into` as far as one segment goes: how far.
    fn piece(&self, at: u64, into: &mut [u8]) -> Option<usize> {
        if let Some(kept) = self.kept.iter().find(|s| s.address <= at && at < s.address + s.size) {
            let length = into.len().min((kept.address + kept.size - at) as usize);
            self.core.read_exact_at(&mut into[..length], kept.offset + at - kept.address).ok()?;
            return Some(length);
        }
        let file = self.in_program(at.wrapping_sub(self.bias))?;
        let length = into.len().min(file.len());
        into[..length].copy_from_slice(&file[..length]);
        Some(length)
    }

    fn read(&self, mut at: u64, length: usize) -> Option<Vec<u8>> {
        let mut bytes = vec![0; length];
        let mut done = 0;
        while done < length {
            let got = self.piece(at, &mut bytes[done..])?;
            done += got;
            at += got as u64;
        }
        Some(bytes)
    }

    fn word(&self, at: u64) -> Option<u64> {
        u64_at(&self.read(at, 8)?, 0)
    }

    /// What the executable has from a linked address to the end of its
    /// segment.
    fn in_program(&self, at: u64) -> Option<&'a [u8]> {
        let load = self.loads.iter().find(|s| s.address <= at && at < s.address + s.size)?;
        self.program.get((load.offset + at - load.address) as usize..(load.offset + load.size) as usize)
    }
}

/// The trace table of the program, as `trace` reads it in the runtime.
struct Code<'a> {
    /// Each function: its code and size, where its entry is, and its number.
    functions: Vec<(u64, u32, u32, usize)>,
    blob: &'a [u8],
    files: usize,
}

impl<'a> Code<'a> {
    fn of(memory: &Memory<'a>) -> Option<Code<'a>> {
        let found = memory.program.windows(MAGIC.len()).position(|window| window == MAGIC)? as u64;
        let load = memory.loads.iter().find(|s| s.offset <= found && found < s.offset + s.size)?;
        let table = memory.read(load.address + found - load.offset + memory.bias, 64)?;
        let (entries, count) = (u64_at(&table, 16)?, u64_at(&table, 24)? as usize);
        let (blob, files) = (u64_at(&table, 32)?, u64_at(&table, 40)? as usize);
        let entries = memory.read(entries, count.min(memory.program.len()) * 16)?;
        let mut functions = Vec::with_capacity(count);
        for (index, entry) in entries.as_chunks::<16>().0.iter().enumerate() {
            functions.push((u64_at(entry, 0)?, u32_at(entry, 8)?, u32_at(entry, 12)?, index));
        }
        functions.sort();
        Some(Code { functions, blob: memory.in_program(blob.wrapping_sub(memory.bias))?, files })
    }

    /// The function whose code holds the byte before `address`.
    fn function(&self, address: u64) -> Option<(u64, usize)> {
        let at = self.functions.partition_point(|f| f.0 < address).checked_sub(1)?;
        let (code, size, meta, _) = self.functions[at];
        (address <= code.wrapping_add(size as u64)).then_some((code, meta as usize))
    }

    fn text(&self, at: usize, length: usize) -> Option<String> {
        Some(String::from_utf8_lossy(self.blob.get(at..at + length)?).into_owned())
    }

    fn named(&self, index: u32) -> Option<String> {
        let mut at = self.files + 4;
        for _ in 0..index {
            at = (at + 4 + u32_at(self.blob, at)? as usize + 3) & !3;
        }
        self.text(at + 4, u32_at(self.blob, at)? as usize)
    }

    /// Where a function's calls are, and how many.
    fn calls(&self, meta: usize) -> Option<(usize, usize)> {
        let name = u32_at(self.blob, meta)? as usize;
        Some(((meta + 8 + name + 3) & !3, u32_at(self.blob, meta + 4)? as usize))
    }

    /// Where a function's lines are: their file, start and end.
    fn lines(&self, meta: usize) -> Option<(u32, usize, usize)> {
        let (calls, count) = self.calls(meta)?;
        let lines = calls + count * CALL;
        Some((u32_at(self.blob, lines)?, lines + 8, lines + 8 + u32_at(self.blob, lines + 4)? as usize))
    }

    fn record(&self, meta: usize, index: u32) -> Option<usize> {
        let (_, _, end) = self.lines(meta)?;
        Some(((end + 3) & !3) + index as usize * RECORD)
    }

    /// The name of the function inlined as `record`, or the function's.
    fn name(&self, meta: usize, record: u32) -> Option<String> {
        match record {
            u32::MAX => self.text(meta + 8, u32_at(self.blob, meta)? as usize),
            index => self.named(u32_at(self.blob, self.record(meta, index)?)?),
        }
    }

    /// Whether `address` is where a call of compiled code returns to.
    fn call(&self, address: u64) -> Option<usize> {
        let (start, meta) = self.function(address)?;
        let (sites, calls) = self.calls(meta)?;
        let offset = (address - start) as u32;
        (0..calls).map(|k| sites + k * CALL).find(|&site| u32_at(self.blob, site) == Some(offset))
    }

    /// The line of `pc`, and the inlined call it is in.
    fn line(&self, pc: u64) -> Option<(u32, u32, u32)> {
        let (start, meta) = self.function(pc + 1)?;
        let (file, mut cursor, end) = self.lines(meta)?;
        let mut number = || {
            let (mut value, mut shift) = (0u64, 0);
            while cursor < end {
                let byte = self.blob[cursor];
                cursor += 1;
                value |= ((byte & 0x7f) as u64) << shift;
                shift += 7;
                if byte & 0x80 == 0 {
                    break;
                }
            }
            (value, cursor < end)
        };
        let (mut at, mut line, mut found, offset) = (0u64, 0i64, (0i64, 0u64), pc - start);
        loop {
            let (step, _) = number();
            at += step;
            let (zigzag, _) = number();
            line += (zigzag >> 1) as i64 ^ -((zigzag & 1) as i64);
            let (inlined, more) = number();
            if at > offset {
                break;
            }
            found = (line, inlined);
            if !more {
                break;
            }
        }
        let record = (found.1 as u32).wrapping_sub(1);
        let file = match record {
            u32::MAX => file,
            index => u32_at(self.blob, self.record(meta, index)? + 20)?,
        };
        (found.0 > 0 && file != u32::MAX).then_some((file, found.0 as u32, record))
    }

    /// The number of a function, where its code starts.
    fn number(&self, start: u64) -> Option<usize> {
        let at = self.functions.binary_search_by_key(&start, |f| f.0).ok()?;
        Some(self.functions[at].3)
    }

    /// The frames code stands for, from the innermost out: each with its
    /// inlined call, `u32::MAX` for the function's own, and its place.
    fn scopes(&self, address: u64, exact: bool) -> Option<Vec<(u32, String)>> {
        let (_, meta) = self.function(if exact { address.wrapping_add(1) } else { address })?;
        let site = if exact { None } else { self.call(address) };
        let (file, line, record) = match site {
            Some(site) => (u32_at(self.blob, site + 12)?, u32_at(self.blob, site + 4)?, u32_at(self.blob, site + 20)?),
            None => self.line(if exact { address } else { address - 1 })?,
        };
        let mut scopes = vec![(record, format!("{}:{line} in {}", self.named(file)?, self.name(meta, record)?))];
        let mut outer = record;
        while outer != u32::MAX {
            let at = self.record(meta, outer)?;
            let parent = u32_at(self.blob, at + 16)?;
            let (file, line) = (self.named(u32_at(self.blob, at + 12)?)?, u32_at(self.blob, at + 4)?);
            scopes.push((parent, format!("{file}:{line} in {}", self.name(meta, parent)?)));
            outer = parent;
        }
        Some(scopes)
    }

    /// A frame as a dump has it: at `address`, the next instruction to run
    /// when `exact` and otherwise where a call returns to.
    fn frame(&self, address: u64, exact: bool) -> Option<Value> {
        let (_, meta) = self.function(if exact { address.wrapping_add(1) } else { address })?;
        let site = if exact { None } else { self.call(address) };
        let (file, line, record) = match site {
            Some(site) => (u32_at(self.blob, site + 12)?, u32_at(self.blob, site + 4)?, u32_at(self.blob, site + 20)?),
            None => self.line(if exact { address } else { address - 1 })?,
        };
        let mut inlined = Vec::new();
        let mut outer = record;
        while outer != u32::MAX {
            let at = self.record(meta, outer)?;
            let parent = u32_at(self.blob, at + 16)?;
            inlined.push(json!({
                "function": self.name(meta, parent)?,
                "file": self.named(u32_at(self.blob, at + 12)?)?,
                "line": u32_at(self.blob, at + 4)?,
            }));
            outer = parent;
        }
        Some(
            json!({ "function": self.name(meta, record)?, "file": self.named(file)?, "line": line, "inlined": inlined }),
        )
    }
}

/// Where a binding is over a run of code.
#[derive(Clone, Copy)]
enum Located {
    Frame(i64),
    Register(usize),
}

/// What the table of a build without probes says of a binding.
struct Bound {
    name: String,
    ty: String,
    slot: Option<i64>,
    pointer: bool,
    ranges: Vec<(u32, u32, Located)>,
    scope: u32,
    from: u32,
}

/// What it says of a function: the registers it saved, and where, and its
/// bindings.
#[derive(Default)]
struct Bindings {
    saved: Vec<(usize, i64)>,
    bound: Vec<Bound>,
}

/// The bindings of each function, by its number, from the text a build
/// without probes keeps.
fn bindings(program: &[u8]) -> HashMap<usize, Bindings> {
    let mut functions: HashMap<usize, Bindings> = HashMap::new();
    let mark = BINDINGS.as_bytes();
    let Some(start) = program.windows(mark.len()).position(|window| window == mark) else { return functions };
    let length = start.checked_sub(16).and_then(|at| u64_at(program, at)).unwrap_or(0) as usize;
    let Some(text) = program.get(start..start + length) else { return functions };
    let (text, mut current) = (String::from_utf8_lossy(text), 0);
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let function = functions.entry(current).or_default();
        match fields[..] {
            ["F", index] => current = index.parse().unwrap_or(0),
            ["C", register, at] => function.saved.extend(register.parse().ok().zip(at.parse().ok())),
            ["B", name, ty, kind, value, _, scope, from] => {
                let value: i64 = value.parse().unwrap_or(0);
                function.bound.push(Bound {
                    name: name.to_string(),
                    ty: ty.to_string(),
                    slot: (kind == "s").then_some(value),
                    pointer: kind != "s" && value == 1,
                    ranges: Vec::new(),
                    scope: scope.parse().unwrap_or(u32::MAX),
                    from: from.parse().unwrap_or(0),
                });
            }
            ["R", start, end, kind, at] => {
                let place = match kind {
                    "f" => at.parse().ok().map(Located::Frame),
                    "r" => at.parse().ok().map(Located::Register),
                    _ => None,
                };
                if let (Ok(start), Ok(end), Some(place), Some(bound)) =
                    (start.parse(), end.parse(), place, function.bound.last_mut())
                {
                    bound.ranges.push((start, end, place));
                }
            }
            _ => {}
        }
    }
    functions
}

/// What a value of a type the program did not define reads as: the rest
/// have a form only the program's code knows.
fn decoded(ty: &str, word: u64, memory: &Memory) -> Option<String> {
    Some(match ty {
        "Int" | "Int64" => (word as i64).to_string(),
        "Int32" => (word as i32).to_string(),
        "Int16" => (word as i16).to_string(),
        "Int8" => (word as i8).to_string(),
        "UInt" | "UInt64" => word.to_string(),
        "UInt32" => (word as u32).to_string(),
        "UInt16" => (word as u16).to_string(),
        "UInt8" | "Byte" => (word as u8).to_string(),
        "Bool" => (word as u8 != 0).to_string(),
        "Float" | "Float64" => format!("{:?}", f64::from_bits(word)),
        "Float32" => format!("{:?}", f32::from_bits(word as u32)),
        "Char" => format!("{:?}", char::from_u32(word as u32)?),
        "String" => {
            let length = memory.word(word.checked_add(8)?)?;
            let bytes = memory.read(word.checked_add(STRING_HEADER)?, length.min(SHOWN) as usize)?;
            serde_json::to_string(&String::from_utf8_lossy(&bytes)).ok()?
        }
        _ => return None,
    })
}

impl Bound {
    /// The word of the binding in the frame at `fp`, at `offset` of its code.
    fn word(&self, fp: u64, offset: u32, registers: &[Option<u64>; 32], memory: &Memory) -> Option<u64> {
        if let Some(slot) = self.slot {
            return memory.word(fp.wrapping_add_signed(slot));
        }
        let &(.., place) = self.ranges.iter().find(|&&(start, end, _)| start <= offset && offset < end)?;
        let word = match place {
            Located::Frame(at) => memory.word(fp.wrapping_add_signed(16 + at))?,
            Located::Register(number) => (*registers.get(number)?)?,
        };
        match self.pointer {
            true => memory.word(word),
            false => Some(word),
        }
    }
}

/// Where a stack starts: the frame and code of what stands innermost, the
/// next instruction when `exact` and otherwise where it goes on, with the
/// registers known there.
struct Start {
    fp: u64,
    pc: Option<u64>,
    exact: bool,
    registers: [Option<u64>; 32],
}

/// The frame tables of the executable, which say what a frame of the
/// runtime saved, and where.
struct Unwinder<'a> {
    frames: gimli::EhFrame<gimli::EndianSlice<'a, gimli::LittleEndian>>,
    bases: gimli::BaseAddresses,
    bias: u64,
}

impl<'a> Unwinder<'a> {
    fn of(program: &'a [u8], bias: u64) -> Option<Unwinder<'a>> {
        use cranelift_object::object::{Object, ObjectSection};
        let file = cranelift_object::object::File::parse(program).ok()?;
        let section = file.section_by_name(".eh_frame")?;
        let frames = gimli::EhFrame::new(section.data().ok()?, gimli::LittleEndian);
        let bases = gimli::BaseAddresses::default().set_eh_frame(section.address());
        Some(Unwinder { frames, bases, bias })
    }

    /// The registers of the caller of a frame at `fp` of code that goes on
    /// at `address`, where it saved them.
    fn restored(&self, address: u64, fp: u64, registers: &mut [Option<u64>; 32], memory: &Memory) -> Option<()> {
        use gimli::UnwindSection;
        let mut context = gimli::UnwindContext::new();
        let at = address.wrapping_sub(self.bias).wrapping_sub(1);
        let row =
            self.frames.unwind_info_for_address(&self.bases, &mut context, at, gimli::EhFrame::cie_from_offset).ok()?;
        // Frames keep their pointer, so what is saved is relative to it.
        let frame = fp.wrapping_add(16);
        for (register, rule) in row.registers() {
            if let (gimli::RegisterRule::Offset(offset), Some(slot)) = (rule, registers.get_mut(register.0 as usize)) {
                *slot = memory.word(frame.wrapping_add_signed(*offset));
            }
        }
        Some(())
    }
}

/// What reading values takes.
struct Reading<'a> {
    tables: HashMap<usize, Bindings>,
    unwinder: Option<Unwinder<'a>>,
    callee: &'static [usize],
}

/// The numbers and strings of the frames of a task: as `dump` has them,
/// but for what only the program's code can show.
fn values(task: u64, start: Start, memory: &Memory, code: &Code, reading: &Reading) -> Vec<Value> {
    let Start { mut fp, pc, exact, mut registers } = start;
    let mut frames = Vec::new();
    frames.extend(pc.map(|pc| (fp, pc, exact)));
    while frames.len() < FRAMES && fp != 0 {
        let (Some(next), Some(back)) = (memory.word(fp), memory.word(fp.wrapping_add(8))) else { break };
        if back == 0 || next == fp {
            break;
        }
        frames.push((next, back, false));
        fp = next;
    }
    let (mut found, mut at) = (Vec::new(), 0);
    for (fp, address, exact) in frames {
        let place = code.function(if exact { address.wrapping_add(1) } else { address });
        let Some((start, _)) = place else {
            // A frame of the runtime says where it saved what it used.
            let restored = reading.unwinder.as_ref().and_then(|u| u.restored(address, fp, &mut registers, memory));
            if restored.is_none() {
                registers = [None; 32];
            }
            continue;
        };
        let offset = (if exact { address } else { address - 1 }).wrapping_sub(start) as u32;
        let table = code.number(start).and_then(|number| reading.tables.get(&number));
        for (record, place) in code.scopes(address, exact).unwrap_or_default() {
            if place.contains(" in spawn") {
                continue;
            }
            let line = place.split(" in ").next().and_then(|at| at.rsplit(':').next());
            let line: u32 = line.and_then(|line| line.parse().ok()).unwrap_or(u32::MAX);
            for bound in table.iter().flat_map(|t| &t.bound).filter(|b| b.scope == record && b.from <= line) {
                let word = bound.word(fp, offset, &registers, memory);
                if let Some(value) = word.and_then(|word| decoded(&bound.ty, word, memory)) {
                    found.push(json!({
                        "task": task, "frame": at, "at": place, "name": bound.name, "type": bound.ty,
                        "value": value, "parts": [],
                    }));
                }
            }
            at += 1;
        }
        let mut caller = [None; 32];
        reading.callee.iter().for_each(|&register| caller[register] = registers[register]);
        for &(register, saved) in table.iter().flat_map(|t| &t.saved) {
            if let Some(slot) = caller.get_mut(register) {
                *slot = memory.word(fp.wrapping_add_signed(16 + saved));
            }
        }
        registers = caller;
    }
    found
}

/// Adds to `stack` what the chain of frame pointers compiled code and the
/// runtime keep says, from `frame` out.
fn follow(mut frame: u64, memory: &Memory, stack: &mut Vec<u64>) {
    while stack.len() < FRAMES && frame != 0 {
        let (Some(next), Some(back)) = (memory.word(frame), memory.word(frame.wrapping_add(8))) else { break };
        if back == 0 || next == frame {
            break;
        }
        if stack.last() != Some(&back) {
            stack.push(back);
        }
        frame = next;
    }
}

/// The addresses of a thread's stack, from the innermost out. C code may
/// not keep the chain: what it was called from is then the first return
/// address of compiled code above it.
fn stack(thread: &Thread, memory: &Memory, code: &Code) -> Vec<u64> {
    let mut stack = vec![thread.pc];
    if code.function(thread.pc.wrapping_add(1)).is_none() {
        let end = thread.fp.min(thread.sp.saturating_add(0x10000));
        let above = (thread.sp..end).step_by(8).filter_map(|at| memory.word(at));
        let caller = thread.link.into_iter().chain(above).find(|&word| code.call(word).is_some());
        stack.extend(caller);
    }
    follow(thread.fp, memory, &mut stack);
    stack
}

/// A task or a thread as the dump has it.
struct Row {
    id: u64,
    main: bool,
    state: &'static str,
    stack: Vec<u64>,
    stopped: bool,
    wait: Option<Value>,
    cancelled: bool,
}

/// A task in memory: its saved stack pointer, number and state, whether it
/// is the main task or cancelled, and what it is blocked on.
struct Task {
    sp: u64,
    id: u64,
    state: u8,
    main: bool,
    cancelled: bool,
    blocked: (u8, u64),
}

impl Task {
    /// What it waits for, as a dump says it.
    fn wait(&self) -> Option<Value> {
        let what = match self.blocked.0 {
            1 => "a std.sync lock",
            2 => "a value of a channel",
            3 => "room in a channel",
            4 => "a task to end",
            _ => return None,
        };
        Some(json!({ "for": what, "on": [], "object": format!("{:#x}", self.blocked.1), "ms": 0 }))
    }
}

/// The events of a program that kept them, from the oldest, each a line of
/// JSON: the ring at `at`, whose first line may be cut.
fn events_at(memory: &Memory, at: u64) -> Vec<Value> {
    let Some(written) = memory.word(at + 16) else { return Vec::new() };
    let Some(ring) = memory.read(at + 24, EVENTS) else { return Vec::new() };
    let start = written as usize % EVENTS;
    let text: Vec<u8> = match written as usize > EVENTS {
        true => [&ring[start..], &ring[..start]].concat(),
        false => ring[..written as usize].to_vec(),
    };
    let lines = text.split(|&byte| byte == b'\n');
    lines.filter_map(|line| serde_json::from_slice(line).ok()).collect()
}

/// The tasks alive in the memory a core kept, each found by its mark: the
/// runtime's lists would need the layout of what holds them. And where the
/// events are.
fn tasks(memory: &Memory) -> (Vec<Task>, Option<u64>) {
    const CHUNK: usize = 1 << 22;
    let mut found = Vec::new();
    let mut events = None;
    let events_mark = u64::from_le_bytes(EVENTS_MARK[..8].try_into().expect("eight bytes"));
    for segment in &memory.kept {
        let mut bytes = vec![0; CHUNK.min(segment.size as usize)];
        let mut done = 0;
        while done < segment.size {
            let length = bytes.len().min((segment.size - done) as usize);
            if memory.core.read_exact_at(&mut bytes[..length], segment.offset + done).is_err() {
                break;
            }
            for (index, word) in bytes[..length].as_chunks::<8>().0.iter().enumerate() {
                let word = u64::from_le_bytes(*word);
                if word != TASK && word != events_mark {
                    continue;
                }
                let place = segment.address + done + index as u64 * 8;
                if word == events_mark {
                    if memory.read(place, 16).is_some_and(|mark| mark == EVENTS_MARK) {
                        events = Some(place);
                    }
                    continue;
                }
                let Some(fields) = memory.read(place, TASK_FIELDS) else { continue };
                let (sp, id) = (u64_at(&fields, 8).unwrap_or(0), u64_at(&fields, 16).unwrap_or(0));
                if id != 0 && id < 1 << 48 {
                    let blocked = (fields[27], u64_at(&fields, 32).unwrap_or(0));
                    found.push(Task {
                        sp,
                        id,
                        state: fields[24],
                        main: fields[25] != 0,
                        cancelled: fields[26] != 0,
                        blocked,
                    });
                }
            }
            done += length as u64;
        }
    }
    found.sort_by_key(|task| task.id);
    (found, events)
}

fn signal_name(signal: u32) -> String {
    let name = match signal {
        0 => return "taken from a program that ran".to_string(),
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        11 => "SIGSEGV",
        31 => "SIGSYS",
        _ => return format!("signal {signal}"),
    };
    format!("{name} (signal {signal})")
}

/// The core at `path` as a dump, its executable where the core says unless
/// `executable` says otherwise.
pub fn read(path: &Path, executable: Option<&Path>) -> Result<Value, String> {
    let shown = path.display();
    let not_core = || format!("{shown} is not a core of a 64-bit program of Linux or macOS");
    let file = std::fs::File::open(path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    let mut header = [0; 64];
    file.read_exact_at(&mut header, 0).map_err(|_| not_core())?;
    if header.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return Err(format!("{shown} is compressed: `zstd -d` or `coredumpctl dump -o <file>` writes it whole"));
    }
    let from_core = |at: u64, length: usize| {
        let mut bytes = vec![0; length];
        file.read_exact_at(&mut bytes, at).ok().map(|()| bytes)
    };
    let macho = header.starts_with(&MACH_O);
    let (machine, found, mut kept) = match macho {
        true => mach_core(&header, from_core).ok_or_else(not_core)?,
        false => {
            let machine = u16_at(&header, 0x12).unwrap_or(0);
            let elf = header.starts_with(b"\x7fELF\x02\x01") && u16_at(&header, 0x10) == Some(4);
            if !elf || ![62, 183].contains(&machine) {
                return Err(not_core());
            }
            let mut found = Notes::default();
            for note in segments(&header, from_core, 4).ok_or_else(not_core)? {
                let bytes =
                    from_core(note.offset, note.size as usize).ok_or_else(|| format!("{shown} is cut short"))?;
                notes(&bytes, machine, &mut found).ok_or_else(not_core)?;
            }
            (machine, found, segments(&header, from_core, 1).ok_or_else(not_core)?)
        }
    };
    kept.retain(|segment| segment.size > 0);
    if found.threads.is_empty() {
        return Err(format!("{shown} has no threads"));
    }

    let mapped = found.files.iter().find(|file| file.start <= found.entry && found.entry < file.end);
    let program = match (executable, mapped) {
        (Some(path), _) => path.to_path_buf(),
        (None, Some(mapped)) => PathBuf::from(&mapped.path),
        (None, None) => return Err(format!("{shown} does not say its program: name it with `--executable <file>`")),
    };
    let bytes = std::fs::read(&program).map_err(|e| {
        format!("cannot read the program of this core, {}: {e}; name it with `--executable <file>`", program.display())
    })?;
    let from_program = |at: u64, length: usize| bytes.get(at as usize..at as usize + length).map(<[u8]>::to_vec);
    let wrong = || format!("{} is not a program for this core", program.display());
    // Where the program is, and where the core kept its first page, so that
    // a build that is not this one shows.
    let (loads, bias, first) = match macho {
        true => {
            let (cpu, loads) = mach_program(&bytes).ok_or_else(wrong)?;
            // Not `__PAGEZERO`, which is at no offset either and holds nothing.
            let linked = loads.iter().find(|load| load.offset == 0 && load.size > 0).ok_or_else(wrong)?.address;
            let executable_header = |segment: &&Segment| {
                let start = from_core(segment.offset, 16);
                start.is_some_and(|start| start.starts_with(&MACH_O) && u32_at(&start, 12) == Some(2))
            };
            let base = kept.iter().find(executable_header).filter(|_| cpu == machine).ok_or_else(wrong)?;
            (loads, base.address.wrapping_sub(linked), Some(base.address))
        }
        false => {
            let same = bytes.starts_with(b"\x7fELF\x02\x01") && u16_at(&bytes, 0x12) == Some(machine);
            let loads = same.then(|| segments(&bytes, from_program, 1)).flatten().ok_or_else(wrong)?;
            let entry = u64_at(&bytes, 0x18).ok_or_else(wrong)?;
            let own = mapped.map(|mapped| mapped.path.as_str());
            let first = found.files.iter().find(|file| file.offset == 0 && Some(file.path.as_str()) == own);
            (loads, found.entry.wrapping_sub(entry), first.map(|first| first.start))
        }
    };
    let memory = Memory { core: &file, kept, program: &bytes, loads, bias };
    if let Some(kept) = first.and_then(|first| memory.kept.iter().find(|s| s.address == first)) {
        let length = kept.size.min(4096) as usize;
        if from_core(kept.offset, length).as_deref() != bytes.get(..length) {
            let name = program.display();
            return Err(format!("{name} is not the build this core is of: give that one with `--executable <file>`"));
        }
    }
    let code = Code::of(&memory).ok_or_else(|| {
        format!("{} is not a Tondo program of this version, or the core is cut short", program.display())
    })?;

    // Each task: on a thread when its stack is the one the thread is on,
    // where the thread says; otherwise where it saved its registers.
    let same_stack = |first: u64, second: u64| {
        let holds = |segment: &&Segment, at: u64| segment.address <= at && at < segment.address + segment.size;
        memory.kept.iter().find(|segment| holds(segment, first)).is_some_and(|segment| holds(&segment, second))
    };
    let saved = if machine == 62 { 5 * 8 } else { 10 * 8 };
    let callee: &'static [usize] =
        if machine == 62 { &[3, 12, 13, 14, 15] } else { &[19, 20, 21, 22, 23, 24, 25, 26, 27, 28] };
    let reading = Reading { tables: bindings(&bytes), unwinder: Unwinder::of(&bytes, bias), callee };
    let of_thread = |task: u64, thread: &Thread| {
        let start = Start { fp: thread.fp, pc: Some(thread.pc), exact: true, registers: thread.registers };
        values(task, start, &memory, &code, &reading)
    };
    // What `switch` left on a task's stack: what it saved, in the order of
    // the registers it says, and where the task goes on.
    let switched = |sp: u64| {
        let mut registers = [None; 32];
        let order: &[usize] = if machine == 62 { &[15, 14, 13, 12, 3] } else { callee };
        for (slot, &register) in order.iter().enumerate() {
            registers[register] = memory.word(sp.wrapping_add(slot as u64 * 8));
        }
        (registers, memory.word(sp.wrapping_add(saved + 8)))
    };
    let mut bound = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    let threads: Vec<(&Thread, Vec<u64>)> =
        found.threads.iter().map(|thread| (thread, stack(thread, &memory, &code))).collect();
    let mut placed = vec![false; threads.len()];
    let (found_tasks, events) = tasks(&memory);
    for task in found_tasks {
        let thread = threads.iter().position(|(thread, _)| same_stack(task.sp, thread.sp));
        match thread.filter(|_| task.state != PARKED) {
            Some(index) => {
                placed[index] = true;
                bound.extend(of_thread(task.id, threads[index].0));
                rows.push(Row {
                    id: task.id,
                    main: task.main,
                    state: "runs",
                    stack: threads[index].1.clone(),
                    stopped: index == 0,
                    wait: None,
                    cancelled: task.cancelled,
                });
            }
            None => {
                let mut stack = Vec::new();
                let fp = memory.word(task.sp.wrapping_add(saved)).unwrap_or(0);
                follow(fp, &memory, &mut stack);
                let (registers, pc) = switched(task.sp);
                let start = Start { fp, pc, exact: false, registers };
                bound.extend(values(task.id, start, &memory, &code, &reading));
                let state = if task.state == PARKED { "waits" } else { "ready" };
                rows.push(Row {
                    id: task.id,
                    main: task.main,
                    state,
                    stack,
                    stopped: false,
                    wait: task.wait(),
                    cancelled: task.cancelled,
                });
            }
        }
    }
    // A thread with no task, as one in the runtime or in C, shows when it
    // has the program's code, or is what the signal stopped.
    for (index, (thread, stack)) in threads.iter().enumerate() {
        let compiled = stack.iter().any(|&address| code.function(address.wrapping_add(1)).is_some());
        if !placed[index] && (index == 0 || compiled) {
            bound.extend(of_thread(thread.id as u64, thread));
            rows.push(Row {
                id: thread.id as u64,
                main: false,
                state: "runs",
                stack: stack.clone(),
                stopped: index == 0,
                wait: None,
                cancelled: false,
            });
        }
    }
    rows.sort_by_key(|row| !row.stopped);

    let mut frames = Map::new();
    let image = program.display().to_string();
    let own = mapped.map_or("", |mapped| mapped.path.as_str());
    let mut tasks = Vec::new();
    for Row { id, main, state, stack: addresses, stopped, wait, cancelled } in rows {
        for (at, &address) in addresses.iter().enumerate() {
            let frame = code.frame(address, stopped && at == 0).unwrap_or_else(|| {
                match found.files.iter().find(|file| file.start <= address && address < file.end) {
                    Some(file) if file.path != own => {
                        json!({ "image": file.path, "offset": address - file.start + file.offset })
                    }
                    _ => json!({ "image": image, "offset": address.wrapping_sub(bias) }),
                }
            });
            frames.insert(format!("{address:x}"), frame);
        }
        let stack: Vec<String> = addresses.iter().map(|address| format!("{address:x}")).collect();
        let mut task = json!({
            "id": id, "main": main, "state": state,
            "cancelled": cancelled, "stack": stack, "spawned": [], "locks": [], "contexts": [],
        });
        if let Some(wait) = wait {
            task["wait"] = wait;
        }
        tasks.push(task);
    }
    let signal = signal_name(found.signal);
    let place = if found.code > 0 { format!(" at address {:#x}", found.address) } else { String::new() };
    let time = std::fs::metadata(path).and_then(|data| data.modified()).ok();
    let time = time.and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |time| time.as_millis());
    let arguments: Vec<&str> = found.command.split_whitespace().skip(1).collect();
    // A request with no answer after it was in flight.
    let mut events = events.map_or(Vec::new(), |at| events_at(&memory, at));
    let now = time as u64;
    let mut flying: Vec<Value> = Vec::new();
    for event in &mut events {
        let at = event["time_ms"].as_u64().unwrap_or(now);
        event["ago_ms"] = json!(now.saturating_sub(at));
        match event["what"].as_str() {
            Some("request") => {
                let (method, path) = event["text"].as_str().and_then(|t| t.split_once(' ')).unwrap_or(("", ""));
                let ms = now.saturating_sub(at);
                flying.push(json!({ "method": method, "path": path, "task": event["task"], "ms": ms }));
            }
            Some("answered") => {
                let text = event["text"].as_str().unwrap_or_default();
                let same = |request: &Value| {
                    format!("{} {}", request["method"].as_str().unwrap_or(""), request["path"].as_str().unwrap_or(""))
                        == text
                };
                if let Some(at) = flying.iter().position(same) {
                    flying.remove(at);
                }
            }
            _ => {}
        }
    }
    Ok(json!({
        "dump": 1,
        "reason": "fault",
        "message": format!("{signal}{place}, from a system core, which says where tasks were and the numbers and strings they held"),
        "info": { "pid": found.threads[0].id, "program": image, "arguments": arguments, "uptime_ms": 0 },
        "time_ms": time,
        "stopped": true,
        "counters": [],
        "tasks": tasks,
        "heap": { "rate": 0, "dropped": 0, "samples": [] },
        "requests": flying,
        "events": events,
        "sources": Roots::text_in(&bytes).unwrap_or_default(),
        "lock_order": [],
        "values": bound,
        "frames": frames,
    }))
}

/// `tondo dump core`: writes a core as a dump, which every other command
/// then reads.
pub fn convert(core: &Path, executable: Option<&Path>, output: Option<&Path>) -> Result<PathBuf, String> {
    let name = core.to_string_lossy();
    let pid = name.parse::<u32>().ok().filter(|_| !core.exists());
    // A core that comes from elsewhere stays only until it is read: the
    // dump is a thousandth of its size.
    let taken = match (pid, name.as_ref()) {
        (Some(pid), _) => Some(from_coredumpctl(pid, output)?),
        (None, "-") => Some(from_pipe(output)?),
        _ => None,
    };
    let file = taken.as_deref().unwrap_or(core);
    let dump = read(file, executable);
    if let Some(taken) = &taken {
        let _ = std::fs::remove_file(taken);
    }
    let dump = dump?;
    let output = output.map_or_else(|| PathBuf::from(format!("{}.dump.json", name)), Path::to_path_buf);
    let text = serde_json::to_string(&dump).map_err(|e| e.to_string())?;
    std::fs::write(&output, text).map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    Ok(output)
}

/// Where a core goes for a moment: beside the dump, as there may be no room
/// for it in a temporary directory.
fn beside(output: Option<&Path>) -> PathBuf {
    let name = format!("tondo-core-{}", std::process::id());
    match output.and_then(Path::parent).filter(|dir| !dir.as_os_str().is_empty()) {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// The core of process `pid`, out of what systemd keeps.
fn from_coredumpctl(pid: u32, output: Option<&Path>) -> Result<PathBuf, String> {
    let file = beside(output);
    let ran = std::process::Command::new("coredumpctl")
        .args(["dump", &pid.to_string(), "-o"])
        .arg(&file)
        .output()
        .map_err(|e| format!("cannot run coredumpctl: {e}: give the core as a file"))?;
    if !ran.status.success() {
        let _ = std::fs::remove_file(&file);
        return Err(format!("coredumpctl has no core of {pid}: {}", String::from_utf8_lossy(&ran.stderr).trim()));
    }
    Ok(file)
}

/// The core on standard input, as the kernel gives it to `core_pattern`.
fn from_pipe(output: Option<&Path>) -> Result<PathBuf, String> {
    let file = beside(output);
    let mut written = std::fs::File::create(&file).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    std::io::copy(&mut std::io::stdin(), &mut written).map_err(|e| format!("cannot read the core: {e}"))?;
    Ok(file)
}
