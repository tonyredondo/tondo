//! Walks callee-saved registers through the runtime's own frames for a
//! dump (toolchain 7.5), from the unwind tables of the executable, ELF's
//! `.eh_frame` or Mach-O's `__unwind_info`: a task that waits is deep in
//! the runtime, whose frames hold what the program's code had in its
//! registers. Only frames with a frame pointer: others are not on the walk.

#![cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]

/// The registers of a frame, by their DWARF numbers.
pub(crate) type Registers = [Option<usize>; 32];
/// What a task left as it switched out: its registers, and where it goes on.
pub(crate) type Switched = (Registers, usize);

#[cfg(target_arch = "x86_64")]
const FRAME_REGISTER: u64 = 6;
#[cfg(target_arch = "aarch64")]
const FRAME_REGISTER: u64 = 29;

/// The registers of the caller of the frame at `address`, whose frame
/// pointer is `fp`, given its own: those it saved come back from its
/// frame. Nothing is known without its tables.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn restored(address: usize, fp: usize, registers: &Registers, bounds: (usize, usize)) -> Registers {
    let (mut out, none) = (*registers, [None; 32]);
    let Some(rules) = rules_at(address - 1) else { return none };
    if rules.frame_register != FRAME_REGISTER {
        return none;
    }
    let frame = fp as i64 + rules.frame_offset;
    for (register, rule) in rules.saved.iter().enumerate() {
        let Some(offset) = rule else { continue };
        // The outermost frame's pointer is 0, and what it gives is no place.
        let at = (frame + offset) as usize;
        let inside = at >= bounds.0 && at.checked_add(8).is_some_and(|end| end <= bounds.1);
        out[register] = inside.then(|| unsafe { *(at as *const usize) });
    }
    out
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn restored(_: usize, _: usize, _: &Registers, _: (usize, usize)) -> Registers {
    [None; 32]
}

/// What the tables say of a place in the code: its canonical frame
/// address, as a register and an offset, and where from there each saved
/// register is.
#[derive(Clone, Copy)]
struct Rules {
    frame_register: u64,
    frame_offset: i64,
    saved: [Option<i64>; 32],
}

/// Reads the tables, which are the program's own and well formed.
struct Reader(usize);

impl Reader {
    fn read<T: Copy>(&mut self) -> T {
        let value = unsafe { (self.0 as *const T).read_unaligned() };
        self.0 += size_of::<T>();
        value
    }

    fn unsigned(&mut self) -> u64 {
        let (mut value, mut shift) = (0u64, 0);
        loop {
            let byte: u8 = self.read();
            value |= u64::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                return value;
            }
        }
    }

    fn signed(&mut self) -> i64 {
        let (mut value, mut shift) = (0i64, 0);
        loop {
            let byte: u8 = self.read();
            value |= i64::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                if shift < 64 && byte & 0x40 != 0 {
                    value |= -1 << shift;
                }
                return value;
            }
        }
    }

    /// A pointer as `encoding` writes it, or a size without the base.
    fn pointer(&mut self, encoding: u8) -> Option<usize> {
        let place = self.0;
        let value = match encoding & 0x0f {
            0x00 | 0x04 | 0x0c => self.read::<u64>() as usize,
            0x01 => self.unsigned() as usize,
            0x03 => self.read::<u32>() as usize,
            0x09 => self.signed() as usize,
            0x0b => self.read::<i32>() as isize as usize,
            _ => return None,
        };
        Some(if encoding & 0x70 == 0x10 { place.wrapping_add(value) } else { value })
    }
}

/// Where the index of the unwind tables is.
#[cfg(target_os = "linux")]
fn index() -> Option<usize> {
    let headers = unsafe { libc::getauxval(libc::AT_PHDR) } as usize;
    let count = unsafe { libc::getauxval(libc::AT_PHNUM) } as usize;
    let headers = unsafe { std::slice::from_raw_parts(headers as *const libc::Elf64_Phdr, count) };
    // The headers follow the file's, at its start, unless one says where.
    let own = headers
        .iter()
        .find(|h| h.p_type == libc::PT_PHDR)
        .map_or(size_of::<libc::Elf64_Ehdr>(), |h| h.p_vaddr as usize);
    let base = headers.as_ptr() as usize - own;
    headers.iter().find(|h| h.p_type == libc::PT_GNU_EH_FRAME).map(|h| base + h.p_vaddr as usize)
}

/// The description of the code at `pc`: where it starts and its record.
#[cfg(target_os = "linux")]
fn described(pc: usize) -> Option<usize> {
    let index = index()?;
    let mut reader = Reader(index);
    let [version, frame_encoding, count_encoding, table_encoding]: [u8; 4] = reader.read();
    // What linkers write: a sorted table of 32-bit offsets from the index.
    if version != 1 || table_encoding != 0x3b {
        return None;
    }
    reader.pointer(frame_encoding)?;
    let count = reader.pointer(count_encoding)?;
    let table = unsafe { std::slice::from_raw_parts(reader.0 as *const [i32; 2], count) };
    let target = (pc as i64 - index as i64) as i32;
    let found = table.partition_point(|entry| entry[0] <= target).checked_sub(1)?;
    Some((index as isize + table[found][1] as isize) as usize)
}

#[cfg(target_os = "linux")]
fn rules_at(pc: usize) -> Option<Rules> {
    recorded(described(pc)?, pc)
}

/// What the image's compact tables say of the code at `pc`: the usual
/// frames of arm64 and x86-64 they say themselves, and the others they
/// leave to a record of `__eh_frame`.
#[cfg(target_os = "macos")]
fn rules_at(pc: usize) -> Option<Rules> {
    unsafe extern "C" {
        // The start of the image the runtime is in, executable or library.
        static __dso_handle: u8;
        fn getsectiondata(header: *const u8, segment: *const i8, section: *const i8, size: *mut u64) -> *const u8;
    }
    let header = &raw const __dso_handle;
    let section = |name: &std::ffi::CStr| {
        let mut size = 0;
        let at = unsafe { getsectiondata(header, c"__TEXT".as_ptr(), name.as_ptr(), &mut size) };
        (!at.is_null()).then_some(at as usize)
    };
    let encoding = compact(section(c"__unwind_info")?, u32::try_from(pc.checked_sub(header as usize)?).ok()?)?;
    let mut rules = Rules { frame_register: FRAME_REGISTER, frame_offset: 16, saved: [None; 32] };
    rules.saved[FRAME_REGISTER as usize] = Some(-16);
    match (cfg!(target_arch = "aarch64"), encoding & 0x0f00_0000) {
        // Pairs from x19 and x20 on, as it says, below the frame's record.
        (true, 0x0400_0000) => {
            let mut at = -24;
            for pair in (0..5).filter(|pair| encoding & (1 << pair) != 0) {
                for register in [19 + 2 * pair, 20 + 2 * pair] {
                    rules.saved[register] = Some(at);
                    at -= 8;
                }
            }
        }
        // Up to five, from where it says on, each as three bits say.
        (false, 0x0100_0000) => {
            let mut at = -16 - 8 * i64::from((encoding >> 16) & 0xff);
            for slot in 0..5 {
                let register = [None, Some(3), Some(12), Some(13), Some(14), Some(15), Some(6)];
                if let Some(&Some(register)) = register.get((encoding >> (3 * slot) & 7) as usize) {
                    rules.saved[register] = Some(at);
                }
                at += 8;
            }
        }
        (true, 0x0300_0000) | (false, 0x0400_0000) => {
            return recorded(section(c"__eh_frame")? + (encoding & 0x00ff_ffff) as usize, pc);
        }
        _ => return None,
    }
    Some(rules)
}

/// The compact encoding of the function at `offset` in the image, from
/// its `__unwind_info`: an index of pages, each regular or compressed.
#[cfg(target_os = "macos")]
fn compact(info: usize, offset: u32) -> Option<u32> {
    let word = |at: usize| unsafe { (at as *const u32).read_unaligned() };
    let half = |at: usize| usize::from(unsafe { (at as *const u16).read_unaligned() });
    if word(info) != 1 {
        return None;
    }
    let (common, common_count) = (info + word(info + 4) as usize, word(info + 8));
    let (index, count) = (info + word(info + 20) as usize, word(info + 24) as usize);
    // The index's last entry only says where the code ends.
    let entry = (0..count.checked_sub(1)?).rev().map(|i| index + 12 * i).find(|&e| word(e) <= offset)?;
    let page = match word(entry + 4) as usize {
        0 => return None,
        page => info + page,
    };
    let (entries, entry_count) = (page + half(page + 4), half(page + 6));
    match word(page) {
        2 => (0..entry_count).rev().map(|i| entries + 8 * i).find(|&e| word(e) <= offset).map(|e| word(e + 4)),
        3 => {
            let start = word(entry);
            let found =
                (0..entry_count).rev().map(|i| word(entries + 4 * i)).find(|e| start + (e & 0x00ff_ffff) <= offset)?;
            let which = found >> 24;
            Some(match which < common_count {
                true => word(common + 4 * which as usize),
                false => word(page + half(page + 8) + 4 * (which - common_count) as usize),
            })
        }
        _ => None,
    }
}

/// The rules of a record of `__eh_frame` for the code at `pc`.
fn recorded(record: usize, pc: usize) -> Option<Rules> {
    let mut reader = Reader(record);
    let length = reader.read::<u32>() as usize;
    let at_pointer = reader.0;
    let common = at_pointer - reader.read::<u32>() as usize;
    // The record all of a kind share.
    let mut shared = Reader(common);
    let shared_end = common + 4 + shared.read::<u32>() as usize;
    let _id: u32 = shared.read();
    let _version: u8 = shared.read();
    let mut augmentation = Vec::new();
    loop {
        match shared.read::<u8>() {
            0 => break,
            byte => augmentation.push(byte),
        }
    }
    let code_factor = shared.unsigned() as usize;
    let data_factor = shared.signed();
    let _return_register = shared.unsigned();
    let mut encoding = 0;
    if augmentation.first() == Some(&b'z') {
        let size = shared.unsigned() as usize;
        let after = shared.0 + size;
        for letter in &augmentation[1..] {
            match letter {
                b'R' => encoding = shared.read::<u8>(),
                b'L' => _ = shared.read::<u8>(),
                b'P' => {
                    let personality = shared.read::<u8>();
                    shared.pointer(personality)?;
                }
                _ => {}
            }
        }
        shared.0 = after;
    }
    let start = reader.pointer(encoding)?;
    let range = reader.pointer(encoding & 0x0f)?;
    if pc < start || pc >= start + range {
        return None;
    }
    if augmentation.first() == Some(&b'z') {
        let size = reader.unsigned() as usize;
        reader.0 += size;
    }
    let mut rules = Rules { frame_register: u64::MAX, frame_offset: 0, saved: [None; 32] };
    let mut place = start;
    run(&mut Reader(shared.0), shared_end, code_factor, data_factor, usize::MAX, &mut place, &mut rules, None)?;
    let initial = rules;
    place = start;
    let end = at_pointer + length;
    run(&mut reader, end, code_factor, data_factor, pc, &mut place, &mut rules, Some(&initial))?;
    Some(rules)
}

/// Runs the instructions of a record up to `pc`.
#[allow(clippy::too_many_arguments)]
fn run(
    reader: &mut Reader,
    end: usize,
    code_factor: usize,
    data_factor: i64,
    pc: usize,
    place: &mut usize,
    rules: &mut Rules,
    initial: Option<&Rules>,
) -> Option<()> {
    let mut remembered = Vec::new();
    let set = |rules: &mut Rules, register: u64, value: Option<i64>| {
        if let Some(rule) = rules.saved.get_mut(register as usize) {
            *rule = value;
        }
    };
    while reader.0 < end {
        let instruction: u8 = reader.read();
        let (high, low) = (instruction >> 6, u64::from(instruction & 0x3f));
        let advance = match (high, instruction) {
            (1, _) => low as usize,
            (2, _) => {
                let offset = reader.unsigned() as i64 * data_factor;
                set(rules, low, Some(offset));
                0
            }
            (3, _) => {
                set(rules, low, initial.and_then(|rules| *rules.saved.get(low as usize)?));
                0
            }
            (_, 0x00) => 0,
            (_, 0x02) => usize::from(reader.read::<u8>()),
            (_, 0x03) => usize::from(reader.read::<u16>()),
            (_, 0x04) => reader.read::<u32>() as usize,
            (_, 0x05) => {
                let register = reader.unsigned();
                let offset = reader.unsigned() as i64 * data_factor;
                set(rules, register, Some(offset));
                0
            }
            (_, 0x06) => {
                let register = reader.unsigned();
                set(rules, register, initial.and_then(|rules| *rules.saved.get(register as usize)?));
                0
            }
            (_, 0x07 | 0x08) => {
                let register = reader.unsigned();
                set(rules, register, None);
                0
            }
            (_, 0x0a) => {
                remembered.push(*rules);
                0
            }
            (_, 0x0b) => {
                *rules = remembered.pop()?;
                0
            }
            (_, 0x0c) => {
                rules.frame_register = reader.unsigned();
                rules.frame_offset = reader.unsigned() as i64;
                0
            }
            (_, 0x0d) => {
                rules.frame_register = reader.unsigned();
                0
            }
            (_, 0x0e) => {
                rules.frame_offset = reader.unsigned() as i64;
                0
            }
            (_, 0x2e) => {
                reader.unsigned();
                0
            }
            // What the runtime's code does not need, as expressions.
            _ => return None,
        };
        if advance != 0 {
            *place += advance * code_factor;
            if *place > pc {
                return Some(());
            }
        }
    }
    Some(())
}
