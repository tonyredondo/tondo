//! Packages and their dependencies: `tondo.toml`, fetching git repositories
//! and archives into the cache, and `tondo.lock`, which pins the exact
//! commit or content of each fetched dependency.

use crate::project::{Arch, ProjectFile, System, is_identifier};
use crate::toml::{self, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Package {
    /// The package name: the first segment of its module paths, unique in
    /// the graph.
    pub name: String,
    /// Its `[package] version`, or `0.0.0`.
    pub version: String,
    pub dir: PathBuf,
    /// The file of a script that declares dependencies (toolchain 2), the
    /// package's one source, in `dir`.
    pub script: Option<PathBuf>,
    /// The repository and commit it was fetched from, if it was.
    pub origin: Option<(String, String)>,
    /// `[source] url`: the page of a line at the project's own host.
    pub source_url: Option<String>,
    /// Its dependencies by alias, as indices into the graph.
    pub dependencies: Vec<(String, usize)>,
    /// The project's dependencies that only its tests import
    /// (`[dev-dependencies]`); a dependency's are not fetched.
    pub dev_dependencies: Vec<(String, usize)>,
    /// Reached only through dev-dependencies: a build of the program does
    /// not load it.
    pub tests_only: bool,
    /// What `[native] libraries` links: `-lname`, or a library file.
    pub libraries: Vec<String>,
    /// The provider of each trait it declares that `derive` can ask for
    /// (`[derive]`): the trait's name and the provider's file.
    pub providers: Vec<(String, PathBuf)>,
    /// The generators it declares (`[generators.<name>]`).
    pub generators: Vec<Generator>,
    /// The systems it runs on (`[package] systems`), all of them when it
    /// names none (language 6.8).
    pub systems: Vec<System>,
    /// The architectures it runs on (`[package] architectures`), both when
    /// it names none.
    pub architectures: Vec<Arch>,
    /// The level of processor its programs are built for (`[build] cpu`),
    /// and where its manifest says it, for an error about it.
    pub cpu: Option<(String, String)>,
    /// Whether its builds leave a place at each line for a logpoint
    /// (`[build] logpoints`).
    pub logpoints: bool,
    /// Whether its release builds put in regions what the program makes and
    /// lets go (`[build] lifetimes`).
    pub regions: bool,
}

impl Package {
    /// Its sources, as `project::sources` gives them: a script's one file, as
    /// the module of its name, or those of its directory.
    pub fn sources(&self, target: Option<crate::project::Target>) -> Result<(Vec<ProjectFile>, usize), String> {
        match &self.script {
            Some(file) => Ok((vec![ProjectFile { path: file.clone(), module: vec![self.name.clone()] }], 0)),
            None => crate::project::sources(&self.dir, &self.name, target),
        }
    }
}

/// A generator (27.5 of the language): a program that writes modules of
/// the package from files it is given and declarations it may look at.
#[derive(Clone, Debug)]
pub struct Generator {
    pub name: String,
    pub program: PathBuf,
    /// Each input by the name the manifest gives it, and its file.
    pub inputs: Vec<(String, PathBuf)>,
    /// The modules it writes, as paths inside the package: `proto.user`.
    pub outputs: Vec<String>,
    /// The public declarations it may look at, as `module.path.Type`.
    pub roots: Vec<String>,
    /// Its budgets of steps, memory and output, when it declares them.
    pub steps: Option<u64>,
    pub memory: Option<u64>,
    pub output: Option<u64>,
}

#[derive(Clone)]
enum Source {
    Path(PathBuf),
    Git { url: String, reference: Reference },
    Archive { url: String, sha256: Option<String> },
}

#[derive(Clone)]
enum Reference {
    Tag(String),
    Branch(String),
    Rev(String),
    Default,
}

impl Source {
    /// How the lockfile names the source.
    fn key(&self) -> String {
        match self {
            Source::Path(path) => path.display().to_string(),
            Source::Git { url, reference } => match reference {
                Reference::Tag(tag) => format!("git+{url}?tag={tag}"),
                Reference::Branch(branch) => format!("git+{url}?branch={branch}"),
                Reference::Rev(rev) => format!("git+{url}?rev={rev}"),
                Reference::Default => format!("git+{url}"),
            },
            Source::Archive { url, .. } => url.clone(),
        }
    }
}

struct Manifest {
    source_url: Option<String>,
    name: Option<String>,
    version: Option<String>,
    dependencies: Vec<(String, Source)>,
    dev_dependencies: Vec<(String, Source)>,
    libraries: Vec<String>,
    providers: Vec<(String, PathBuf)>,
    generators: Vec<Generator>,
    systems: Vec<System>,
    architectures: Vec<Arch>,
    cpu: Option<(String, String)>,
    logpoints: bool,
    regions: bool,
}

impl Manifest {
    fn empty() -> Manifest {
        Manifest {
            source_url: None,
            name: None,
            version: None,
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            libraries: Vec::new(),
            providers: Vec::new(),
            generators: Vec::new(),
            systems: Vec::new(),
            architectures: Vec::new(),
            cpu: None,
            logpoints: true,
            regions: true,
        }
    }
}

/// The number of lines of a script's header (toolchain 2): those before
/// its first line of code, a shebang, comments and blank lines.
pub fn header_lines(text: &str) -> usize {
    let header = |(index, line): &(usize, &str)| {
        let line = line.trim();
        line.is_empty() || line.starts_with("//") || (*index == 0 && line.starts_with("#!"))
    };
    text.lines().enumerate().take_while(header).count()
}

/// The word after `// tondo:` in a line of comment, and what follows it.
pub fn directive(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim().strip_prefix("//")?.trim_start().strip_prefix("tondo:")?.trim_start();
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some((&rest[..end], rest[end..].trim()))
}

/// Whether the file at `path` is a script that declares something with
/// `// tondo:` in its header, but `allow`: then it is a package of its own,
/// with its dependencies (toolchain 2).
pub fn is_script_package(path: &Path) -> bool {
    if path.is_dir() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    text.lines().take(header_lines(&text)).filter_map(directive).any(|(word, _)| word != "allow")
}

/// Where the lockfile of a script is: beside it, its name and `.lock`.
pub fn script_lock(script: &Path) -> PathBuf {
    let mut name = script.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    script.with_file_name(name)
}

/// An alias and its source, from what follows `// tondo: dependency`.
fn declared(rest: &str) -> Result<(String, Source), String> {
    let usage = "write `// tondo: dependency name = { git = \"https://...\", tag = \"v1.0.0\" }`";
    let tables = toml::parse(rest).map_err(|e| format!("{}; {usage}", e.trim_start_matches("line 1: ")))?;
    match tables.first().map(|t| t.entries.as_slice()) {
        Some([(alias, value, _)]) if tables.len() == 1 => Ok((alias.clone(), dependency(alias, value)?)),
        _ => Err(usage.to_string()),
    }
}

/// The `tondo add` that writes the dependency of a `// tondo: dependency`
/// line into a project's tondo.toml.
pub fn add_command(rest: &str) -> Option<String> {
    let (alias, source) = declared(rest).ok()?;
    let flags = match source {
        Source::Path(path) => format!("--path {}", path.display()),
        Source::Git { url, reference } => match reference {
            Reference::Tag(tag) => format!("--git {url} --tag {tag}"),
            Reference::Branch(branch) => format!("--git {url} --branch {branch}"),
            Reference::Rev(rev) => format!("--git {url} --rev {rev}"),
            Reference::Default => format!("--git {url}"),
        },
        Source::Archive { url, sha256: None } => format!("--url {url}"),
        Source::Archive { url, sha256: Some(hash) } => format!("--url {url} --sha256 {hash}"),
    };
    Some(format!("tondo add {alias} {flags}"))
}

/// What the header of a script declares: its dependencies (toolchain 2).
fn script_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let at = |line: usize, message: String| format!("{}:{line}: {message}", path.display());
    let mut manifest = Manifest::empty();
    let mut lines: Vec<(String, usize)> = Vec::new();
    for (index, line) in text.lines().take(header_lines(&text)).enumerate() {
        let Some((word, rest)) = directive(line) else { continue };
        match word {
            "allow" => {}
            "dependency" => {
                let (alias, source) = declared(rest).map_err(|e| at(index + 1, e))?;
                if let Some((_, first)) = lines.iter().find(|(a, _)| *a == alias) {
                    return Err(at(index + 1, format!("dependency `{alias}` is declared on line {first} too")));
                }
                lines.push((alias.clone(), index + 1));
                manifest.dependencies.push((alias, source));
            }
            other => {
                return Err(at(
                    index + 1,
                    format!(
                        "a script declares only its dependencies, `// tondo: dependency name = {{ ... }}`, not `{other}`: one that needs more is {}",
                        crate::manage::SCRIPT_AS_PROJECT
                    ),
                ));
            }
        }
    }
    Ok(manifest)
}

/// Reads `tondo.toml` in `dir`; a missing file declares nothing.
fn manifest(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join("tondo.toml");
    let mut manifest = Manifest::empty();
    if !path.is_file() {
        return Ok(manifest);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let at = |line: usize, message: String| format!("{}:{line}: {message}", path.display());
    for table in toml::parse(&text).map_err(|e| format!("{}: {e}", path.display()))? {
        match (table.name.as_str(), table.array) {
            ("", false) => {
                if let Some((key, _, line)) = table.entries.first() {
                    return Err(at(*line, format!("`{key}` must be inside a table such as `[package]`")));
                }
            }
            ("package", false) => {
                for (key, value, line) in table.entries {
                    if key == "systems" {
                        let usage = "`systems = [\"linux\"]` names the systems the package runs on: `linux`, `macos`";
                        let items = match &value {
                            Value::Array(items) if !items.is_empty() => items,
                            _ => return Err(at(line, usage.into())),
                        };
                        for item in items {
                            let system = item.as_str().and_then(System::parse).ok_or_else(|| at(line, usage.into()))?;
                            if !manifest.systems.contains(&system) {
                                manifest.systems.push(system);
                            }
                        }
                        continue;
                    }
                    if key == "architectures" {
                        let usage =
                            "`architectures = [\"x64\"]` names the architectures the package runs on: `x64`, `arm64`";
                        let items = match &value {
                            Value::Array(items) if !items.is_empty() => items,
                            _ => return Err(at(line, usage.into())),
                        };
                        for item in items {
                            let arch = item.as_str().and_then(Arch::parse).ok_or_else(|| at(line, usage.into()))?;
                            if !manifest.architectures.contains(&arch) {
                                manifest.architectures.push(arch);
                            }
                        }
                        continue;
                    }
                    match (key.as_str(), value.as_str()) {
                        ("name", Some(name)) if is_identifier(name) => manifest.name = Some(name.to_string()),
                        ("name", _) => return Err(at(line, "the package name must be an identifier".into())),
                        ("version", Some(version))
                            if !version.is_empty() && !version.chars().any(|c| c.is_whitespace() || c.is_control()) =>
                        {
                            manifest.version = Some(version.to_string())
                        }
                        ("version", _) => {
                            return Err(at(line, "the version is a string without spaces, as \"1.2.0\"".into()));
                        }
                        ("edition", Some("0.1")) => {}
                        ("edition", _) => return Err(at(line, "the only edition is \"0.1\"".into())),
                        _ => return Err(at(line, format!("unknown key `{key}` in `[package]`"))),
                    }
                }
            }
            (table_name @ ("dependencies" | "dev-dependencies"), false) => {
                for (alias, value, line) in table.entries {
                    let source = dependency(&alias, &value).map_err(|e| at(line, e))?;
                    let declared = manifest.dependencies.iter().chain(&manifest.dev_dependencies);
                    if declared.clone().any(|(other, _)| *other == alias) {
                        return Err(at(line, format!("`{alias}` is declared twice; a dependency has one alias")));
                    }
                    match table_name {
                        "dependencies" => manifest.dependencies.push((alias, source)),
                        _ => manifest.dev_dependencies.push((alias, source)),
                    }
                }
            }
            ("source", false) => {
                for (key, value, line) in table.entries {
                    match (key.as_str(), value.as_str()) {
                        ("url", Some(url)) if url.contains("{path}") => manifest.source_url = Some(url.to_string()),
                        ("url", _) => {
                            let usage = "`url` is the page of a line, as \"https://git.acme.com/{repo}/blob/{commit}/{path}#L{line}\"";
                            return Err(at(line, usage.into()));
                        }
                        _ => return Err(at(line, format!("unknown key `{key}` in `[source]`"))),
                    }
                }
            }
            ("build", false) => {
                for (key, value, line) in table.entries {
                    match (key.as_str(), value.as_str()) {
                        ("cpu", Some(level)) => {
                            manifest.cpu = Some((level.to_string(), format!("{}:{line}", path.display())));
                        }
                        ("cpu", _) => {
                            let usage = "`cpu = \"x86-64-v3\"` names the level of processor the program is built for";
                            return Err(at(line, usage.into()));
                        }
                        ("logpoints", _) => match &value {
                            crate::toml::Value::Boolean(on) => manifest.logpoints = *on,
                            _ => {
                                let usage = "`logpoints = false` builds the program without a place at each line \
                                             for a logpoint, which costs a `nop` each time the line runs";
                                return Err(at(line, usage.into()));
                            }
                        },
                        ("lifetimes", _) => match &value {
                            crate::toml::Value::Boolean(on) => manifest.regions = *on,
                            _ => {
                                let usage = "`lifetimes = false` builds the program without regions: what it makes \
                                             and lets go goes on the heap";
                                return Err(at(line, usage.into()));
                            }
                        },
                        _ => {
                            return Err(at(
                                line,
                                format!(
                                    "unknown key `{key}` in `[build]`, which has `cpu`, `logpoints` and `lifetimes`"
                                ),
                            ));
                        }
                    }
                }
            }
            ("native", false) => {
                for (key, value, line) in table.entries {
                    let usage = "`libraries = [\"sqlite3\", \"static:z\", \"vendor/libpng.a\"]` lists libraries by name or file";
                    let Value::Array(items) = value else { return Err(at(line, usage.into())) };
                    if key != "libraries" {
                        return Err(at(line, format!("unknown key `{key}` in `[native]`: {usage}")));
                    }
                    for item in items {
                        let Some(library) = item.as_str().filter(|l| !l.is_empty()) else {
                            return Err(at(line, usage.into()));
                        };
                        manifest.libraries.push(library_entry(library, dir));
                    }
                }
            }
            ("derive", false) => {
                for (key, value, line) in table.entries {
                    let usage = format!(
                        "`{key} = \"derive/{}.to\"` names the file of the trait's provider",
                        key.to_lowercase()
                    );
                    let Some(file) = value.as_str().filter(|f| f.ends_with(".to")) else {
                        return Err(at(line, usage));
                    };
                    if !is_identifier(&key) {
                        return Err(at(line, format!("`{key}` is not the name of a trait")));
                    }
                    manifest.providers.push((key, dir.join(file)));
                }
            }
            (name, false) if name.starts_with("generators.") => {
                let generator =
                    generator(&name["generators.".len()..], table.entries, dir).map_err(|(line, e)| at(line, e))?;
                manifest.generators.push(generator);
            }
            (name, _) => {
                let message = format!(
                    "unknown table `[{name}]`: tondo.toml has `[package]`, `[build]`, `[source]`, `[dependencies]`, `[dev-dependencies]`, `[native]`, `[derive]` and `[generators.<name>]`"
                );
                return Err(at(table.line, message));
            }
        }
    }
    Ok(manifest)
}

/// A `[generators.<name>]` table: `program`, `inputs`, `outputs`, and
/// optionally `roots` and `limits`.
fn generator(name: &str, entries: Vec<(String, Value, usize)>, dir: &Path) -> Result<Generator, (usize, String)> {
    let usage = "a generator has `program = \"gen/x.to\"`, `inputs = [\"file\", ...]`, `outputs = [\"module.path\", ...]`, and optionally `roots = [\"module.Type\", ...]` and `limits = { steps = \"...\", memory = \"...\", output = \"...\" }`";
    if !is_identifier(name) {
        return Err((0, format!("`{name}` is not a generator name: {usage}")));
    }
    let mut generator = Generator {
        name: name.to_string(),
        program: PathBuf::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        roots: Vec::new(),
        steps: None,
        memory: None,
        output: None,
    };
    let strings = |value: &Value, line: usize| -> Result<Vec<String>, (usize, String)> {
        match value {
            Value::Array(items) => {
                items.iter().map(|i| i.as_str().map(String::from).ok_or((line, usage.to_string()))).collect()
            }
            _ => Err((line, usage.to_string())),
        }
    };
    for (key, value, line) in entries {
        match key.as_str() {
            "program" => {
                let file = value.as_str().filter(|f| f.ends_with(".to")).ok_or((line, usage.to_string()))?;
                generator.program = dir.join(file);
            }
            "inputs" => {
                generator.inputs = strings(&value, line)?.into_iter().map(|i| (i.clone(), dir.join(i))).collect()
            }
            "outputs" => generator.outputs = strings(&value, line)?,
            "roots" => generator.roots = strings(&value, line)?,
            "limits" => {
                let Value::Table(limits) = value else { return Err((line, usage.to_string())) };
                for (limit, amount) in limits {
                    let amount = amount.as_str().and_then(|a| a.parse::<u64>().ok()).filter(|&a| a > 0);
                    let amount = amount.ok_or((line, format!("`{limit}` is a positive number in a string")))?;
                    match limit.as_str() {
                        "steps" => generator.steps = Some(amount),
                        "memory" => generator.memory = Some(amount),
                        "output" => generator.output = Some(amount),
                        _ => return Err((line, usage.to_string())),
                    }
                }
            }
            _ => return Err((line, format!("unknown key `{key}`: {usage}"))),
        }
    }
    if generator.program.as_os_str().is_empty() || generator.outputs.is_empty() {
        return Err((0, format!("the generator `{name}` needs `program` and `outputs`: {usage}")));
    }
    Ok(generator)
}

/// A native library as `[native] libraries` or `--link` names it: a name,
/// linked dynamically from the system's library paths; `static:name`,
/// linked statically from them; or a library file, whose path, relative to
/// `dir`, becomes absolute here.
pub fn library_entry(library: &str, dir: &Path) -> String {
    let file = library.contains('/') || [".a", ".so", ".dylib"].iter().any(|e| library.ends_with(e));
    if file { dir.join(library).display().to_string() } else { library.to_string() }
}

/// The source of a dependency: `{ path = ... }`, `{ git = ..., tag = ... }`
/// or `{ url = ... }`.
fn dependency(alias: &str, value: &Value) -> Result<Source, String> {
    if !is_identifier(alias) || alias == "std" {
        return Err(format!("`{alias}` cannot name a dependency: use an identifier other than `std`"));
    }
    let usage = "use `{ path = \"../dir\" }`, `{ git = \"https://...\", tag = \"v1.0.0\" }` or `{ url = \"https://.../package.tar.gz\" }`";
    let Value::Table(fields) = value else { return Err(format!("dependency `{alias}`: {usage}")) };
    let mut get = HashMap::new();
    for (key, value) in fields {
        let Some(text) = value.as_str() else { return Err(format!("dependency `{alias}`: `{key}` must be a string")) };
        get.insert(key.as_str(), text.to_string());
    }
    let allowed: &[&str] = match () {
        _ if get.contains_key("path") => &["path"],
        _ if get.contains_key("git") => &["git", "tag", "branch", "rev"],
        _ if get.contains_key("url") => &["url", "sha256"],
        _ => return Err(format!("dependency `{alias}`: {usage}")),
    };
    if let Some(key) = get.keys().find(|k| !allowed.contains(k)) {
        return Err(format!("dependency `{alias}`: `{key}` does not go with `{}`; {usage}", allowed[0]));
    }
    if let Some(path) = get.remove("path") {
        return Ok(Source::Path(PathBuf::from(path)));
    }
    if let Some(url) = get.remove("git") {
        let references: Vec<Reference> = [
            get.remove("tag").map(Reference::Tag),
            get.remove("branch").map(Reference::Branch),
            get.remove("rev").map(Reference::Rev),
        ]
        .into_iter()
        .flatten()
        .collect();
        if references.len() > 1 {
            return Err(format!("dependency `{alias}`: give one of `tag`, `branch` or `rev`"));
        }
        return Ok(Source::Git { url, reference: references.into_iter().next().unwrap_or(Reference::Default) });
    }
    let url = get.remove("url").unwrap();
    let sha256 = get.remove("sha256");
    if sha256.as_ref().is_some_and(|h| h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit())) {
        return Err(format!("dependency `{alias}`: `sha256` must be 64 hexadecimal digits"));
    }
    Ok(Source::Archive { url, sha256: sha256.map(|h| h.to_lowercase()) })
}

/// Resolves the package graph of the project in `dir`, or of the script
/// that `dir` is, the root first. What `tondo.lock`, or the script's lock,
/// pins is used as is; other dependencies are fetched and pinned, and the
/// lockfile is rewritten when that changes it. `update` names
/// dependencies to resolve again, all of them when empty.
pub fn resolve(dir: &Path, update: Option<&[String]>) -> Result<Vec<Package>, String> {
    let script = !dir.is_dir();
    let lock_path = if script { script_lock(dir) } else { dir.join("tondo.lock") };
    let mut pins = read_lock(&lock_path)?;
    // What this process resolved for a lock it could not write.
    if script && update.is_none() {
        let unwritten = UNWRITTEN.lock().unwrap_or_else(|e| e.into_inner());
        for (source, pin) in unwritten.get(&lock_path).into_iter().flatten() {
            pins.entry(source.clone()).or_insert_with(|| pin.clone());
        }
    }
    let mut resolver = Resolver {
        pins,
        update,
        used: BTreeMap::new(),
        packages: Vec::new(),
        origins: BTreeMap::new(),
        visiting: Vec::new(),
    };
    resolver.visit(dir, None)?;
    // What the program's dependencies do not reach only the tests use.
    let mut reached = vec![false; resolver.packages.len()];
    let mut pending = vec![0];
    while let Some(index) = pending.pop() {
        if !std::mem::replace(&mut reached[index], true) {
            pending.extend(resolver.packages[index].dependencies.iter().map(|&(_, d)| d));
        }
    }
    for (package, reached) in resolver.packages.iter_mut().zip(reached) {
        package.tests_only = !reached;
    }
    let text = write_lock(&resolver.used);
    let current = std::fs::read_to_string(&lock_path).ok();
    if current.as_deref() != Some(text.as_str()) && (current.is_some() || !resolver.used.is_empty()) {
        match std::fs::write(&lock_path, text) {
            Ok(()) => {}
            // A script runs where it was installed, which its user may not
            // be able to write: it goes on, and says so once.
            Err(e) if script => {
                let mut unwritten = UNWRITTEN.lock().unwrap_or_else(|e| e.into_inner());
                if unwritten.insert(lock_path.clone(), resolver.used.clone()).is_none() {
                    let reason = e.to_string();
                    let reason = reason.split(" (os error").next().unwrap_or_default();
                    eprintln!(
                        "warning: cannot write {} ({reason}): the dependencies were resolved for this run only; install the script with its lock beside it to fix them",
                        lock_path.display()
                    );
                }
            }
            Err(e) => return Err(format!("cannot write {}: {e}", lock_path.display())),
        }
    }
    // Packages that share a name stay distinct.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for package in &mut resolver.packages {
        let count = seen.entry(package.name.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            package.name = format!("{}@{count}", package.name);
        }
    }
    Ok(resolver.packages)
}

/// The pins resolved by this process for each script's lock that could
/// not be written, so that it resolves them once.
static UNWRITTEN: std::sync::Mutex<BTreeMap<PathBuf, BTreeMap<String, String>>> =
    std::sync::Mutex::new(BTreeMap::new());

struct Resolver<'a> {
    pins: BTreeMap<String, String>,
    update: Option<&'a [String]>,
    /// The pins of the dependencies reached, which the lockfile keeps.
    used: BTreeMap<String, String>,
    packages: Vec<Package>,
    /// Where each directory of a repository's commit came from.
    origins: BTreeMap<PathBuf, (String, String)>,
    /// The directories being visited, to report cycles.
    visiting: Vec<PathBuf>,
}

impl Resolver<'_> {
    fn visit(&mut self, dir: &Path, alias: Option<&str>) -> Result<usize, String> {
        let dir = std::fs::canonicalize(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        // The root may be a script, a package of its one file.
        let (dir, script) = match dir.is_file() {
            true => (dir.parent().map(Path::to_path_buf).unwrap_or_default(), Some(dir)),
            false => (dir, None),
        };
        if let Some(index) = self.packages.iter().position(|p| p.dir == dir && p.script == script) {
            if self.visiting.contains(&dir) {
                return Err(format!("packages depend on each other in a cycle through {}", dir.display()));
            }
            return Ok(index);
        }
        let manifest = match &script {
            Some(file) => script_manifest(file)?,
            None => manifest(&dir)?,
        };
        let name = match (manifest.name, &script) {
            (Some(name), _) => name,
            (None, Some(file)) => file.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
            (None, None) => {
                let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                if !is_identifier(&name) {
                    let what = alias.map_or("the project".to_string(), |a| format!("dependency `{a}`"));
                    return Err(format!(
                        "{what} in {} needs a package name: its directory name `{name}` is not an identifier; declare one in tondo.toml: [package] name = \"app\"",
                        dir.display()
                    ));
                }
                name
            }
        };
        let index = self.packages.len();
        self.packages.push(Package {
            name: name.clone(),
            version: manifest.version.unwrap_or_else(|| "0.0.0".into()),
            origin: self.origins.get(&dir).cloned(),
            source_url: manifest.source_url,
            dir: dir.clone(),
            script,
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            tests_only: false,
            libraries: manifest.libraries,
            providers: manifest.providers,
            generators: manifest.generators,
            systems: manifest.systems,
            architectures: manifest.architectures,
            cpu: manifest.cpu,
            logpoints: manifest.logpoints,
            regions: manifest.regions,
        });
        self.visiting.push(dir.clone());
        for (dependency, source) in manifest.dependencies {
            if dependency == name {
                return Err(format!("dependency `{dependency}` has the name of its package `{name}`; rename one"));
            }
            let fetched = self.fetch(&dir, &dependency, &source)?;
            let target = self.visit(&fetched, Some(&dependency))?;
            self.packages[index].dependencies.push((dependency, target));
        }
        // Only the project's tests have dev-dependencies.
        if alias.is_none() {
            for (dependency, source) in manifest.dev_dependencies {
                if dependency == name {
                    return Err(format!("dependency `{dependency}` has the name of its package `{name}`; rename one"));
                }
                let fetched = self.fetch(&dir, &dependency, &source)?;
                let target = self.visit(&fetched, Some(&dependency))?;
                self.packages[index].dev_dependencies.push((dependency, target));
            }
        }
        self.visiting.pop();
        Ok(index)
    }

    /// Whether the pin of a dependency is to be ignored.
    fn refreshing(&self, alias: &str) -> bool {
        self.update.is_some_and(|names| names.is_empty() || names.iter().any(|n| n == alias))
    }

    /// The directory of a dependency, fetched into the cache if needed.
    fn fetch(&mut self, base: &Path, alias: &str, source: &Source) -> Result<PathBuf, String> {
        let key = source.key();
        let pinned = if self.refreshing(alias) { None } else { self.pins.get(&key).cloned() };
        let dir = match source {
            Source::Path(path) => {
                let dir = base.join(path);
                if !dir.is_dir() {
                    return Err(format!("dependency `{alias}`: {} is not a directory", dir.display()));
                }
                return Ok(dir);
            }
            Source::Git { url, reference } => {
                let (commit, dir) = git_checkout(alias, url, reference, pinned.as_deref())
                    .map_err(|e| format!("dependency `{alias}` ({url}): {e}"))?;
                self.used.insert(key, commit.clone());
                self.origins.insert(std::fs::canonicalize(&dir).unwrap_or(dir.clone()), (url.clone(), commit));
                dir
            }
            Source::Archive { url, sha256 } => {
                let expected = pinned.as_deref().or(sha256.as_deref());
                let (hash, dir) =
                    archive(alias, url, expected).map_err(|e| format!("dependency `{alias}` ({url}): {e}"))?;
                if let Some(declared) = sha256
                    && *declared != hash
                {
                    return Err(format!(
                        "dependency `{alias}`: the archive has SHA-256 {hash}, but tondo.toml declares {declared}"
                    ));
                }
                self.used.insert(key, hash);
                dir
            }
        };
        Ok(dir)
    }
}

fn read_lock(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut pins = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(pins) };
    let invalid = |e: String| format!("{}: {e}; delete it to fetch the dependencies again", path.display());
    for table in toml::parse(&text).map_err(invalid)? {
        if table.name != "package" {
            continue;
        }
        let field = |name: &str| table.entries.iter().find(|(k, _, _)| k == name).and_then(|(_, v, _)| v.as_str());
        let (Some(source), Some(pin)) = (field("source"), field("commit").or(field("sha256"))) else {
            return Err(invalid(format!("line {}: a package needs `source` and `commit` or `sha256`", table.line)));
        };
        pins.insert(source.to_string(), pin.to_string());
    }
    Ok(pins)
}

fn write_lock(pins: &BTreeMap<String, String>) -> String {
    let mut text = String::from(
        "# Generated by tondo: the exact version of each fetched dependency.\n# Commit it; `tondo update` changes it.\nversion = 1\n",
    );
    for (source, pin) in pins {
        let kind = if source.starts_with("git+") { "commit" } else { "sha256" };
        text.push_str(&format!("\n[[package]]\nsource = {}\n{kind} = {}\n", toml::quote(source), toml::quote(pin)));
    }
    text
}

fn run(command: &mut Command) -> Result<String, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output =
        command.output().map_err(|e| format!("cannot run `{program}`: {e}; install it to fetch dependencies"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`{program}` failed: {}", stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn cache(kind: &str) -> Result<PathBuf, String> {
    let dir = crate::driver::cache_dir().join(kind);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// The commit of a repository and a directory with its files at that
/// commit; with `pinned`, that commit.
/// Says on stderr that a package is being downloaded (toolchain 6).
fn fetching(alias: &str, what: &str) {
    eprintln!("fetching {alias}: {what}");
}

fn git_checkout(
    alias: &str,
    url: &str,
    reference: &Reference,
    pinned: Option<&str>,
) -> Result<(String, PathBuf), String> {
    let deps = cache("deps")?;
    if let Some(commit) = pinned {
        let dir = deps.join(format!("git-{commit}"));
        if dir.is_dir() {
            return Ok((commit.to_string(), dir));
        }
    }
    let repository = cache("git")?.join(crate::driver::fingerprint(&[url.as_bytes()]));
    let what = match reference {
        Reference::Tag(tag) => format!("git {url}, tag {tag}"),
        Reference::Branch(branch) => format!("git {url}, branch {branch}"),
        Reference::Rev(rev) => format!("git {url}, rev {rev}"),
        Reference::Default => format!("git {url}"),
    };
    let cloned = !repository.is_dir();
    if cloned {
        fetching(alias, &what);
        let partial = repository.with_extension(format!("partial{}", std::process::id()));
        run(Command::new("git").args(["clone", "--quiet", "--bare", url]).arg(&partial))?;
        std::fs::rename(&partial, &repository).map_err(|e| e.to_string())?;
    }
    let git = |args: &[&str]| run(Command::new("git").arg("-C").arg(&repository).args(args));
    let known = |commit: &str| git(&["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_ok();
    if pinned.is_none_or(|commit| !known(commit)) {
        if !cloned {
            fetching(alias, &what);
        }
        git(&["fetch", "--quiet", "--force", "origin", "+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"])?;
    }
    let commit = match (pinned, reference) {
        (Some(commit), _) => commit.to_string(),
        (None, Reference::Tag(tag)) => git(&["rev-parse", &format!("refs/tags/{tag}^{{commit}}")])
            .map_err(|_| format!("the repository has no tag `{tag}`"))?,
        (None, Reference::Branch(branch)) => git(&["rev-parse", &format!("refs/heads/{branch}^{{commit}}")])
            .map_err(|_| format!("the repository has no branch `{branch}`"))?,
        (None, Reference::Rev(rev)) => git(&["rev-parse", &format!("{rev}^{{commit}}")])
            .map_err(|_| format!("the repository has no commit `{rev}`"))?,
        (None, Reference::Default) => git(&["rev-parse", "HEAD^{commit}"])?,
    };
    if !known(&commit) {
        return Err(format!("the repository has no commit {commit}; `tondo update` resolves it again"));
    }
    let dir = deps.join(format!("git-{commit}"));
    if !dir.is_dir() {
        let partial = deps.join(format!(".partial-{}", std::process::id()));
        std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
        let mut archive = Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["archive", "--format=tar", &commit])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run `git`: {e}"))?;
        let stdout = archive.stdout.take().expect("piped");
        let extracted = Command::new("tar").arg("-x").arg("-C").arg(&partial).stdin(stdout).status();
        let archived = archive.wait().map_err(|e| e.to_string())?;
        if !archived.success() || !extracted.is_ok_and(|s| s.success()) {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(format!("cannot extract commit {commit}"));
        }
        std::fs::rename(&partial, &dir).map_err(|e| e.to_string())?;
    }
    Ok((commit, dir))
}

/// The SHA-256 of an archive and the directory of its contents; with
/// `expected`, a cached copy is used and a download must match it.
fn archive(alias: &str, url: &str, expected: Option<&str>) -> Result<(String, PathBuf), String> {
    let deps = cache("deps")?;
    if let Some(hash) = expected {
        let dir = deps.join(format!("sha256-{hash}"));
        if dir.is_dir() {
            return Ok((hash.to_string(), dir));
        }
    }
    fetching(alias, url);
    let partial = deps.join(format!(".partial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
    let name = url.rsplit('/').next().unwrap_or("archive").split(['?', '#']).next().unwrap_or("archive").to_string();
    let file = partial.join(&name);
    run(Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location", "--output"])
        .arg(&file)
        .arg(url))?;
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    let hash = crate::hash::sha256_hex(&bytes);
    if let Some(expected) = expected
        && expected != hash
    {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(format!("the archive has SHA-256 {hash}, but {expected} was expected"));
    }
    let contents = partial.join("contents");
    std::fs::create_dir_all(&contents).map_err(|e| e.to_string())?;
    if name.ends_with(".zip") {
        run(Command::new("unzip").arg("-q").arg(&file).arg("-d").arg(&contents))?;
    } else {
        run(Command::new("tar").arg("-xf").arg(&file).arg("-C").arg(&contents))?;
    }
    // An archive usually holds one top directory, such as `zlib-1.0/`.
    let entries: Vec<PathBuf> =
        std::fs::read_dir(&contents).map_err(|e| e.to_string())?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    let root = match entries.as_slice() {
        [single] if single.is_dir() => single.clone(),
        _ => contents.clone(),
    };
    let dir = deps.join(format!("sha256-{hash}"));
    if !dir.is_dir() {
        std::fs::rename(&root, &dir).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(&partial);
    Ok((hash, dir))
}
