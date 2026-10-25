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
    let moves_at = entries_at + entries * 16;
    if bytes.len() < moves_at || table + 40 > size {
        return Err("the patch is damaged".into());
    }
    let (mut moves, redirects) = moves(&bytes[moves_at..])?;
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
    if table as usize + (6 + slots * 2 + identities * 2) * 8 > base as usize + size {
        return Err("the patch is damaged".into());
    }
    // Then its conversions of the old values frames bring.
    let at = 5 + slots * 2 + identities * 2;
    let count = read(at) as usize;
    if table as usize + (6 + slots * 2 + identities * 2 + count * 3) * 8 > base as usize + size {
        return Err("the patch is damaged".into());
    }
    let conversions: Vec<Conversion> = (0..count)
        .map(|index| {
            let word = |offset: usize| read(at + 1 + index * 3 + offset) as usize;
            unsafe {
                Conversion {
                    convert: std::mem::transmute::<usize, Convert>(word(0)),
                    from: (word(1) != 0).then(|| std::mem::transmute::<usize, Release>(word(1))),
                    to: (word(2) != 0).then(|| std::mem::transmute::<usize, Release>(word(2))),
                }
            }
        })
        .collect();
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
    // Values of a closure that is only numbered otherwise run its new code.
    for &(from, to) in &redirects {
        if from as u64 >= info.capacity || to as u64 >= info.capacity {
            return Err("the patch is damaged".into());
        }
        let slot = |index: u32| unsafe { (info.slots.add(index as usize) as *const AtomicU64).as_ref() };
        let code = slot(to).expect("the image's slots").load(Ordering::Acquire);
        slot(from).expect("the image's slots").store(code, Ordering::Release);
    }
    // Frames of the old code move once the slots lead to the new: never
    // before, when the state would reach the old code. Each enters this
    // patch's code, which its state is for, even once another came.
    for (_, found) in &mut moves {
        if found.slot >= info.capacity || found.conversions.iter().any(|c| c.index as usize >= conversions.len()) {
            return Err("the patch is damaged".into());
        }
        let params = found.params.iter_mut().flat_map(|p| p.converted.iter_mut().chain(p.back.iter_mut()));
        let parts = found.conversions.iter_mut().chain(params).chain(found.result.iter_mut().map(|(r, _)| r));
        for converted in parts {
            let found = conversions.get(converted.index as usize).ok_or("the patch is damaged")?;
            converted.conversion = Some(*found);
        }
        found.code = unsafe { (info.slots.add(found.slot as usize) as *const AtomicU64).as_ref() }
            .expect("the image's slots")
            .load(Ordering::Acquire);
    }
    let moving = moves.len();
    TOLD.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *MOVES.write().unwrap_or_else(|e| e.into_inner()) = moves;
    PENDING.store(moving > 0, Ordering::Release);
    let patches = PATCHES.fetch_add(1, Ordering::AcqRel) + 1;
    Ok(format!("{{\"patched\":true,\"functions\":{slots},\"patches\":{patches},\"moves\":{moving}}}"))
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

/// How a frame at a site of the old code moves to the new: the slot of its
/// function and the code it enters, the point of that code, the size of
/// its state, and where each part of the old state goes in it, as it is or
/// converted to a value of a type of another shape.
struct Move {
    slot: u64,
    code: u64,
    point: u32,
    size: u32,
    copies: Vec<(u32, u32, u32)>,
    conversions: Vec<Converted>,
    /// For a function of another identity: each parameter, by its place, as
    /// it is or converted, which the new code takes from the runtime; and
    /// for a result of another shape, the conversion back to the caller's,
    /// with the new result's size.
    params: Vec<Param>,
    result: Option<(Converted, u32)>,
}

/// A parameter a frame passes to the new code: its place, and how it is
/// converted, with whether it is passed in memory and the size there.
struct Param {
    index: u32,
    converted: Option<Converted>,
    memory: bool,
    size: u32,
    /// For a value lent to the call: the conversion that puts it back, and
    /// its size in the caller's shape.
    back: Option<Converted>,
    old_size: u32,
}

/// A part of the old state that a conversion of the patch makes a value
/// of the new code: where it is and goes, which conversion, and its type.
struct Converted {
    from: u32,
    to: u32,
    index: u32,
    name: String,
    conversion: Option<Conversion>,
}

/// A conversion: what writes the value of the new type, given the old,
/// and whether it could; and what releases a value of either type.
#[derive(Clone, Copy)]
struct Conversion {
    convert: Convert,
    from: Option<Release>,
    to: Option<Release>,
}

type Convert = unsafe extern "C" fn(*const u8, *mut u8) -> u8;
type Release = unsafe extern "C" fn(*mut u8);

/// A patch's moves, by the site of the old code, and the slots that lead to
/// another's code.
type Moves = (Vec<(i64, Move)>, Vec<(u32, u32)>);

/// The moves of the last patch, by the site of the old code.
static MOVES: std::sync::RwLock<Vec<(i64, Move)>> = std::sync::RwLock::new(Vec::new());
static PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The moves a patch ends with, after `OSR5`: their count, then each
/// one's site, slot, point, size, count of copies and the copies, count of
/// conversions and each one's place, conversion and type's name, and count
/// of parameters and each one's place, whether it converts, whether it is
/// in memory, its conversion, size and type's name, and whether it goes
/// back, by which conversion, and its old size; whether the result converts
/// back, by which conversion, its new size and type's name; then the count
/// of the slots that lead to another's code and each pair.
fn moves(bytes: &[u8]) -> Result<Moves, String> {
    if bytes.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let damaged = || "the patch's moves are damaged".to_string();
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let slice = bytes.get(at..at + n).ok_or_else(damaged)?;
        at += n;
        Ok(slice)
    };
    let u32_of = |b: &[u8]| u32::from_le_bytes(b.try_into().expect("4 bytes"));
    if take(4)? != b"OSR5" {
        return Err(damaged());
    }
    let count = u32_of(take(4)?);
    let mut moves = Vec::new();
    for _ in 0..count {
        let site = i64::from_le_bytes(take(8)?.try_into().expect("8 bytes"));
        let slot = u32_of(take(4)?) as u64;
        let (point, size, copies) = (u32_of(take(4)?), u32_of(take(4)?), u32_of(take(4)?));
        let mut list = Vec::new();
        for _ in 0..copies {
            list.push((u32_of(take(4)?), u32_of(take(4)?), u32_of(take(4)?)));
        }
        let mut conversions = Vec::new();
        for _ in 0..u32_of(take(4)?) {
            let (from, to, index, length) = (u32_of(take(4)?), u32_of(take(4)?), u32_of(take(4)?), u32_of(take(4)?));
            let name = String::from_utf8_lossy(take(length as usize)?).into_owned();
            conversions.push(Converted { from, to, index, name, conversion: None });
        }
        let mut params = Vec::new();
        for _ in 0..u32_of(take(4)?) {
            let (index, converts, memory) = (u32_of(take(4)?), u32_of(take(4)?), u32_of(take(4)?));
            let (conversion, size, length) = (u32_of(take(4)?), u32_of(take(4)?), u32_of(take(4)?));
            let name = String::from_utf8_lossy(take(length as usize)?).into_owned();
            let converted = (converts != 0).then(|| Converted {
                from: 0,
                to: 0,
                index: conversion,
                name: name.clone(),
                conversion: None,
            });
            let (backs, back, old_size) = (u32_of(take(4)?), u32_of(take(4)?), u32_of(take(4)?));
            let back = (backs != 0).then_some(Converted { from: 0, to: 0, index: back, name, conversion: None });
            params.push(Param { index, converted, memory: memory != 0, size, back, old_size });
        }
        let (returns, back, result_size, length) =
            (u32_of(take(4)?), u32_of(take(4)?), u32_of(take(4)?), u32_of(take(4)?));
        let name = String::from_utf8_lossy(take(length as usize)?).into_owned();
        let result =
            (returns != 0).then_some((Converted { from: 0, to: 0, index: back, name, conversion: None }, result_size));
        moves.push((site, Move { slot, code: 0, point, size, copies: list, conversions, params, result }));
    }
    moves.sort_by_key(|(site, _)| *site);
    let mut redirects = Vec::new();
    for _ in 0..u32_of(take(4)?) {
        redirects.push((u32_of(take(4)?), u32_of(take(4)?)));
    }
    Ok((moves, redirects))
}

/// What a probe answers for its site: whether its frame moves to new code.
pub(crate) fn moving(site: i64) -> u8 {
    if !PENDING.load(Ordering::Acquire) {
        return 0;
    }
    let moves = MOVES.read().unwrap_or_else(|e| e.into_inner());
    if moves.binary_search_by_key(&site, |(s, _)| *s).is_ok() { 2 } else { 0 }
}

/// The state of a frame at a site of the old code as the new code's point
/// expects it, in memory the new code releases; null when it does not move.
/// `params` holds the frame's parameters, a word each, as the call passes
/// them; for a function of another identity the state says where the call
/// takes them from, converted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_live_transfer(site: i64, old: *const u8, params: *const u64) -> u64 {
    let moves = MOVES.read().unwrap_or_else(|e| e.into_inner());
    let Ok(index) = moves.binary_search_by_key(&site, |(s, _)| *s) else { return 0 };
    let found = &moves[index].1;
    let state = unsafe { libc::calloc(1, 24 + found.size as usize) } as *mut u8;
    if state.is_null() {
        return 0;
    }
    unsafe {
        (state as *mut u64).write(found.point as u64);
        (state as *mut u64).add(1).write(found.code);
        // Every value converts, or the frame stays as it was.
        for (index, converted) in found.conversions.iter().enumerate() {
            let conversion = converted.conversion.expect("a conversion of the patch");
            if (conversion.convert)(old.add(converted.from as usize), state.add(24 + converted.to as usize)) == 0 {
                for done in &found.conversions[..index] {
                    if let Some(release) = done.conversion.and_then(|c| c.to) {
                        release(state.add(24 + done.to as usize));
                    }
                }
                libc::free(state as *mut libc::c_void);
                stays(site, &converted.name);
                return 0;
            }
        }
        if !found.params.is_empty() || found.result.is_some() {
            match arguments(found, params) {
                Ok(block) => (state as *mut u64).add(2).write(block as u64),
                Err(name) => {
                    for done in &found.conversions {
                        if let Some(release) = done.conversion.and_then(|c| c.to) {
                            release(state.add(24 + done.to as usize));
                        }
                    }
                    libc::free(state as *mut libc::c_void);
                    stays(site, &name);
                    return 0;
                }
            }
        }
        // The old values are the new ones now.
        for converted in &found.conversions {
            if let Some(release) = converted.conversion.and_then(|c| c.from) {
                release(old.add(converted.from as usize) as *mut u8);
            }
        }
        for &(from, to, size) in &found.copies {
            std::ptr::copy_nonoverlapping(old.add(from as usize), state.add(24 + to as usize), size as usize);
        }
    }
    state as u64
}

/// What the runtime made for the calls that frames make as they move, by
/// the words it gives the calls.
static ARGUMENTS: std::sync::Mutex<Vec<(usize, Vec<Made>)>> = std::sync::Mutex::new(Vec::new());

/// What a call of the new code takes that the runtime made: a parameter
/// converted, which goes once the call returns; a value lent, converted,
/// which then goes back to the caller's place in the caller's shape; or a
/// place for a result of another shape, which goes to the caller's place
/// converted back.
enum Made {
    Value { at: usize, release: Option<Release>, memory: bool },
    Lent { at: usize, to: usize, size: u32, there: Conversion, back: Conversion },
    Result { at: usize, to: usize, back: Conversion },
}

/// The words a moving frame's call takes where its result goes and its
/// parameters from: each as it is, or converted; or the name of the type
/// of one that does not convert.
unsafe fn arguments(found: &Move, params: *const u64) -> Result<*mut u64, String> {
    let count = found.params.iter().map(|p| p.index as usize + 2).max().unwrap_or(1);
    let block = unsafe { libc::calloc(count, 8) } as *mut u64;
    let mut made: Vec<Made> = Vec::new();
    // The result, in the new shape, then converted to the caller's place.
    unsafe { block.write(params.read()) };
    if let Some((converted, size)) = &found.result {
        let at = unsafe { libc::calloc(1, (*size).max(1) as usize) };
        let back = converted.conversion.expect("a conversion of the patch");
        made.push(Made::Result { at: at as usize, to: unsafe { params.read() } as usize, back });
        unsafe { block.write(at as u64) };
    }
    for param in &found.params {
        let word = unsafe { params.add(param.index as usize + 1) };
        let at = unsafe { block.add(param.index as usize + 1) };
        let Some(converted) = &param.converted else {
            unsafe { at.write(word.read()) };
            continue;
        };
        let conversion = converted.conversion.expect("a conversion of the patch");
        // A value in memory is passed by its address; one in a register, as
        // it is.
        let (from, to) = match param.memory {
            true => {
                (unsafe { word.read() } as *const u8, unsafe { libc::calloc(1, param.size.max(1) as usize) } as *mut u8)
            }
            false => (word as *const u8, at as *mut u8),
        };
        if unsafe { (conversion.convert)(from, to) } == 0 {
            if param.memory {
                unsafe { libc::free(to as *mut libc::c_void) };
            }
            unsafe { release(made) };
            unsafe { libc::free(block as *mut libc::c_void) };
            return Err(converted.name.clone());
        }
        if param.memory {
            unsafe { at.write(to as u64) };
        }
        made.push(match param.back.as_ref().and_then(|b| b.conversion) {
            Some(back) => {
                Made::Lent { at: to as usize, to: from as usize, size: param.old_size, there: conversion, back }
            }
            None => Made::Value { at: to as usize, release: conversion.to, memory: param.memory },
        });
    }
    ARGUMENTS.lock().unwrap_or_else(|e| e.into_inner()).push((block as usize, made));
    Ok(block)
}

/// Once a call returns: what the runtime made for it goes, and what goes
/// back to the caller, a value lent or the result, goes there converted.
/// With `returned` false, as when making the call failed, nothing goes back.
unsafe fn release(made: Vec<Made>) {
    unsafe { finish(made, false) }
}

unsafe fn finish(made: Vec<Made>, returned: bool) {
    let free = |at: usize| unsafe { libc::free(at as *mut libc::c_void) };
    for made in made {
        match made {
            Made::Value { at, release, memory } => {
                if let Some(release) = release {
                    unsafe { release(at as *mut u8) };
                }
                if memory {
                    free(at);
                }
            }
            Made::Lent { at, to, size, there, back } => {
                if returned {
                    // The caller's value is the one the call left, in the
                    // caller's shape, which loses nothing.
                    let again = unsafe { libc::calloc(1, size.max(1) as usize) } as *mut u8;
                    if unsafe { (back.convert)(at as *const u8, again) } != 0 {
                        if let Some(release) = there.from {
                            unsafe { release(to as *mut u8) };
                        }
                        unsafe { std::ptr::copy_nonoverlapping(again, to as *mut u8, size as usize) };
                    }
                    free(again as usize);
                }
                if let Some(release) = there.to {
                    unsafe { release(at as *mut u8) };
                }
                free(at);
            }
            Made::Result { at, to, back } => {
                if returned && unsafe { (back.convert)(at as *const u8, to as *mut u8) } == 0 {
                    super::runtime_fault("a result did not convert back to the caller's shape");
                }
                if returned && let Some(release) = back.from {
                    unsafe { release(at as *mut u8) };
                }
                free(at);
            }
        }
    }
}

/// Once the call of a frame that moved returns: what the runtime converted
/// for it goes, and what goes back to the caller goes there.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_live_args_done(block: u64) {
    if block == 0 {
        return;
    }
    let made = {
        let mut list = ARGUMENTS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(index) = list.iter().position(|(b, _)| *b == block as usize) else { return };
        list.swap_remove(index).1
    };
    unsafe {
        finish(made, true);
        libc::free(block as *mut libc::c_void);
    }
}

/// The types whose values did not convert since the last patch.
static TOLD: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Says, once for each type and patch, that a frame stays on the old code
/// because one of its values does not convert.
fn stays(site: i64, name: &str) {
    let mut told = TOLD.lock().unwrap_or_else(|e| e.into_inner());
    if told.iter().any(|t| t == name) {
        return;
    }
    told.push(name.to_string());
    let place = super::debugger::place(site as usize);
    let _ = std::io::Write::write_fmt(
        &mut std::io::stderr(),
        format_args!(
            "tondo runtime: a frame at {place} stays on the old code: one of its `{name}` values does not convert: it is of a variant the new code does not have, or a number that does not fit\n"
        ),
    );
}

/// Releases a state once the new code took it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_live_free(state: u64) {
    unsafe { libc::free(state as *mut libc::c_void) };
}
