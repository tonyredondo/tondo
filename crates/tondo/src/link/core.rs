//! What the linkers of every format share: how archives' members come in,
//! the image they write, and how they say what is wrong with an input.

use crate::hash::FastMap;
use std::collections::HashSet;

pub(super) fn error(input: &str, message: impl std::fmt::Display) -> String {
    format!("linking {input}: {message}")
}

/// The bytes of the output, written at offsets in any order.
pub(super) struct Image {
    pub(super) bytes: Vec<u8>,
}

impl Image {
    pub(super) fn put(&mut self, at: u64, data: &[u8]) {
        let at = at as usize;
        if self.bytes.len() < at + data.len() {
            self.bytes.resize(at + data.len(), 0);
        }
        self.bytes[at..at + data.len()].copy_from_slice(data);
    }

    pub(super) fn u16(&mut self, at: u64, v: u16) {
        self.put(at, &v.to_le_bytes());
    }

    pub(super) fn u32(&mut self, at: u64, v: u32) {
        self.put(at, &v.to_le_bytes());
    }

    pub(super) fn u64(&mut self, at: u64, v: u64) {
        self.put(at, &v.to_le_bytes());
    }

    pub(super) fn read32(&self, at: u64) -> u32 {
        u32::from_le_bytes(self.bytes[at as usize..at as usize + 4].try_into().unwrap())
    }

    pub(super) fn read64(&self, at: u64) -> u64 {
        u64::from_le_bytes(self.bytes[at as usize..at as usize + 8].try_into().unwrap())
    }
}

/// What a linker does with the members of archives: which global a member
/// would define for it now, and how it takes one in.
pub(super) trait Members<'a> {
    /// A member read, before it is taken in.
    type Object: Send;
    /// The name of global `id` while it is undefined and a definition of
    /// it brings a member in.
    fn wanted(&self, id: usize) -> Option<&'a [u8]>;
    /// Takes an object in, and gives the globals it added.
    fn take_member(&mut self, object: Self::Object) -> Result<Vec<usize>, String>;
    /// Makes room for as many more globals.
    fn reserve(&mut self, count: usize);
}

/// Brings in the members of `archives` that define what the linker wants,
/// starting from the globals of `pending`, and then what those want, in
/// rounds: the members wanted now are read by `parse` on every core, and
/// taken in, in the order of the archives, while what brought each in is
/// still wanted.
pub(super) fn load_members<'a, L: Members<'a>>(
    linker: &mut L,
    archives: &[(String, &'a [u8])],
    mut pending: Vec<usize>,
    parse: impl Fn(String, &'a [u8]) -> Result<L::Object, String> + Sync,
) -> Result<(), String> {
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
    linker.reserve(index.len());
    let mut loaded: HashSet<(usize, u64)> = HashSet::new();
    loop {
        let mut wanted: Vec<((usize, u64), usize)> = Vec::new();
        for id in pending.drain(..) {
            let Some(name) = linker.wanted(id) else { continue };
            let Some(&member) = index.get(name) else { continue };
            if loaded.insert(member) {
                wanted.push((member, id));
            }
        }
        if wanted.is_empty() {
            return Ok(());
        }
        wanted.sort_unstable();
        let read = crate::parallel::map(&wanted, 4, |&((a, offset), _)| {
            let (archive_name, data) = &archives[a];
            let member =
                parsed[a].member(object::read::archive::ArchiveOffset(offset)).map_err(|e| error(archive_name, e))?;
            let bytes = member.data(*data).map_err(|e| error(archive_name, e))?;
            parse(format!("{archive_name}({})", String::from_utf8_lossy(member.name())), bytes)
        });
        for ((member, id), read) in wanted.into_iter().zip(read) {
            if linker.wanted(id).is_none() {
                // Another member of this round defined it first.
                loaded.remove(&member);
                continue;
            }
            pending.extend(linker.take_member(read?)?);
        }
    }
}
