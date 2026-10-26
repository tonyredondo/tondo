//! The functions of the program's own executable, from its table of
//! symbols, read once when a profile the program makes of itself names
//! them (standard library 14.26): the runtime's and the C library's linked
//! in, which the loader's `dladdr` does not know, as only what a file
//! exports is in its dynamic table.

use std::sync::OnceLock;

/// Each function: where its code starts and ends in memory, and its name.
type Function = (usize, usize, String);

fn functions() -> &'static [Function] {
    static FUNCTIONS: OnceLock<Vec<Function>> = OnceLock::new();
    FUNCTIONS.get_or_init(|| {
        let file = std::env::current_exe().ok().and_then(|path| std::fs::read(path).ok()).unwrap_or_default();
        let mut found = read(&file).unwrap_or_default();
        found.sort();
        found
    })
}

/// The function whose code holds `address`.
pub(crate) fn name(address: usize) -> Option<&'static str> {
    let functions = functions();
    let at = functions.partition_point(|(start, ..)| *start <= address).checked_sub(1)?;
    let (start, end, name) = &functions[at];
    (*start <= address && address < *end).then_some(name.as_str())
}

#[cfg(target_os = "linux")]
fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?) as usize)
}

fn text(bytes: &[u8], at: usize) -> Option<String> {
    let rest = bytes.get(at..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

#[cfg(target_os = "linux")]
fn read(file: &[u8]) -> Option<Vec<Function>> {
    if file.get(..4)? != b"\x7fELF" {
        return None;
    }
    // Where the file's header is in memory, against where the file says it
    // is: the first loaded segment, which starts at the header.
    unsafe extern "C" {
        static __ehdr_start: u8;
    }
    let (phoff, phsize, phcount) = (u64_at(file, 0x20)?, u16_at(file, 0x36)? as usize, u16_at(file, 0x38)? as usize);
    let header = (0..phcount)
        .map(|i| phoff + i * phsize)
        .find(|&p| u32_at(file, p) == Some(1) && u64_at(file, p + 8) == Some(0));
    let linked = header.and_then(|p| u64_at(file, p + 16)).unwrap_or(0);
    let bias = (&raw const __ehdr_start as usize).wrapping_sub(linked);
    let (shoff, shsize, shcount) = (u64_at(file, 0x28)?, u16_at(file, 0x3a)? as usize, u16_at(file, 0x3c)? as usize);
    let section = |index: usize| shoff + index * shsize;
    // The full table, or the dynamic one of a file stripped of it.
    let table = (0..shcount)
        .find(|&i| u32_at(file, section(i) + 4) == Some(2))
        .or_else(|| (0..shcount).find(|&i| u32_at(file, section(i) + 4) == Some(11)))?;
    let (offset, size) = (u64_at(file, section(table) + 0x18)?, u64_at(file, section(table) + 0x20)?);
    let names = u64_at(file, section(u32_at(file, section(table) + 0x28)? as usize) + 0x18)?;
    let mut out = Vec::new();
    for symbol in (offset..offset + size).step_by(24) {
        let (info, value, length) = (*file.get(symbol + 4)?, u64_at(file, symbol + 8)?, u64_at(file, symbol + 16)?);
        if info & 0xf == 2 && value != 0 {
            let name = text(file, names + u32_at(file, symbol)? as usize)?;
            out.push((value.wrapping_add(bias), value.wrapping_add(bias) + length.max(1), name));
        }
    }
    Some(out)
}

#[cfg(target_os = "macos")]
fn read(file: &[u8]) -> Option<Vec<Function>> {
    unsafe extern "C" {
        fn _dyld_get_image_vmaddr_slide(index: u32) -> isize;
    }
    if u32_at(file, 0)? != 0xfeed_facf {
        return None;
    }
    let slide = unsafe { _dyld_get_image_vmaddr_slide(0) } as usize;
    let mut command = 32;
    for _ in 0..u32_at(file, 16)? {
        let (kind, size) = (u32_at(file, command)?, u32_at(file, command + 4)? as usize);
        if kind == 2 {
            let (symbols, count) = (u32_at(file, command + 8)? as usize, u32_at(file, command + 12)? as usize);
            let names = u32_at(file, command + 16)? as usize;
            let mut starts = Vec::new();
            for symbol in (0..count).map(|i| symbols + i * 16) {
                let kind = *file.get(symbol + 4)?;
                // Defined in a section, and no debugging entry.
                if kind & 0xe0 == 0 && kind & 0x0e == 0x0e {
                    let name = text(file, names + u32_at(file, symbol)? as usize)?;
                    let name = name.strip_prefix('_').unwrap_or(&name).to_string();
                    starts.push((u64_at(file, symbol + 8)?.wrapping_add(slide), name));
                }
            }
            starts.sort();
            // Each runs to where the next begins.
            let ends: Vec<usize> = starts.iter().skip(1).map(|(start, _)| *start).chain([usize::MAX]).collect();
            return Some(starts.into_iter().zip(ends).map(|((start, name), end)| (start, end, name)).collect());
        }
        command += size;
    }
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read(_: &[u8]) -> Option<Vec<Function>> {
    None
}
