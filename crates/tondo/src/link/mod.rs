//! Tondo's own linker (toolchain, H12): executables without a C toolchain.
//! It finds the shared libraries the program needs where the system keeps
//! them to run programs, so it needs no development packages either.

mod elf;

use std::path::{Path, PathBuf};

/// Links the program's object with archives (the runtime first), the
/// system libraries the runtime needs (`-lname` and files, as Rust lists
/// them) and the native libraries of `[native]`. A static executable
/// links archives alone, the C library among them.
pub fn link(
    object: &[u8],
    archives: &[PathBuf],
    runtime: &[String],
    native: &[String],
    output: &Path,
    strip: bool,
    statically: bool,
) -> Result<(), String> {
    let arch = if cfg!(target_arch = "aarch64") { elf::Arch::Aarch64 } else { elf::Arch::X86_64 };
    let dirs = search_dirs(arch);
    let mut archive_paths: Vec<PathBuf> = archives.to_vec();
    let mut shared: Vec<PathBuf> = Vec::new();
    let mut runpath: Vec<String> = Vec::new();
    for library in native {
        if let Some(name) = library.strip_prefix("static:") {
            let file = format!("lib{name}.a");
            let found = dirs.iter().map(|d| d.join(&file)).find(|p| p.is_file());
            let Some(found) = found else {
                return Err(format!(
                    "no static library {file} in the system's library paths; give its file instead, as \"vendor/{file}\""
                ));
            };
            archive_paths.push(found);
            continue;
        }
        let path = Path::new(library);
        let archive = library.ends_with(".a");
        if statically && !archive {
            let name = path.file_name().map_or(library.clone(), |n| n.to_string_lossy().into_owned());
            return Err(format!(
                "a static executable cannot load the shared library `{name}`: give its static library, as \"static:{name}\" or a `.a` file, or build without --static"
            ));
        }
        if library.ends_with(".so") || library.contains(".so.") {
            if !path.is_file() {
                return Err(format!("the library {library} does not exist"));
            }
            let dir = path.parent().unwrap_or(Path::new("."));
            runpath.push(dir.display().to_string());
            shared.push(path.to_path_buf());
        } else if library.contains('/') || library.ends_with(".a") {
            if !path.is_file() {
                return Err(format!("the library {library} does not exist"));
            }
            archive_paths.push(path.to_path_buf());
        } else {
            shared.push(find_shared(arch, &dirs, library)?);
        }
    }
    // The C library first: the entry needs it.
    let mut system: Vec<PathBuf> = if statically { Vec::new() } else { vec![find_shared(arch, &dirs, "c")?] };
    for argument in runtime {
        match argument.strip_prefix("-l") {
            Some(name) => {
                let found = find_shared(arch, &dirs, name)?;
                if !system.contains(&found) {
                    system.push(found);
                }
            }
            None => archive_paths.push(PathBuf::from(argument)),
        }
    }
    // glibc's dynamic loader defines some of what C code uses, such as
    // `__stack_chk_guard` on aarch64.
    let loader = PathBuf::from(interpreter(arch));
    if !statically && is_elf(&loader, arch) && !system.contains(&loader) {
        system.push(loader);
    }
    let read = |path: &Path| std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()));
    let archive_data: Vec<(String, Vec<u8>)> =
        archive_paths.iter().map(|p| Ok((p.display().to_string(), read(p)?))).collect::<Result<_, String>>()?;
    let shared_data: Vec<(String, Vec<u8>)> =
        shared.iter().chain(&system).map(|p| Ok((p.display().to_string(), read(p)?))).collect::<Result<_, String>>()?;
    let inputs = elf::Inputs {
        arch,
        objects: vec![("the program".into(), object)],
        archives: archive_data.iter().map(|(n, d)| (n.clone(), d.as_slice())).collect(),
        shared: shared_data.iter().map(|(n, d)| (n.clone(), d.as_slice())).collect(),
        always_needed: shared.len(),
        runpath,
        interpreter: (!statically).then(|| interpreter(arch).into()),
        entry: "tondo_rt_start".into(),
        strip,
    };
    let bytes = elf::link(inputs).map_err(|message| match message.strip_prefix("undefined: ") {
        Some(names) => {
            let names: Vec<String> = names.split(' ').map(|n| format!("`{n}`")).collect();
            format!(
                "no linked library defines the C function {}: add its library to `[native] libraries` in tondo.toml, or pass `--link <library>` to `tondo run` or `tondo build`",
                names.join(", ")
            )
        }
        None => message,
    })?;
    std::fs::write(output, bytes).map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// The dynamic loader of the system's C library.
fn interpreter(arch: elf::Arch) -> &'static str {
    match (cfg!(target_env = "musl"), arch) {
        (false, elf::Arch::X86_64) => "/lib64/ld-linux-x86-64.so.2",
        (false, elf::Arch::Aarch64) => "/lib/ld-linux-aarch64.so.1",
        (true, elf::Arch::X86_64) => "/lib/ld-musl-x86_64.so.1",
        (true, elf::Arch::Aarch64) => "/lib/ld-musl-aarch64.so.1",
    }
}

/// Where the system keeps its libraries: what `/etc/ld.so.conf` lists, then
/// the usual places.
fn search_dirs(arch: elf::Arch) -> Vec<PathBuf> {
    let triplet = match arch {
        elf::Arch::X86_64 => "x86_64-linux-gnu",
        elf::Arch::Aarch64 => "aarch64-linux-gnu",
    };
    let mut dirs = Vec::new();
    read_conf(Path::new("/etc/ld.so.conf"), &mut dirs, 0);
    for dir in [
        format!("/lib/{triplet}"),
        format!("/usr/lib/{triplet}"),
        "/lib64".into(),
        "/usr/lib64".into(),
        "/lib".into(),
        "/usr/lib".into(),
        "/usr/local/lib".into(),
    ] {
        let dir = PathBuf::from(dir);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn read_conf(path: &Path, dirs: &mut Vec<PathBuf>, depth: u32) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if let Some(pattern) = line.strip_prefix("include") {
            let pattern = pattern.trim();
            let (dir, suffix) = match pattern.rsplit_once("/*") {
                Some((dir, suffix)) => (dir, suffix),
                None => (pattern, ""),
            };
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut files: Vec<PathBuf> =
                entries.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(suffix)).collect();
            files.sort();
            for file in files.into_iter().filter(|_| depth < 4) {
                read_conf(&file, dirs, depth + 1);
            }
        } else if line.starts_with('/') && !dirs.contains(&PathBuf::from(line)) {
            dirs.push(PathBuf::from(line));
        }
    }
}

/// The shared library `-lname` links: by the name the system's own
/// libraries run with, then `libname.so`, then the newest `libname.so.N`.
fn find_shared(arch: elf::Arch, dirs: &[PathBuf], name: &str) -> Result<PathBuf, String> {
    let musl = cfg!(target_env = "musl");
    let known: &[&str] = match name {
        // musl keeps all of these in its C library.
        "c" | "m" | "pthread" | "dl" | "rt" | "util" if musl => {
            &["libc.so", "ld-musl-x86_64.so.1", "ld-musl-aarch64.so.1"]
        }
        "c" => &["libc.so.6"],
        "m" => &["libm.so.6"],
        "pthread" => &["libpthread.so.0", "libc.so.6"],
        "dl" => &["libdl.so.2", "libc.so.6"],
        "rt" => &["librt.so.1", "libc.so.6"],
        "util" => &["libutil.so.1", "libc.so.6"],
        "gcc_s" => &["libgcc_s.so.1"],
        _ => &[],
    };
    let fits = |path: &Path| is_elf(path, arch);
    for file in known {
        if let Some(found) = dirs.iter().map(|d| d.join(file)).find(|p| fits(p)) {
            return Ok(found);
        }
    }
    let plain = format!("lib{name}.so");
    if let Some(found) = dirs.iter().map(|d| d.join(&plain)).find(|p| fits(p)) {
        return Ok(found);
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut versioned: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{plain}."))))
            .filter(|p| fits(p))
            .collect();
        versioned.sort_by_key(|p| version_key(p));
        if let Some(found) = versioned.pop() {
            return Ok(found);
        }
    }
    Err(format!(
        "the library `{name}` is not installed: no {plain} in {}",
        dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ")
    ))
}

/// The numbers after `.so.`, to find the newest.
fn version_key(path: &Path) -> Vec<u64> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let after = name.split(".so.").nth(1).unwrap_or_default();
    after.split('.').map(|n| n.parse().unwrap_or(0)).collect()
}

/// Whether a file is a 64-bit ELF file for `arch`, not a linker script or
/// a library of another word size.
fn is_elf(path: &Path, arch: elf::Arch) -> bool {
    use std::io::Read;
    let mut header = [0u8; 20];
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    if file.read_exact(&mut header).is_err() {
        return false;
    }
    let machine = u16::from_le_bytes([header[18], header[19]]);
    let wanted = match arch {
        elf::Arch::X86_64 => object::elf::EM_X86_64,
        elf::Arch::Aarch64 => object::elf::EM_AARCH64,
    };
    header.starts_with(b"\x7fELF") && header[4] == 2 && machine == wanted.0
}
