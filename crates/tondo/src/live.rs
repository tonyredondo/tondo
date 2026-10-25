//! Live builds (toolchain 7.10): debug builds whose code changes while
//! they run. Every call of one of the program's functions goes through a
//! slot of a table, and the image keeps tables of what a patch may use of
//! it: the runtime's functions, the functions of C it calls and the
//! identity of each type. A patch is the program compiled again; the
//! runtime maps it and points the slots at its functions, and the old
//! code stays for the frames still in it.

use std::collections::HashMap;
use std::path::PathBuf;

/// What codegen needs of a live build, and what it tells of it.
pub struct Build {
    /// The build's identity, which its program reports.
    pub id: String,
    /// The slot of each function by its identity: its name, its
    /// parameters and its result.
    pub slots: HashMap<String, u32>,
    pub capacity: u32,
    /// The runtime's functions, in the order of the image's table.
    pub runtime: Vec<String>,
    /// The functions of C the image calls, and the types whose identity it
    /// holds, in the order of their tables.
    pub c: Vec<String>,
    pub data: Vec<String>,
    /// A hash of each function's code, to tell what a patch changes.
    pub code: HashMap<String, u64>,
    /// The debugger's sites of the build.
    pub sites: u32,
    /// For a patch: what the running program has.
    pub patch: Option<Base>,
}

/// What a patch builds on: the types whose identity the running program
/// holds, and the number of its debugger's sites.
pub struct Base {
    pub data: Vec<String>,
    pub sites: u32,
}

impl Build {
    pub fn new(runtime: Vec<String>) -> Build {
        Build {
            id: build_id(),
            slots: HashMap::new(),
            capacity: 0,
            runtime,
            c: Vec::new(),
            data: Vec::new(),
            code: HashMap::new(),
            sites: 0,
            patch: None,
        }
    }
}

/// A new build's identity: when and where it was made.
fn build_id() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    crate::driver::fingerprint(&[&now.as_nanos().to_le_bytes(), &std::process::id().to_le_bytes()])
}

/// Room for the functions a patch adds.
pub fn capacity(functions: usize) -> u32 {
    (functions * 2 + 4096) as u32
}

/// Where the manifest of a build is kept.
pub fn manifest_path(id: &str) -> PathBuf {
    crate::driver::cache_dir().join("live").join(format!("{id}.json"))
}

/// How a live program was built, which a patch builds it again with.
pub struct Options {
    pub statically: bool,
    pub target: Option<crate::target::Target>,
    pub libraries: Vec<String>,
}

/// The object of a live build of the program at `path`, whose manifest it
/// keeps for the patches.
pub fn compile(
    analysis: &crate::driver::Analysis,
    path: &std::path::Path,
    options: &Options,
) -> Result<Vec<u8>, String> {
    let mut build = Build::new(crate::driver::runtime_functions(options.statically, options.target)?);
    let object = crate::driver::compile(analysis, true, options.target, Some(&mut build))?;
    let types = &analysis.program.as_ref().expect("a checked program").types;
    save(&build, &shapes(types), path, options, None)?;
    Ok(object)
}

/// What each nominal type is made of: its fields, its variants, or what a
/// newtype wraps. A patch may not change it, since values of the old one
/// are alive.
fn shapes(types: &crate::types::Types) -> HashMap<String, String> {
    use crate::types::{AdtId, AdtKind, Payload};
    let fields = |fields: &[crate::types::Field]| {
        let fields: Vec<String> = fields.iter().map(|f| format!("{}: {}", f.name, types.display(f.ty))).collect();
        format!("{{ {} }}", fields.join(", "))
    };
    let mut shapes = HashMap::new();
    for index in 0..types.adt_count() {
        let adt = types.adt(AdtId(index as u32));
        let name = match &adt.instance {
            Some((_, args)) if !adt.name.contains('[') => {
                let args: Vec<String> = args.iter().map(|&arg| types.display(arg)).collect();
                format!("{}[{}]", adt.name, args.join(", "))
            }
            _ => adt.name.clone(),
        };
        let shape = match &adt.kind {
            AdtKind::Record(record) => fields(record),
            AdtKind::Newtype(inner) => format!("= {}", types.display(*inner)),
            AdtKind::Enum(variants) => {
                let variants: Vec<String> = variants
                    .iter()
                    .map(|v| match &v.payload {
                        Payload::Unit => v.name.clone(),
                        Payload::Tuple(items) => {
                            let items: Vec<String> = items.iter().map(|&t| types.display(t)).collect();
                            format!("{}({})", v.name, items.join(", "))
                        }
                        Payload::Record(record) => format!("{} {}", v.name, fields(record)),
                    })
                    .collect();
                variants.join(" | ")
            }
        };
        shapes.insert(name, shape);
    }
    shapes
}

/// Writes a build's manifest: the build's own, or one process's after
/// its patches.
fn save(
    build: &Build,
    shapes: &HashMap<String, String>,
    path: &std::path::Path,
    options: &Options,
    pid: Option<u32>,
) -> Result<(), String> {
    // The path as it was given, and where from: the places in its code
    // name its files so, and a patch's must name them the same.
    let manifest = serde_json::json!({
        "build": build.id,
        "dir": std::env::current_dir().map_err(|e| e.to_string())?,
        "path": path,
        "static": options.statically,
        "target": options.target.map(crate::target::Target::name),
        "libraries": options.libraries,
        "capacity": build.capacity,
        "slots": build.slots,
        "runtime": build.runtime,
        "c": build.c,
        "data": build.data,
        "code": build.code,
        "sites": build.sites,
        "types": shapes,
    });
    let file = match pid {
        Some(pid) => manifest_path(&format!("{}-{pid}", build.id)),
        None => manifest_path(&build.id),
    };
    std::fs::create_dir_all(file.parent().expect("a directory")).map_err(|e| e.to_string())?;
    std::fs::write(&file, manifest.to_string()).map_err(|e| format!("cannot write {}: {e}", file.display()))
}

/// What a reload did: the functions whose code changed and those it added.
pub struct Reloaded {
    pub changed: Vec<String>,
    pub added: Vec<String>,
}

/// Why a reload did not happen: the program does not compile, or the
/// change needs it started again.
pub enum Refused {
    Diagnostics(String),
    Restart(String),
    Failed(String),
}

impl From<String> for Refused {
    fn from(message: String) -> Refused {
        Refused::Failed(message)
    }
}

/// Builds the program a live process runs again from its sources, and
/// patches the process `pid` with it; `ask` sends the process a request
/// and gives its answer.
pub fn reload(pid: u32, ask: &mut dyn FnMut(&str) -> Result<serde_json::Value, String>) -> Result<Reloaded, Refused> {
    let live = ask("live")?;
    if let Some(error) = live["error"].as_str() {
        return Err(Refused::Failed(error.to_string()));
    }
    let id = live["build"].as_str().unwrap_or_default().to_string();
    let file = [manifest_path(&format!("{id}-{pid}")), manifest_path(&id)].into_iter().find(|f| f.exists());
    let file = file.ok_or_else(|| format!("tondo has no manifest of the build {id}: build it again with --live"))?;
    let text = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let manifest: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let strings = |key: &str| -> Vec<String> {
        manifest[key].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect()
    };
    let path = std::path::PathBuf::from(manifest["path"].as_str().unwrap_or_default());
    let options = Options {
        statically: manifest["static"].as_bool().unwrap_or(false),
        target: manifest["target"].as_str().and_then(|t| crate::target::Target::parse(t).ok()),
        libraries: strings("libraries"),
    };
    let dir = std::path::PathBuf::from(manifest["dir"].as_str().unwrap_or("."));
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    std::env::set_current_dir(&dir).map_err(|e| format!("cannot go to {}: {e}", dir.display()))?;
    let reloaded = patch(pid, ask, &manifest, &path, options, &live, id);
    let _ = std::env::set_current_dir(here);
    reloaded
}

/// The rest of a reload, from the directory the program was built in.
fn patch(
    pid: u32,
    ask: &mut dyn FnMut(&str) -> Result<serde_json::Value, String>,
    manifest: &serde_json::Value,
    path: &std::path::Path,
    options: Options,
    live: &serde_json::Value,
    id: String,
) -> Result<Reloaded, Refused> {
    let strings = |key: &str| -> Vec<String> {
        manifest[key].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect()
    };
    let analysis = crate::driver::analyze(path)?;
    if analysis.program.is_none() {
        return Err(Refused::Diagnostics(analysis.diags.render(&analysis.sources)));
    }
    let types = &analysis.program.as_ref().expect("a checked program").types;
    let shapes = shapes(types);
    let mut reshaped: Vec<String> = manifest["types"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, shape)| shapes.get(*name).is_some_and(|now| Some(now.as_str()) != shape.as_str()))
        .map(|(name, _)| format!("`{name}`"))
        .collect();
    if !reshaped.is_empty() {
        reshaped.sort();
        return Err(Refused::Restart(format!(
            "{} changed what {} made of, and values of the old shape are alive: start the program again",
            reshaped.join(", "),
            if reshaped.len() == 1 { "it is" } else { "they are" }
        )));
    }
    let known: HashMap<String, u64> = serde_json::from_value(manifest["code"].clone()).unwrap_or_default();
    let (runtime, c, data) = (strings("runtime"), strings("c"), strings("data"));
    let mut build = Build {
        id: id.clone(),
        slots: serde_json::from_value(manifest["slots"].clone()).unwrap_or_default(),
        capacity: manifest["capacity"].as_u64().unwrap_or(0) as u32,
        runtime: runtime.clone(),
        c: c.clone(),
        data: data.clone(),
        code: HashMap::new(),
        sites: 0,
        patch: Some(Base { data: data.clone(), sites: live["sites"].as_u64().unwrap_or(0) as u32 }),
    };
    let object = crate::driver::compile(&analysis, true, options.target, Some(&mut build)).map_err(Refused::Failed)?;
    use crate::link::patch::External;
    let resolve = |name: &str| -> Result<External, String> {
        let at = |list: &[String], table: u32| {
            list.iter().position(|n| n == name).map(|i| External { table, index: i as u32 })
        };
        if name == "tondo_live_slots" {
            return Ok(External { table: 0, index: 0 });
        }
        if let Some(index) = name.strip_prefix("tondo_live_data").and_then(|i| i.parse().ok()) {
            return Ok(External { table: 3, index });
        }
        at(&runtime, 1).or_else(|| at(&c, 2)).ok_or_else(|| {
            format!("the change calls `{name}`, which the running program does not link: start it again")
        })
    };
    let patch = crate::link::patch::link(&object, &resolve, "tondo_patch").map_err(Refused::Restart)?;
    let patch_file = crate::driver::cache_dir().join("live").join(format!("patch-{pid}.bin"));
    std::fs::write(&patch_file, &patch).map_err(|e| format!("cannot write {}: {e}", patch_file.display()))?;
    let answer = ask(&format!("patch {}", patch_file.display()));
    let _ = std::fs::remove_file(&patch_file);
    let answer = answer?;
    if let Some(error) = answer["error"].as_str() {
        return Err(Refused::Restart(error.to_string()));
    }
    let mut changed = Vec::new();
    let mut added = Vec::new();
    for (identity, hash) in &build.code {
        let name = identity.split('(').next().unwrap_or(identity).to_string();
        match known.get(identity) {
            Some(old) if old == hash => {}
            Some(_) => changed.push(name),
            None => added.push(name),
        }
    }
    let mut code = known;
    code.extend(build.code.drain());
    build.code = code;
    save(&build, &shapes, path, &options, Some(pid))?;
    changed.sort();
    changed.dedup();
    added.sort();
    added.dedup();
    Ok(Reloaded { changed, added })
}

/// `tondo reload <pid|name>`: the running live program `name`.
pub fn reload_named(name: &str) -> Result<Reloaded, Refused> {
    let mut program = crate::observe::connect(name)?;
    let pid = program.pid;
    reload(pid, &mut |request| program.ask(request))
}

impl Refused {
    /// The refusal as one message.
    pub fn message(&self) -> String {
        match self {
            Refused::Diagnostics(text) => format!("{}\nnot reloaded: the program goes on as it was", text.trim_end()),
            Refused::Restart(message) | Refused::Failed(message) => format!("not reloaded: {message}"),
        }
    }
}

impl Reloaded {
    /// What a reload says it did.
    pub fn summary(&self) -> String {
        let list = |names: &[String]| names.join(", ");
        match (self.changed.is_empty(), self.added.is_empty()) {
            (true, true) => "reloaded: no function changed".into(),
            (false, true) => format!("reloaded: {} changed ({})", self.changed.len(), list(&self.changed)),
            (true, false) => format!("reloaded: {} added ({})", self.added.len(), list(&self.added)),
            (false, false) => format!(
                "reloaded: {} changed ({}), {} added ({})",
                self.changed.len(),
                list(&self.changed),
                self.added.len(),
                list(&self.added)
            ),
        }
    }
}
