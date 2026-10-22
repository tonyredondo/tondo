//! The snapshot store and the artifact store (testing 15.7, 15.8).

use super::json::Value;
use crate::hash::{sha256, sha256_hex};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The snapshot store of a package: its entries by test or suite and name.
pub(super) struct Snapshots {
    pub path: PathBuf,
    pub entries: BTreeMap<(String, String), String>,
    /// The hash of the store tree before the run.
    pub before: String,
    package: String,
}

const HEADER: &str = "{\n  \"format\": \"tondo-snapshot-store-0.1/1\",\n";

impl Snapshots {
    /// Reads `snapshots/tondo.snap.json` of the package at `dir`, which must
    /// be canonical; a store that is not there is empty.
    pub(super) fn read(dir: &Path, package: &str) -> Result<Snapshots, String> {
        let path = dir.join("snapshots/tondo.snap.json");
        let shown = "snapshots/tondo.snap.json";
        let mut entries = BTreeMap::new();
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if !meta.is_file() {
                return Err(format!("{shown} is not a regular file"));
            }
            let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {shown}: {e}"))?;
            entries = parse(&text).ok_or_else(|| {
                format!(
                    "{shown} is not a canonical snapshot store; fix it, or remove it and run `tondo test --update-snapshots`"
                )
            })?;
            if serialize(&entries) != text {
                return Err(format!(
                    "{shown} is not in its canonical form: entries sorted by id and name, one per line"
                ));
            }
        }
        let before = tree_hash(package, &serialize(&entries));
        Ok(Snapshots { path, entries, before, package: package.to_string() })
    }

    /// The entries for the worker: three lengths on a line, then the
    /// identity, the name and the value.
    pub(super) fn for_worker(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for ((id, name), value) in &self.entries {
            out.extend_from_slice(format!("{} {} {}\n", id.len(), name.len(), value.len()).as_bytes());
            out.extend_from_slice(id.as_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(value.as_bytes());
        }
        out
    }

    /// An entry's value, if there is one.
    pub(super) fn expected(&self, id: &str, name: &str) -> Option<String> {
        self.entries.get(&(id.to_string(), name.to_string())).cloned()
    }

    /// Writes the store with these values set, keeping every other entry;
    /// the hash of the tree after.
    pub(super) fn publish(&self, values: Vec<((String, String), String)>) -> Result<String, String> {
        let mut entries = self.entries.clone();
        entries.extend(values);
        let text = serialize(&entries);
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        write_atomically(&self.path, text.as_bytes())?;
        Ok(tree_hash(&self.package, &text))
    }
}

/// The hash of the stores of the packages under test (`tondo-snapshot-tree-v1`).
pub(super) fn tree_hash(package: &str, store: &str) -> String {
    let mut bytes = b"tondo-snapshot-tree-v1\0".to_vec();
    bytes.extend_from_slice(package.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&sha256(store.as_bytes()));
    sha256_hex(&bytes)
}

fn serialize(entries: &BTreeMap<(String, String), String>) -> String {
    if entries.is_empty() {
        return format!("{HEADER}  \"entries\": []\n}}\n");
    }
    let lines: Vec<String> = entries
        .iter()
        .map(|((id, name), value)| {
            let field = |key: &str, text: &str| (key.to_string(), Value::Str(text.to_string()));
            let entry = Value::Object(vec![field("id", id), field("name", name), field("value", value)]);
            format!("    {}", entry.text())
        })
        .collect();
    format!("{HEADER}  \"entries\": [\n{}\n  ]\n}}\n", lines.join(",\n"))
}

/// The entries of a store, read line by line.
fn parse(text: &str) -> Option<BTreeMap<(String, String), String>> {
    let rest = text.strip_prefix(HEADER)?;
    let mut entries = BTreeMap::new();
    if rest == "  \"entries\": []\n}\n" {
        return Some(entries);
    }
    let body = rest.strip_prefix("  \"entries\": [\n")?.strip_suffix("\n  ]\n}\n")?;
    for line in body.split('\n') {
        let line = line.strip_prefix("    ")?;
        let line = line.strip_suffix(',').unwrap_or(line);
        let mut reader = Reader(line.chars().collect(), 0);
        reader.expect("{\"id\":")?;
        let id = reader.string()?;
        reader.expect(",\"name\":")?;
        let name = reader.string()?;
        reader.expect(",\"value\":")?;
        let value = reader.string()?;
        reader.expect("}")?;
        if reader.1 != reader.0.len() || entries.insert((id, name), value).is_some() {
            return None;
        }
    }
    Some(entries)
}

struct Reader(Vec<char>, usize);

impl Reader {
    fn expect(&mut self, text: &str) -> Option<()> {
        for c in text.chars() {
            (self.0.get(self.1) == Some(&c)).then_some(())?;
            self.1 += 1;
        }
        Some(())
    }

    fn string(&mut self) -> Option<String> {
        self.expect("\"")?;
        let mut out = String::new();
        loop {
            let c = *self.0.get(self.1)?;
            self.1 += 1;
            match c {
                '"' => return Some(out),
                '\\' => {
                    let e = *self.0.get(self.1)?;
                    self.1 += 1;
                    out.push(match e {
                        '"' | '\\' | '/' => e,
                        'b' => '\u{8}',
                        't' => '\t',
                        'n' => '\n',
                        'f' => '\u{c}',
                        'r' => '\r',
                        'u' => {
                            let hex: String = self.0.get(self.1..self.1 + 4)?.iter().collect();
                            self.1 += 4;
                            char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
                        }
                        _ => return None,
                    });
                }
                c => out.push(c),
            }
        }
    }
}

/// The artifact store: content-addressed objects and their manifest.
pub(super) struct Artifacts {
    pub root: PathBuf,
    /// The first object that could not be written, which fails the run.
    pub error: Mutex<Option<String>>,
}

impl Artifacts {
    /// Stores the bytes as `objects/<sha256>`, unless an object with exactly
    /// them is there; their hash.
    pub(super) fn store(&self, bytes: &[u8]) -> String {
        let hash = sha256_hex(bytes);
        let path = self.root.join("objects").join(&hash);
        let result = match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() && std::fs::read(&path).is_ok_and(|b| sha256_hex(&b) == hash) => Ok(()),
            Ok(_) => Err(format!("the artifact object {} is not what its name says", path.display())),
            Err(_) => std::fs::create_dir_all(self.root.join("objects"))
                .map_err(|e| e.to_string())
                .and_then(|_| write_atomically(&path, bytes)),
        };
        if let Err(error) = result {
            self.error.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(error);
        }
        hash
    }
}

pub(super) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let partial = path.with_extension(format!("partial-{}", std::process::id()));
    std::fs::write(&partial, bytes)
        .and_then(|_| std::fs::rename(&partial, path))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_round_trip_canonically() {
        let mut entries = BTreeMap::new();
        entries.insert(("a::unit::a::t".to_string(), "out".to_string()), "line\n\"quoted\" \u{1}".to_string());
        entries.insert(("a::unit::a::s".to_string(), "x".to_string()), String::new());
        let text = serialize(&entries);
        assert_eq!(parse(&text), Some(entries));
        let empty = serialize(&BTreeMap::new());
        assert_eq!(empty, "{\n  \"format\": \"tondo-snapshot-store-0.1/1\",\n  \"entries\": []\n}\n");
        assert_eq!(parse(&empty), Some(BTreeMap::new()));
        assert_eq!(parse(&text.replace("    {", "   {")), None);
    }
}
