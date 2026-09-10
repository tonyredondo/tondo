//! Build-time source identity for the compiler-owned hosted standard package.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

// This deliberately includes complete implementation crates: a new lowering,
// host operation or VM helper cannot escape the identity through a hand-picked
// list of functions. Inline Rust tests also invalidate this conservative hash.
pub const INPUTS: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "crates/tondo-compiler/Cargo.toml",
    "crates/tondo-compiler/build.rs",
    "crates/tondo-compiler/build_support",
    "crates/tondo-compiler/src",
    "crates/tondo-stdlib/Cargo.toml",
    "crates/tondo-stdlib/src",
    "crates/tondo-vm/Cargo.toml",
    "crates/tondo-vm/src",
    "stdlib/meta",
];

pub fn read_inputs(root: &Path) -> io::Result<Vec<(String, Vec<u8>)>> {
    let mut inputs = Vec::new();
    for path in INPUTS {
        read_path(root, Path::new(path), &mut inputs)?;
    }
    Ok(inputs)
}

fn read_path(root: &Path, relative: &Path, inputs: &mut Vec<(String, Vec<u8>)>) -> io::Result<()> {
    let path = root.join(relative);
    let kind = fs::symlink_metadata(&path)?.file_type();
    if kind.is_dir() {
        for entry in fs::read_dir(path)? {
            read_path(root, &relative.join(entry?.file_name()), inputs)?;
        }
    } else if kind.is_file() {
        let logical = relative
            .to_str()
            .ok_or_else(|| io::Error::other("standard source paths must be UTF-8"))?
            .replace('\\', "/");
        inputs.push((logical, fs::read(path)?));
    } else {
        return Err(io::Error::other(format!(
            "standard source input must be a regular file or directory: {}",
            relative.display()
        )));
    }
    Ok(())
}

pub fn fingerprint(inputs: &[(String, Vec<u8>)]) -> io::Result<String> {
    let mut ordered = BTreeMap::new();
    for (path, bytes) in inputs {
        if ordered.insert(path, bytes).is_some() {
            return Err(io::Error::other(format!(
                "duplicate standard source path: {path}"
            )));
        }
    }
    let mut digest = Sha256::new();
    digest.update(b"tondo-hosted-standard-source-bundle/1\0");
    for (path, bytes) in ordered {
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    let mut output = String::from("sha256:");
    for byte in digest.finalize() {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(output)
}
