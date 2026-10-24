//! Static libraries (toolchain 7): an `ar` archive of objects with the
//! table of the symbols they define, which linkers read to pick members:
//! GNU's format on Linux, and on macOS BSD's, whose members start at
//! multiples of 8.

use object::{Object, ObjectSymbol};

/// The archive of `members`, each a name and its bytes.
pub fn archive(members: &[(String, &[u8])], darwin: bool) -> Vec<u8> {
    // The global symbols each member defines.
    let symbols: Vec<Vec<String>> = members
        .iter()
        .map(|(_, data)| match object::File::parse(*data) {
            Ok(file) => file
                .symbols()
                .filter(|s| s.is_global() && s.is_definition())
                .filter_map(|s| s.name().ok().map(str::to_string))
                .collect(),
            Err(_) => Vec::new(),
        })
        .collect();
    if darwin { bsd(members, &symbols) } else { gnu(members, &symbols) }
}

fn header(out: &mut Vec<u8>, name: &str, size: usize) {
    let text = format!("{name:<16}{:<12}{:<6}{:<6}{:<8}{size:<10}`\n", 0, 0, 0, 644);
    out.extend_from_slice(text.as_bytes());
}

fn pad(out: &mut Vec<u8>, align: usize, byte: u8) {
    while !out.len().is_multiple_of(align) {
        out.push(byte);
    }
}

/// GNU's: the symbol table `/`, with big-endian offsets, then the names
/// longer than 15 bytes in `//`, then the members, at even offsets.
fn gnu(members: &[(String, &[u8])], symbols: &[Vec<String>]) -> Vec<u8> {
    let mut long = Vec::new();
    let names: Vec<String> = members
        .iter()
        .map(|(name, _)| {
            if name.len() < 16 {
                format!("{name}/")
            } else {
                let at = long.len();
                long.extend_from_slice(name.as_bytes());
                long.extend_from_slice(b"/\n");
                format!("/{at}")
            }
        })
        .collect();
    let count: usize = symbols.iter().map(Vec::len).sum();
    let table = 4 + 4 * count + symbols.iter().flatten().map(|s| s.len() + 1).sum::<usize>();
    let even = |n: usize| n + n % 2;
    let mut at = 8 + 60 + even(table) + if long.is_empty() { 0 } else { 60 + even(long.len()) };
    let mut offsets = Vec::new();
    for (_, data) in members {
        offsets.push(at);
        at += 60 + even(data.len());
    }
    let mut out = b"!<arch>\n".to_vec();
    header(&mut out, "/", table);
    out.extend_from_slice(&(count as u32).to_be_bytes());
    for (member, names) in symbols.iter().enumerate() {
        for _ in names {
            out.extend_from_slice(&(offsets[member] as u32).to_be_bytes());
        }
    }
    for name in symbols.iter().flatten() {
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    pad(&mut out, 2, b'\n');
    if !long.is_empty() {
        header(&mut out, "//", long.len());
        out.extend_from_slice(&long);
        pad(&mut out, 2, b'\n');
    }
    for ((_, data), name) in members.iter().zip(&names) {
        header(&mut out, name, data.len());
        out.extend_from_slice(data);
        pad(&mut out, 2, b'\n');
    }
    out
}

/// BSD's, as Apple's linker reads it: every name follows its header
/// (`#1/<length>`), padded with zeros so that the data starts at a
/// multiple of 8; the table `__.SYMDEF` comes first, little-endian.
fn bsd(members: &[(String, &[u8])], symbols: &[Vec<String>]) -> Vec<u8> {
    // The name's length with its padding, for a member at offset `at`.
    let padded = |at: usize, name: &str| (at + 60 + name.len()).next_multiple_of(8) - at - 60;
    let mut strings = Vec::new();
    for name in symbols.iter().flatten() {
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
    }
    while !strings.len().is_multiple_of(8) {
        strings.push(0);
    }
    let count: usize = symbols.iter().map(Vec::len).sum();
    let table = 4 + 8 * count + 4 + strings.len();
    let symdef = "__.SYMDEF";
    let mut at = 8;
    at += 60 + padded(at, symdef) + table.next_multiple_of(8);
    let mut offsets = Vec::new();
    for (name, data) in members {
        offsets.push(at);
        at += 60 + padded(at, name) + data.len().next_multiple_of(8);
    }
    let mut out = b"!<arch>\n".to_vec();
    let member = |out: &mut Vec<u8>, name: &str, size: usize| {
        let length = padded(out.len(), name);
        header(out, &format!("#1/{length}"), length + size.next_multiple_of(8));
        out.extend_from_slice(name.as_bytes());
        out.resize(out.len() + length - name.len(), 0);
    };
    member(&mut out, symdef, table);
    out.extend_from_slice(&((8 * count) as u32).to_le_bytes());
    let mut string = 0;
    for (index, names) in symbols.iter().enumerate() {
        for name in names {
            out.extend_from_slice(&(string as u32).to_le_bytes());
            out.extend_from_slice(&(offsets[index] as u32).to_le_bytes());
            string += name.len() + 1;
        }
    }
    out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
    out.extend_from_slice(&strings);
    pad(&mut out, 8, 0);
    for (name, data) in members {
        member(&mut out, name, data.len());
        out.extend_from_slice(data);
        pad(&mut out, 8, 0);
    }
    out
}
