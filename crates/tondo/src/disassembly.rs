//! The instructions of a stopped program, around an address: its code as
//! the program's memory has it, by function, with the line of each, for
//! `disassemble` in `tondo debug` and the editor's disassembly view; and
//! the instructions of a function a build made, for `tondo asm`.

use serde_json::{Value, json};
use yaxpeax_arch::{Decoder, LengthedInstruction, U8Reader};

/// How far past a function the next one may start, as it is aligned.
const ALIGNED: u64 = 64;

/// An instruction of the program.
pub(crate) struct Instruction {
    pub(crate) address: u64,
    pub(crate) bytes: String,
    pub(crate) text: String,
    /// The function, on the first instruction of one.
    pub(crate) symbol: Option<String>,
    pub(crate) line: Option<(String, u64)>,
}

/// One instruction of `code`, and how many bytes it takes: an instruction
/// this does not know, as its bytes.
#[cfg(target_arch = "x86_64")]
fn decoded(code: &[u8]) -> (String, usize) {
    let decoder = yaxpeax_x86::amd64::InstDecoder::default();
    match decoder.decode(&mut U8Reader::new(code)) {
        Ok(instruction) => (instruction.to_string(), instruction.len().to_const() as usize),
        Err(_) => (format!(".byte 0x{:02x}", code[0]), 1),
    }
}

#[cfg(target_arch = "aarch64")]
fn decoded(code: &[u8]) -> (String, usize) {
    let decoder = yaxpeax_arm::armv8::a64::InstDecoder::default();
    match decoder.decode(&mut U8Reader::new(code)) {
        Ok(instruction) => (instruction.to_string(), instruction.len().to_const() as usize),
        Err(_) => (format!(".word 0x{}", code.iter().take(4).rev().map(|b| format!("{b:02x}")).collect::<String>()), 4),
    }
}

/// The instructions of `code`, as their offset in it, length and text.
pub(crate) fn decode(code: &[u8]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < code.len() {
        let (text, length) = decoded(&code[at..]);
        let length = length.clamp(1, code.len() - at);
        out.push((at, length, text));
        at += length;
    }
    out
}

/// The instructions of the function at `address`, by what the program says
/// of it; none when no function of the program is there.
fn function_at(ask: &mut dyn FnMut(&str) -> Result<Value, String>, address: u64) -> Option<Vec<Instruction>> {
    let code = ask(&format!("code {address}")).ok().filter(|code| code.get("error").is_none())?;
    let (start, end) = (code["start"].as_u64()?, code["end"].as_u64()?);
    let peeked = ask(&format!("peek {start} {}", end - start)).ok()?;
    let hex = peeked["bytes"].as_str()?.as_bytes();
    let digit = |pair: &[u8]| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok();
    let bytes: Vec<u8> = hex.chunks(2).filter_map(digit).collect();
    let lines = code["lines"].as_array()?;
    let line_of = |at: u64| {
        let index = lines.partition_point(|line| line[0].as_u64().is_some_and(|start| start <= at));
        let line = &lines[index.checked_sub(1)?];
        Some((line[1].as_str()?.to_string(), line[2].as_u64()?))
    };
    let out = decode(&bytes)
        .into_iter()
        .map(|(at, length, text)| {
            let address = start + at as u64;
            Instruction {
                address,
                bytes: bytes[at..at + length].iter().map(|b| format!("{b:02x}")).collect(),
                text,
                symbol: (at == 0).then(|| code["function"].as_str().unwrap_or_default().to_string()),
                line: line_of(address),
            }
        })
        .collect();
    Some(out)
}

/// What follows `address`, or what comes before it, in the program's code:
/// the nearest function past the padding between functions.
fn next_to(ask: &mut dyn FnMut(&str) -> Result<Value, String>, address: u64, back: bool) -> Option<Vec<Instruction>> {
    (0..ALIGNED).find_map(|gap| function_at(ask, if back { address.checked_sub(1 + gap)? } else { address + gap }))
}

/// `count` instructions from `offset` instructions after the one at
/// `address`, which may be before it, as far as the program's code goes:
/// an editor asks for those around the one it shows.
pub(crate) fn around(
    ask: &mut dyn FnMut(&str) -> Result<Value, String>,
    address: u64,
    offset: i64,
    count: usize,
) -> Result<Vec<Instruction>, String> {
    let mut list = function_at(ask, address).ok_or("no function of the program is there")?;
    let mut at = list.partition_point(|i| i.address <= address).saturating_sub(1) as i64;
    while at + offset < 0 {
        let Some(mut before) = next_to(ask, list[0].address, true) else { break };
        at += before.len() as i64;
        before.append(&mut list);
        list = before;
    }
    while at + offset + count as i64 > list.len() as i64 {
        let end = list.last().map_or(address, |last| last.address + last.bytes.len() as u64 / 2);
        let Some(mut after) = next_to(ask, end, false) else { break };
        list.append(&mut after);
    }
    let first = (at + offset).max(0) as usize;
    let last = (at + offset + count as i64).clamp(0, list.len() as i64) as usize;
    Ok(list.drain(first.min(last)..last).collect())
}

/// Instructions in text, the one at `current` marked.
pub(crate) fn text(list: &[Value], current: u64) -> String {
    let mut out = String::new();
    let mut line = &Value::Null;
    for instruction in list {
        if let Some(symbol) = instruction["symbol"].as_str() {
            out.push_str(&format!("{symbol}:\n"));
        }
        if let Some(at) = instruction["at"].as_str().filter(|_| instruction["at"] != *line) {
            line = &instruction["at"];
            out.push_str(&format!("  {at}\n"));
        }
        let address = instruction["address"].as_u64().unwrap_or(0);
        let mark = if address == current { "=>" } else { "  " };
        out.push_str(&format!("{mark} {address:#x}  {}\n", instruction["instruction"].as_str().unwrap_or_default()));
    }
    out
}

/// An instruction as `tondo debug --json` gives it.
pub(crate) fn data(instruction: &Instruction) -> Value {
    let at = instruction.line.as_ref().map(|(file, line)| format!("{file}:{line}"));
    json!({
        "address": instruction.address, "bytes": instruction.bytes, "instruction": instruction.text,
        "symbol": instruction.symbol, "at": at,
    })
}
