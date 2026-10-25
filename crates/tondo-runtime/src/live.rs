//! Live builds (toolchain 7.10): the image registers its tables before
//! anything runs. Every call of one of the program's functions loads its
//! address from a slot; a patch maps new code and points the slots at it,
//! and reaches what it needs of the image and of the runtime through the
//! other tables. A live program listens for the tools whatever its
//! environment says.

use super::{Str, str_bytes};
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

/// The image's tables, as the compiler lays them out (`codegen::live_tables`).
#[repr(C)]
pub struct Info {
    slots: *mut usize,
    capacity: u64,
    used: u64,
    runtime: *const usize,
    runtime_count: u64,
    c: *const usize,
    c_count: u64,
    data: *mut usize,
    data_count: u64,
    data_capacity: u64,
    build: *const Str,
    sites: u64,
}

static INFO: AtomicPtr<Info> = AtomicPtr::new(std::ptr::null_mut());
/// The slots in use, the image's and every patch's, and the patches.
static USED: AtomicU64 = AtomicU64::new(0);
static PATCHES: AtomicU64 = AtomicU64::new(0);

/// Called by a live build's process before anything else.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_live(info: *const Info) {
    let tables = unsafe { &*info };
    USED.store(tables.used, Ordering::Relaxed);
    DATA.store(tables.data_count, Ordering::Relaxed);
    INFO.store(info as *mut Info, Ordering::Release);
}

/// Whether the program is a live build.
pub(crate) fn on() -> bool {
    !INFO.load(Ordering::Acquire).is_null()
}

/// What `tondo reload` asks first: the build, and what its patches took.
pub(crate) fn describe() -> String {
    let info = INFO.load(Ordering::Acquire);
    if info.is_null() {
        return "{\"error\":\"the program is not a live build: build it with --live\"}".into();
    }
    let info = unsafe { &*info };
    let build = String::from_utf8_lossy(unsafe { str_bytes(info.build) }).into_owned();
    format!(
        "{{\"live\":true,\"build\":\"{build}\",\"capacity\":{},\"used\":{},\"sites\":{},\"patches\":{}}}",
        info.capacity,
        USED.load(Ordering::Relaxed),
        super::debugger::sites(),
        PATCHES.load(Ordering::Relaxed)
    )
}

/// Applies the patch in the file at `path` (`link::patch` writes it): maps
/// it, relocates it, fills what it takes of the image and the runtime,
/// makes it code, registers its tables and points the slots at it. After a
/// tab, the functions no task may be in, whose frames may hold values of a
/// type the patch changes: the world stops while the stacks are looked at
/// and the patch goes in.
pub(crate) fn patch(request: &str) -> String {
    let mut parts = request.split('\t');
    let path = parts.next().unwrap_or_default();
    let guarded: Vec<&str> = parts.collect();
    let error = |message: &str| format!("{{\"error\":{}}}", super::diagnostics::quoted(message));
    let answer = |result: Result<String, String>| result.unwrap_or_else(|e| error(&e));
    if guarded.is_empty() {
        return answer(apply(path));
    }
    if !super::tasks::stop_world(std::time::Duration::from_secs(2)) {
        return error("the program did not stop for the patch: reload again");
    }
    let mut alive = Vec::new();
    for shot in super::tasks::snapshot(None) {
        for &address in &shot.stack {
            let Some(frame) = super::trace::describe(address) else { continue };
            if guarded.contains(&frame.function) {
                let at = frame.at.map_or(String::new(), |(file, line)| format!("{file}:{line}"));
                alive.push(format!(
                    "{{\"task\":{},\"function\":{},\"at\":{}}}",
                    shot.id,
                    super::diagnostics::quoted(frame.function),
                    super::diagnostics::quoted(&at)
                ));
            }
        }
    }
    let result = match alive.is_empty() {
        true => answer(apply(path)),
        false => format!("{{\"error\":\"alive\",\"alive\":[{}]}}", alive.join(",")),
    };
    super::tasks::resume_world();
    result
}

fn apply(path: &str) -> Result<String, String> {
    let info = unsafe { INFO.load(Ordering::Acquire).as_ref() }.ok_or("the program is not a live build")?;
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read the patch {path}: {e}"))?;
    let word = |at: usize| -> Result<u64, String> {
        let slice = bytes.get(at..at + 8).ok_or("the patch is cut short")?;
        Ok(u64::from_le_bytes(slice.try_into().expect("8 bytes")))
    };
    if bytes.get(..8) != Some(b"TONDOPT1") {
        return Err("the file is not a patch of this tondo".into());
    }
    let (size, table, words, entries) = (word(8)? as usize, word(16)? as usize, word(24)? as usize, word(32)? as usize);
    let image_at = 40;
    let words_at = image_at + size;
    let entries_at = words_at + words * 8;
    if bytes.len() != entries_at + entries * 16 || table + 40 > size {
        return Err("the patch is damaged".into());
    }
    let length = size.next_multiple_of(65536);
    let base = map(length)?;
    let image = unsafe { std::slice::from_raw_parts_mut(base, length) };
    writable(true);
    image[..size].copy_from_slice(&bytes[image_at..words_at]);
    let at = |offset: usize| -> Result<*mut u64, String> {
        match offset + 8 <= size {
            true => Ok(unsafe { base.add(offset) } as *mut u64),
            false => Err("the patch is damaged".into()),
        }
    };
    for index in 0..words {
        let place = at(word(words_at + index * 8)? as usize)?;
        unsafe { place.write_unaligned(place.read_unaligned().wrapping_add(base as u64)) };
    }
    let lookup = |table: *const usize, count: u64, index: u32| -> Result<u64, String> {
        match (index as u64) < count {
            true => Ok(unsafe { *table.add(index as usize) } as u64),
            false => Err("the patch needs what the program does not have: start it again".into()),
        }
    };
    for index in 0..entries {
        let entry = entries_at + index * 16;
        let place = at(word(entry)? as usize)?;
        let (table, index) = (word(entry + 8)? as u32, (word(entry + 8)? >> 32) as u32);
        let value = match table {
            0 => info.slots as u64,
            1 => lookup(info.runtime, info.runtime_count, index)?,
            2 => lookup(info.c, info.c_count, index)?,
            3 => lookup(info.data, DATA.load(Ordering::Acquire).max(info.data_count), index)?,
            _ => return Err("the patch is damaged".into()),
        };
        unsafe { place.write_unaligned(value) };
    }
    writable(false);
    executable(base, length)?;
    // The patch's table: its tables of traces and of probes, where its
    // sites start, then the slots it sets and the identities it adds.
    let table = unsafe { base.add(table) } as *const u64;
    let read = |index: usize| unsafe { *table.add(index) };
    let (trace, debug, sites, slots, identities) = (read(0), read(1), read(2), read(3) as usize, read(4) as usize);
    if table as usize + (5 + slots * 2 + identities * 2) * 8 > base as usize + size {
        return Err("the patch is damaged".into());
    }
    let current = super::debugger::sites() as u64;
    if debug != 0 && sites != current {
        return Err("the program changed since the patch was made: reload again".into());
    }
    for index in 0..identities {
        let (slot, address) = (read(5 + slots * 2 + index * 2), read(6 + slots * 2 + index * 2));
        if slot >= info.data_capacity {
            return Err("the program has no room for more types: start it again".into());
        }
        unsafe { (info.data.add(slot as usize) as *const AtomicU64).as_ref() }
            .expect("the image's table")
            .store(address, Ordering::Release);
        DATA.fetch_max(slot + 1, Ordering::AcqRel);
    }
    if !super::trace::add(trace as *const _) {
        return Err("the program has no room for more patches: start it again".into());
    }
    if debug != 0 {
        super::debugger::add_table(debug as *const _);
    }
    for index in 0..slots {
        let (slot, address) = (read(5 + index * 2), read(6 + index * 2));
        if slot >= info.capacity {
            return Err("the program has no room for more functions: start it again".into());
        }
        unsafe { (info.slots.add(slot as usize) as *const AtomicU64).as_ref() }
            .expect("the image's slots")
            .store(address, Ordering::Release);
        USED.fetch_max(slot + 1, Ordering::AcqRel);
    }
    let patches = PATCHES.fetch_add(1, Ordering::AcqRel) + 1;
    Ok(format!("{{\"patched\":true,\"functions\":{slots},\"patches\":{patches}}}"))
}

/// The identities in use, the image's and the patches'.
static DATA: AtomicU64 = AtomicU64::new(0);

/// Memory for a patch, which becomes code once written.
fn map(length: usize) -> Result<*mut u8, String> {
    #[cfg(target_os = "macos")]
    let (protection, flags) = (libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC, libc::MAP_JIT);
    #[cfg(not(target_os = "macos"))]
    let (protection, flags) = (libc::PROT_READ | libc::PROT_WRITE, 0);
    let address = unsafe {
        libc::mmap(std::ptr::null_mut(), length, protection, libc::MAP_PRIVATE | libc::MAP_ANON | flags, -1, 0)
    };
    match address == libc::MAP_FAILED {
        true => Err(format!("cannot map memory for the patch: {}", std::io::Error::last_os_error())),
        false => Ok(address as *mut u8),
    }
}

/// On Apple silicon, code memory is writable by one thread at a time.
fn writable(on: bool) {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    unsafe {
        unsafe extern "C" {
            fn pthread_jit_write_protect_np(enabled: i32);
        }
        pthread_jit_write_protect_np(i32::from(!on));
    }
    let _ = on;
}

/// Makes the patch code: read and run only, and what the processor
/// fetches of it the new bytes.
fn executable(base: *mut u8, length: usize) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    if unsafe { libc::mprotect(base as *mut libc::c_void, length, libc::PROT_READ | libc::PROT_EXEC) } != 0 {
        return Err(format!("cannot make the patch code: {}", std::io::Error::last_os_error()));
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    unsafe {
        unsafe extern "C" {
            fn sys_icache_invalidate(start: *mut libc::c_void, length: usize);
        }
        sys_icache_invalidate(base as *mut libc::c_void, length);
    }
    #[cfg(all(not(target_os = "macos"), target_arch = "aarch64"))]
    unsafe {
        let ctr: u64;
        std::arch::asm!("mrs {}, ctr_el0", out(reg) ctr);
        let (data, instruction) = (4usize << ((ctr >> 16) & 0xf), 4usize << (ctr & 0xf));
        let (start, end) = (base as usize, base as usize + length);
        let mut line = start & !(data - 1);
        while line < end {
            std::arch::asm!("dc cvau, {}", in(reg) line);
            line += data;
        }
        std::arch::asm!("dsb ish");
        let mut line = start & !(instruction - 1);
        while line < end {
            std::arch::asm!("ic ivau, {}", in(reg) line);
            line += instruction;
        }
        std::arch::asm!("dsb ish", "isb");
    }
    let _ = (base, length);
    Ok(())
}
