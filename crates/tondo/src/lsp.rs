//! `tondo lsp`: a language server (LSP 3.17) over standard input and
//! output. A project is analyzed as its test build, so test files are too,
//! with the text of the files the editor has open, again after changes.

use crate::driver::{self, Analysis};
use crate::index::{DefKind, Index};
use crate::lexer::{Kw, Tok};
use crate::source::{SourceFile, Span};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// Serves one editor until it says `exit`.
pub fn serve() -> Result<(), String> {
    let (send, messages) = mpsc::channel();
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        while let Some(message) = read_message(&mut input) {
            if send.send(message).is_err() {
                break;
            }
        }
    });
    let mut server = Server::default();
    loop {
        // A burst of changes is analyzed once, when the typing pauses.
        let message = if server.dirty.is_empty() {
            messages.recv().ok()
        } else {
            match messages.recv_timeout(Duration::from_millis(150)) {
                Ok(message) => Some(message),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    server.refresh();
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => None,
            }
        };
        let Some(message) = message else { return Ok(()) };
        if !server.handle(message) {
            return Ok(());
        }
    }
}

fn read_message(input: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    input.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn send(message: Value) {
    let body = message.to_string();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

/// A file the editor has open.
struct Document {
    uri: String,
    text: String,
}

/// The last analysis of a project, or of a file on its own, with the path
/// of each of its source files that has one.
struct Analyzed {
    analysis: Analysis,
    paths: Vec<Option<PathBuf>>,
    /// The tests of a project, when it checked.
    tests: Option<Vec<crate::hir::TestNodeInfo>>,
}

/// The next segment of the modules an import can name after what it
/// has written: `std`, the project's and its dependencies' first; then,
/// after `std.`, the standard modules, each with what it is for.
fn importable(analyzed: &Analyzed, here: &str, written: &str) -> Vec<Value> {
    if written.contains(' ') {
        return Vec::new();
    }
    let prefix = written.rfind('.').map_or("", |at| &written[..=at]);
    let mut modules: Vec<(String, Option<String>, &str)> = crate::api::standard_modules()
        .into_iter()
        .filter(|m| m.starts_with("std."))
        .map(|m| {
            let about = crate::api::summary(&m);
            (m, about, "the standard library")
        })
        .collect();
    let sources = &analyzed.analysis.sources;
    for file in (0..sources.len()).map(|i| sources.file(i)) {
        let module = &file.module;
        if file.path.as_os_str().is_empty() || module.starts_with("std") || module == here || module == "prelude" {
            continue;
        }
        let whose = if file.dependency { "a dependency" } else { "the project" };
        if (file.own || file.dependency) && !modules.iter().any(|(m, ..)| m == module) {
            modules.push((module.clone(), None, whose));
        }
    }
    let mut items: Vec<Value> = Vec::new();
    for (module, about, whose) in &modules {
        let Some(rest) = module.strip_prefix(prefix) else { continue };
        let (segment, deeper) = rest.split_once('.').map_or((rest, false), |(segment, _)| (segment, true));
        if segment.is_empty() || items.iter().any(|item| item["label"] == segment) {
            continue;
        }
        let mut item = json!({ "label": segment, "kind": 9, "detail": format!("{prefix}{segment}") });
        match (deeper, about) {
            (true, _) => item["labelDetails"] = json!({ "description": whose }),
            (false, Some(about)) => {
                let short = about.split(". ").next().unwrap_or(about).trim_end_matches('.');
                item["labelDetails"] = json!({ "description": short });
                item["documentation"] = json!({ "kind": "markdown", "value": about });
            }
            (false, None) => item["labelDetails"] = json!({ "description": format!("a module of {whose}") }),
        }
        items.push(item);
    }
    items
}

/// What the columns of positions count.
#[derive(Default, Clone, Copy, PartialEq)]
enum Units {
    #[default]
    Utf16,
    Utf8,
    /// Characters, as diagnostics count them.
    Chars,
}

impl Units {
    fn count(self, text: &str) -> usize {
        match self {
            Units::Utf16 => text.encode_utf16().count(),
            Units::Utf8 => text.len(),
            Units::Chars => text.chars().count(),
        }
    }
}

#[derive(Default)]
struct Server {
    units: Units,
    /// Sends nothing on its own, as no editor shows diagnostics.
    quiet: bool,
    open: HashMap<PathBuf, Document>,
    analyses: HashMap<PathBuf, Analyzed>,
    /// The analyses before the last of each root, newest first: while a
    /// line is being written it does not parse, and what came before the
    /// cursor is found in them.
    earlier: HashMap<PathBuf, Vec<Analyzed>>,
    /// Roots whose files changed since their analysis.
    dirty: HashSet<PathBuf>,
    /// The files of each root that the editor shows diagnostics for.
    published: HashMap<PathBuf, HashSet<PathBuf>>,
    /// Where the standard modules are written, for definitions in them.
    std: Option<PathBuf>,
    /// Whether the editor asked for the tests of each project
    /// (`tondo/tests`), and the last it was sent of each.
    tests: bool,
    /// The editor's folders, whose projects are analyzed when it starts.
    folders: Vec<PathBuf>,
    listed: HashMap<PathBuf, Value>,
    shut: bool,
}

/// The language server's answers outside an editor, as `tondo mcp` asks
/// them: columns count characters, and files are as they are on disk.
pub struct Language(Server);

impl Default for Language {
    fn default() -> Language {
        Language(Server { units: Units::Chars, quiet: true, ..Server::default() })
    }
}

impl Language {
    /// The answer to a request of the protocol, such as
    /// `textDocument/definition`.
    pub fn ask(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        match self.0.answer(method, params, true) {
            Some(Ok(value)) => Ok(value),
            Some(Err((_, message))) => Err(message),
            None => Ok(Value::Null),
        }
    }
}

impl Server {
    /// Handles a message; false once the editor says `exit`.
    fn handle(&mut self, message: Value) -> bool {
        let method = message["method"].as_str().unwrap_or_default().to_string();
        let params = &message["params"];
        let id = message.get("id").cloned();
        if method == "exit" {
            return false;
        }
        let result = self.answer(&method, params, id.is_some());
        if let (Some(id), Some(result)) = (id, result) {
            send(match result {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err((code, message)) => {
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
                }
            });
        }
        true
    }

    /// What a message asks for: none for a notification.
    fn answer(&mut self, method: &str, params: &Value, request: bool) -> Option<Result<Value, (i64, String)>> {
        match method {
            "initialize" => Some(Ok(self.initialize(params))),
            "shutdown" => {
                self.shut = true;
                Some(Ok(Value::Null))
            }
            "textDocument/didOpen" => {
                let document = &params["textDocument"];
                self.changed(document["uri"].as_str(), document["text"].as_str());
                None
            }
            "textDocument/didChange" => {
                let text = params["contentChanges"].as_array().and_then(|c| c.last()).map(|c| &c["text"]);
                self.changed(params["textDocument"]["uri"].as_str(), text.and_then(Value::as_str));
                None
            }
            "textDocument/didClose" => {
                if let Some(path) = params["textDocument"]["uri"].as_str().and_then(uri_path) {
                    self.open.remove(&path);
                    self.dirty.insert(root_of(&path).0);
                }
                None
            }
            "textDocument/hover" => Some(Ok(self.hover(params).unwrap_or(Value::Null))),
            "textDocument/definition" => Some(Ok(self.definition(params).unwrap_or(Value::Null))),
            "textDocument/references" => Some(Ok(self.references(params).unwrap_or(json!([])))),
            "textDocument/prepareRename" => Some(Ok(self.prepare_rename(params).unwrap_or(Value::Null))),
            "textDocument/rename" => Some(self.rename(params)),
            "textDocument/documentSymbol" => Some(Ok(self.symbols(params).unwrap_or(json!([])))),
            "textDocument/formatting" => Some(Ok(self.formatting(params).unwrap_or(Value::Null))),
            "textDocument/completion" => Some(Ok(self.completion(params).unwrap_or(json!([])))),
            "textDocument/codeAction" => Some(Ok(self.code_actions(params).unwrap_or(json!([])))),
            "textDocument/documentHighlight" => Some(Ok(self.highlights(params).unwrap_or(json!([])))),
            "workspace/symbol" => Some(Ok(self.workspace_symbols(params))),
            "textDocument/signatureHelp" => Some(Ok(self.signature_help(params).unwrap_or(Value::Null))),
            "textDocument/inlayHint" => Some(Ok(self.inlay_hints(params).unwrap_or(json!([])))),
            "textDocument/codeLens" => Some(Ok(self.lenses(params).unwrap_or(json!([])))),
            "textDocument/foldingRange" => Some(Ok(self.folding_ranges(params).unwrap_or(json!([])))),
            "textDocument/selectionRange" => Some(Ok(self.selection_ranges(params).unwrap_or(json!([])))),
            "textDocument/rangeFormatting" => Some(Ok(self.range_formatting(params).unwrap_or(json!([])))),
            "textDocument/implementation" => Some(Ok(self.implementation(params).unwrap_or(Value::Null))),
            "textDocument/typeDefinition" => Some(Ok(self.type_definition(params).unwrap_or(Value::Null))),
            "textDocument/prepareCallHierarchy" => {
                Some(Ok(self.prepare_hierarchy(params, &[DefKind::Function, DefKind::Method]).unwrap_or(Value::Null)))
            }
            "callHierarchy/incomingCalls" => Some(Ok(self.incoming_calls(params).unwrap_or(json!([])))),
            "callHierarchy/outgoingCalls" => Some(Ok(self.outgoing_calls(params).unwrap_or(json!([])))),
            "textDocument/prepareTypeHierarchy" => {
                let kinds = [DefKind::Type, DefKind::Enum, DefKind::Trait];
                Some(Ok(self.prepare_hierarchy(params, &kinds).unwrap_or(Value::Null)))
            }
            "typeHierarchy/supertypes" => Some(Ok(self.type_hierarchy(params, true).unwrap_or(json!([])))),
            "typeHierarchy/subtypes" => Some(Ok(self.type_hierarchy(params, false).unwrap_or(json!([])))),
            "workspace/willRenameFiles" => Some(Ok(self.will_rename_files(params).unwrap_or(Value::Null))),
            "initialized" => {
                self.analyze_workspace();
                None
            }
            "textDocument/semanticTokens/full" => {
                Some(Ok(self.semantic_tokens(params).unwrap_or(json!({ "data": [] }))))
            }
            _ if request => Some(Err((-32601, format!("`{method}` is not supported")))),
            _ => None,
        }
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let encodings = &params["capabilities"]["general"]["positionEncodings"];
        let utf8 = encodings.as_array().is_some_and(|e| e.iter().any(|e| e == "utf-8"));
        self.units = if utf8 { Units::Utf8 } else { Units::Utf16 };
        self.tests = params["initializationOptions"]["tests"] == true;
        let folders = params["workspaceFolders"].as_array().into_iter().flatten().filter_map(|f| f["uri"].as_str());
        self.folders = folders.chain(params["rootUri"].as_str()).filter_map(uri_path).collect();
        self.folders.dedup();
        json!({
            "capabilities": {
                "positionEncoding": if utf8 { "utf-8" } else { "utf-16" },
                "textDocumentSync": { "openClose": true, "change": 1 },
                "hoverProvider": true,
                "definitionProvider": true,
                "referencesProvider": true,
                "renameProvider": { "prepareProvider": true },
                "documentSymbolProvider": true,
                "documentFormattingProvider": true,
                "completionProvider": { "triggerCharacters": ["."] },
                "codeActionProvider": { "codeActionKinds": ["quickfix", "source.fixAll", "source.organizeImports"] },
                "documentHighlightProvider": true,
                "workspaceSymbolProvider": true,
                "signatureHelpProvider": { "triggerCharacters": ["(", ","] },
                "inlayHintProvider": true,
                "codeLensProvider": { "resolveProvider": false },
                "foldingRangeProvider": true,
                "selectionRangeProvider": true,
                "documentRangeFormattingProvider": true,
                "callHierarchyProvider": true,
                "implementationProvider": true,
                "typeDefinitionProvider": true,
                "typeHierarchyProvider": true,
                "workspace": {
                    "fileOperations": {
                        "willRename": { "filters": [{ "scheme": "file", "pattern": { "glob": "**", "matches": "folder" } }] },
                    },
                },
                "semanticTokensProvider": {
                    "legend": { "tokenTypes": TOKEN_TYPES, "tokenModifiers": ["declaration", "readonly", "defaultLibrary"] },
                    "full": true,
                },
            },
            "serverInfo": { "name": "tondo", "version": driver::VERSION },
        })
    }

    fn changed(&mut self, uri: Option<&str>, text: Option<&str>) {
        let (Some(uri), Some(text)) = (uri, text) else { return };
        let Some(path) = uri_path(uri) else { return };
        self.dirty.insert(root_of(&path).0);
        self.open.insert(path, Document { uri: uri.to_string(), text: text.to_string() });
    }

    /// Analyzes again what changed, and shows its diagnostics.
    fn refresh(&mut self) {
        for root in std::mem::take(&mut self.dirty) {
            self.analyze(&root);
        }
    }

    /// Analyzes the root of the file a request is about again, if it
    /// changed since.
    fn prepare(&mut self, params: &Value) -> Option<()> {
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        let (root, _) = root_of(&path);
        if self.dirty.remove(&root) || !self.analyses.contains_key(&key_of(&path)) {
            self.analyze(&root);
        }
        Some(())
    }

    fn analyze(&mut self, root: &Path) {
        let open: HashMap<PathBuf, String> =
            self.open.iter().map(|(path, document)| (path.clone(), document.text.clone())).collect();
        // A project with files of a system is read for each one it runs on,
        // this machine's first: the errors of each go with the first's, and
        // the files of another system are read with that system's analysis.
        let host = crate::project::System::host();
        let systems = driver::checked_systems(root).unwrap_or_else(|_| vec![host]);
        let main = systems.first().copied().unwrap_or(host);
        let Some(mut analyzed) = driver::for_system(main, || self.analyzed(root, open.clone())) else { return };
        let mut others = Vec::new();
        for &system in systems.iter().filter(|&&system| system != host) {
            let Some(other) = driver::for_system(system, || self.analyzed(root, open.clone())) else { continue };
            if system != main {
                driver::merge_system(&mut analyzed.analysis, &other.analysis, system);
            }
            others.push((system, other));
        }
        analyzed.paths = self.paths(&analyzed.analysis);
        self.publish(root, &analyzed);
        self.publish_tests(root, &analyzed, &others);
        for (system, other) in others {
            self.analyses.insert(root.join(format!("#{}", system.name())), other);
        }
        if let Some(previous) = self.analyses.insert(root.to_path_buf(), analyzed) {
            let earlier = self.earlier.entry(root.to_path_buf()).or_default();
            earlier.insert(0, previous);
            earlier.truncate(3);
        }
    }

    /// An analysis of a root with the given text of open files.
    fn analyzed(&mut self, root: &Path, open: HashMap<PathBuf, String>) -> Option<Analyzed> {
        let mut analysis = driver::analyze_for_editor(root, root.is_dir(), open).ok()?;
        let tests = analysis.program.take().map(|program| program.tests).filter(|_| root.is_dir());
        let paths = self.paths(&analysis);
        Some(Analyzed { analysis, paths, tests })
    }

    /// The file on disk of each source of an analysis.
    fn paths(&mut self, analysis: &Analysis) -> Vec<Option<PathBuf>> {
        let std = self.std_dir();
        (0..analysis.sources.len())
            .map(|index| {
                let file = analysis.sources.file(index);
                match file.logical.strip_prefix("<std>/") {
                    Some(name) => std.as_ref().map(|dir| dir.join(name)),
                    None if file.path.is_absolute() => file.path.canonicalize().ok(),
                    None => None,
                }
            })
            .collect()
    }

    /// Sends a project's suites and tests when they change, as the editor
    /// asked: each with its id, its parent's, its name, where it is, and
    /// whether it is a benchmark.
    fn publish_tests(&mut self, root: &Path, analyzed: &Analyzed, others: &[(crate::project::System, Analyzed)]) {
        let Some(nodes) = analyzed.tests.as_ref().filter(|_| self.tests && !self.quiet) else { return };
        let mut tests = Vec::new();
        let mut ids = HashSet::new();
        // Those of another system's files go too, with the system, which
        // this machine does not run (language 6.8).
        let others = others.iter().filter_map(|(system, other)| Some((Some(*system), other, other.tests.as_ref()?)));
        for (system, from, nodes) in std::iter::once((None, analyzed, nodes)).chain(others) {
            for node in nodes.iter().skip(1) {
                if !ids.insert(node.id.clone()) {
                    continue;
                }
                let Some(location) = self.location(from, node.span) else { continue };
                let parent = nodes.get(node.parent as usize).filter(|_| node.parent != 0).map(|p| p.id.clone());
                let name = node.id.rsplit("::").next().unwrap_or(&node.id);
                let mut test = json!({ "id": node.id, "parent": parent, "name": name, "suite": node.suite,
                    "bench": node.bench, "fuzz": node.fuzz, "location": location });
                if let Some(system) = system {
                    test["system"] = json!(system.shown());
                }
                tests.push(test);
            }
        }
        let tests = Value::Array(tests);
        if self.listed.get(root) != Some(&tests) {
            self.listed.insert(root.to_path_buf(), tests.clone());
            send(json!({ "jsonrpc": "2.0", "method": "tondo/tests",
                "params": { "root": self.uri(root), "tests": tests } }));
        }
    }

    /// Run, Debug and Debug Live over a program's `main`, or over the first
    /// statement of a script, in the file that holds it.
    fn lenses(&self, params: &Value) -> Option<Value> {
        let uri = params["textDocument"]["uri"].as_str()?;
        let path = uri_path(uri)?;
        let (root, project) = root_of(&path);
        let holds = !project || [root.join("src/main.to"), root.join("main.to")].contains(&path);
        if !holds {
            return Some(json!([]));
        }
        let text = match self.open.get(&path) {
            Some(document) => document.text.clone(),
            None => std::fs::read_to_string(&path).ok()?,
        };
        let mut sources = crate::source::SourceMap::default();
        let index = sources.add(&path, text);
        let file = sources.file(index);
        let module = crate::parser::parse_file(file, &mut crate::diag::Diagnostics::default());
        let main = module.items.iter().find_map(|item| match item {
            crate::ast::Item::Fn(f) if f.name.name == "main" && f.owner.is_none() => Some(f.name.span),
            _ => None,
        });
        let Some(span) = main.or(module.statements.first().map(|s| s.span)) else { return Some(json!([])) };
        let range = self.range(file, span);
        let commands = [
            ("Run", "tondo.run"),
            ("Debug", "tondo.debug"),
            ("Debug Live", "tondo.debugLive"),
            ("Profile", "tondo.profile"),
        ];
        let lenses: Vec<Value> = commands
            .iter()
            .map(|(title, command)| json!({ "range": range, "command": { "title": title, "command": command, "arguments": [uri] } }))
            .collect();
        Some(Value::Array(lenses))
    }

    /// Sends the diagnostics of each file of a root, and clears those of
    /// files that have none now.
    fn publish(&mut self, root: &Path, analyzed: &Analyzed) {
        if self.quiet {
            return;
        }
        let mut by_file: HashMap<PathBuf, Vec<Value>> = HashMap::new();
        for diagnostic in &analyzed.analysis.diags.list {
            let Some(span) = diagnostic.span else { continue };
            let Some((path, range)) = self.place(analyzed, span) else { continue };
            if path.starts_with(self.std.as_deref().unwrap_or(Path::new("/nonexistent"))) {
                continue;
            }
            let severity = match diagnostic.severity {
                crate::diag::Severity::Error => 1,
                crate::diag::Severity::Warning => 2,
            };
            let code = diagnostic.code.unwrap_or("E0000");
            let mut published = json!({
                "range": range,
                "severity": severity,
                "code": code,
                "source": "tondo",
                "message": diagnostic.message,
            });
            // The code opens what it means (`tondo explain`).
            if crate::explain::CODES.iter().any(|(c, ..)| *c == code) {
                published["codeDescription"] = json!({ "href": format!("tondo-explain:{code}") });
            }
            by_file.entry(path).or_default().push(published);
        }
        let before = self.published.remove(root).unwrap_or_default();
        for path in before.iter().filter(|path| !by_file.contains_key(*path)) {
            let uri = self.uri(path);
            send(json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                "params": { "uri": uri, "diagnostics": [] } }));
        }
        for (path, diagnostics) in &by_file {
            let uri = self.uri(path);
            send(json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                "params": { "uri": uri, "diagnostics": diagnostics } }));
        }
        self.published.insert(root.to_path_buf(), by_file.into_keys().collect());
    }

    /// Writes the standard modules where an editor can open them, once.
    fn std_dir(&mut self) -> Option<PathBuf> {
        if self.std.is_none() {
            let dir = driver::cache_dir().join(format!("std-{}", driver::VERSION));
            if std::fs::create_dir_all(&dir).is_ok() {
                for (name, text) in crate::stdlib::sources() {
                    let path = dir.join(format!("{name}.to"));
                    if std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
                        let _ = std::fs::write(&path, text);
                    }
                }
                self.std = dir.canonicalize().ok();
            }
        }
        self.std.clone()
    }

    fn uri(&self, path: &Path) -> String {
        self.open.get(path).map(|document| document.uri.clone()).unwrap_or_else(|| file_uri(path))
    }

    /// The file and range of a span.
    fn place(&self, analyzed: &Analyzed, span: Span) -> Option<(PathBuf, Value)> {
        let sources = &analyzed.analysis.sources;
        let index = (0..sources.len()).rev().find(|&i| sources.file(i).base <= span.start)?;
        let path = analyzed.paths[index].clone()?;
        let file = sources.file(index);
        Some((path, self.range(file, span)))
    }

    fn location(&self, analyzed: &Analyzed, span: Span) -> Option<Value> {
        let (path, range) = self.place(analyzed, span)?;
        Some(json!({ "uri": self.uri(&path), "range": range }))
    }

    fn range(&self, file: &SourceFile, span: Span) -> Value {
        let start = (span.start - file.base) as usize;
        let end = (span.end.max(span.start) - file.base) as usize;
        json!({ "start": position(&file.text, start, self.units), "end": position(&file.text, end, self.units) })
    }

    /// The analysis, the file and the offset a request points at, once
    /// `prepare` made the analysis.
    fn at(&self, params: &Value) -> Option<(&Analyzed, usize, u32)> {
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        let analyzed = self.analyses.get(&key_of(&path))?;
        let index = analyzed.paths.iter().position(|p| p.as_deref() == Some(path.as_path()))?;
        let file = analyzed.analysis.sources.file(index);
        let offset = offset(&file.text, &params["position"], self.units)?;
        Some((analyzed, index, file.base + offset as u32))
    }

    /// The declaration a request points at, and the name it points at.
    fn def_at(&self, params: &Value) -> Option<(&Analyzed, usize, Span)> {
        let (analyzed, _, offset) = self.at(params)?;
        let index = analyzed.analysis.index.as_ref()?;
        let def = index.at(offset)?;
        let span = index.uses.iter().find(|(s, d)| *d == def && s.start <= offset && offset <= s.end)?.0;
        Some((analyzed, def, span))
    }

    /// The module an `import` line names, when the request points at it:
    /// its path, its first file when it has one, and the span of the path.
    fn imported(&self, params: &Value) -> Option<(&Analyzed, String, Option<usize>, Span)> {
        let (analyzed, file, offset) = self.at(params)?;
        let source = analyzed.analysis.sources.file(file);
        let at = (offset - source.base) as usize;
        let start = source.text[..at].rfind('\n').map_or(0, |i| i + 1);
        let line = source.text[start..].lines().next()?;
        let path = line.strip_prefix("import ")?.split_whitespace().next()?;
        let sources = &analyzed.analysis.sources;
        let target = (0..sources.len()).find(|&i| {
            let candidate = sources.file(i);
            candidate.module == path && analyzed.paths[i].is_some()
        });
        let span = Span::new(source.base + start as u32 + 7, source.base + (start + 7 + path.len()) as u32);
        Some((analyzed, path.to_string(), target, span))
    }

    fn hover(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        if let Some((analyzed, module, _, span)) = self.imported(params) {
            let (_, range) = self.place(analyzed, span)?;
            return Some(
                json!({ "contents": { "kind": "markdown", "value": format!("```tondo\nmodule {module}\n```") }, "range": range }),
            );
        }
        let units = self.units;
        let Some((analyzed, def, span)) = self.def_at(params) else { return self.builtin_hover(params) };
        let index = analyzed.analysis.index.as_ref()?;
        let def = &index.defs[def];
        let sources = &analyzed.analysis.sources;
        let file = sources.file_of(def.decl_span.start);
        let text = file.slice(def.decl_span);
        let ty = def.ty.clone().unwrap_or_else(|| "?".into());
        let header = match def.kind {
            DefKind::Local => format!("let {}: {ty}", def.name),
            DefKind::Parameter if def.name == "self" => format!("self: {ty}"),
            DefKind::Parameter => format!("{}: {ty}", def.name),
            DefKind::Module => format!("module {}", file.module),
            DefKind::Function | DefKind::Method => header(text),
            DefKind::Type | DefKind::Enum | DefKind::Trait => {
                let lines: Vec<&str> = text.lines().collect();
                match lines.len() > 24 {
                    true => format!("{}\n    ...\n}}", lines[..20].join("\n")),
                    false => text.to_string(),
                }
            }
            DefKind::Variant => {
                // `Role.Member(Int)`: the variant under its enum.
                let owner = index.defs.iter().find(|d| {
                    d.kind == DefKind::Enum
                        && d.decl_span.start <= def.decl_span.start
                        && def.decl_span.end <= d.decl_span.end
                });
                let line = file.text[(def.decl_span.start - file.base) as usize..].lines().next().unwrap_or_default();
                match owner {
                    Some(owner) => format!("{}.{}", owner.name, line.trim()),
                    None => line.trim().to_string(),
                }
            }
            _ => text.lines().next().unwrap_or_default().trim().to_string(),
        };
        let docs = docs_before(file, def.decl_span.start);
        let mut value = format!("```tondo\n{header}\n```");
        if !docs.is_empty() {
            value.push_str("\n\n");
            value.push_str(&docs);
        }
        // A context's field read here: where its value comes from (11.16).
        if let Some(&(_, set)) = index.contexts.iter().find(|(read, _)| *read == span) {
            let note = match set {
                Some(with) => {
                    let (line, _) = sources.file_of(with.start).line_col(with.start);
                    format!("Here its value is the one `with` gave it at line {line}.")
                }
                None => "Here its value is the one it has where this function is called, or in the task that started this one.".to_string(),
            };
            value.push_str(&format!("\n\n{note}"));
        }
        // A declaration of a file of a system: where each system has it.
        let places: Vec<String> = self
            .for_systems(analyzed, def)
            .into_iter()
            .filter_map(|(other, found)| {
                let there = other.analysis.sources.file_of(found.start);
                let systems = crate::project::systems_of(&there.path)?;
                let named: Vec<&str> = systems.iter().map(|s| s.shown()).collect();
                Some(format!("for {} in `{}`", named.join(" and "), there.logical))
            })
            .collect();
        if places.len() > 1 {
            let mut places = places;
            places.sort();
            value.push_str(&format!("\n\nOne declaration {}.", places.join(", ")));
        }
        let file = sources.file_of(span.start);
        let range = {
            let start = (span.start - file.base) as usize;
            let end = (span.end - file.base) as usize;
            json!({ "start": position(&file.text, start, units), "end": position(&file.text, end, units) })
        };
        Some(json!({ "contents": { "kind": "markdown", "value": value }, "range": range }))
    }

    /// What the runtime implements, which the program does not declare:
    /// `text.length()`, `console.println(...)`, from `tondo api`.
    fn builtin_hover(&self, params: &Value) -> Option<Value> {
        let (analyzed, file, offset) = self.at(params)?;
        let source = analyzed.analysis.sources.file(file);
        let text = &source.text;
        let at = (offset - source.base) as usize;
        let is_name = |c: char| c.is_alphanumeric() || c == '_';
        let start = text[..at].rfind(|c: char| !is_name(c)).map_or(0, |i| i + 1);
        let end = text[at..].find(|c: char| !is_name(c)).map_or(text.len(), |i| at + i);
        let name = &text[start..end];
        let (header, docs) = self.builtin(analyzed, file, start, name)?;
        let mut value = format!("```tondo\n{header}\n```");
        if !docs.is_empty() {
            value.push_str(&format!("\n\n{docs}"));
        }
        let span = Span::new(source.base + start as u32, source.base + end as u32);
        Some(json!({ "contents": { "kind": "markdown", "value": value }, "range": self.range(source, span) }))
    }

    /// The declaration and documentation of `name`, written at `start` after
    /// a dot, when the runtime implements it.
    fn builtin(&self, analyzed: &Analyzed, file: usize, start: usize, name: &str) -> Option<(String, String)> {
        let source = analyzed.analysis.sources.file(file);
        let text = &source.text;
        let dot = start.checked_sub(1).filter(|&d| text[d..].starts_with('.'))?;
        let receiver_start = text[..dot].rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
        let receiver = &text[receiver_start..dot];
        // A standard module the file imports.
        let imported = text.lines().filter_map(|line| line.strip_prefix("import ")).find_map(|rest| {
            let mut words = rest.split_whitespace();
            let path = words.next()?;
            let alias = match (words.next(), words.next()) {
                (Some("as"), Some(alias)) => alias,
                _ => path.rsplit('.').next()?,
            };
            (alias == receiver).then_some(path)
        });
        if let Some(module) = imported {
            return crate::api::function_doc(module, name);
        }
        // A value: its type's member.
        let index = analyzed.analysis.index.as_ref()?;
        let end = source.base + dot as u32;
        let (_, shape) = index.types.iter().filter(|(s, _)| s.end == end).min_by_key(|(s, _)| s.end - s.start)?;
        let member = index.shapes[*shape].members.iter().find(|m| m.name == name)?;
        crate::api::builtin_doc(member.builtin.as_ref()?, name)
    }

    fn highlights(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        let (analyzed, file, _) = self.at(params)?;
        let (_, def, _) = self.def_at(params)?;
        let index = analyzed.analysis.index.as_ref()?;
        let source = analyzed.analysis.sources.file(file);
        let own = index.defs[def].name_span;
        let highlights: Vec<Value> = index
            .references(def)
            .into_iter()
            .filter(|span| span.start >= source.base && span.end <= source.base + source.text.len() as u32)
            .map(|span| json!({ "range": self.range(source, span), "kind": if span == own { 3 } else { 2 } }))
            .collect();
        Some(Value::Array(highlights))
    }

    /// The program's declarations whose names contain the query, in every
    /// project the editor has open.
    fn workspace_symbols(&mut self, params: &Value) -> Value {
        self.refresh();
        let query = params["query"].as_str().unwrap_or_default().to_lowercase();
        let std = self.std.clone();
        let mut symbols = Vec::new();
        for analyzed in self.analyses.values() {
            let Some(index) = analyzed.analysis.index.as_ref() else { continue };
            for def in &index.defs {
                let kind = match def.kind {
                    DefKind::Function => 12,
                    DefKind::Method => 6,
                    DefKind::Type => 23,
                    DefKind::Enum => 10,
                    DefKind::Trait => 11,
                    DefKind::Const => 14,
                    _ => continue,
                };
                if !def.name.to_lowercase().contains(&query) {
                    continue;
                }
                let Some((path, range)) = self.place(analyzed, def.name_span) else { continue };
                if std.as_ref().is_some_and(|std| path.starts_with(std)) {
                    continue;
                }
                symbols.push(
                    json!({ "name": def.name, "kind": kind, "location": { "uri": self.uri(&path), "range": range } }),
                );
                if symbols.len() >= 500 {
                    return Value::Array(symbols);
                }
            }
        }
        Value::Array(symbols)
    }

    /// The signature of the call being written, and which argument the
    /// cursor is in.
    fn signature_help(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        let (analyzed, file, offset) = self.at(params)?;
        let source = analyzed.analysis.sources.file(file);
        let text = &source.text;
        let cursor = (offset - source.base) as usize;
        // The innermost parenthesis still open before the cursor.
        let (mut depth, mut commas, mut open) = (0usize, 0usize, None);
        for (i, c) in text[..cursor].char_indices().rev() {
            match c {
                ')' | ']' | '}' => depth += 1,
                '(' | '[' | '{' if depth > 0 => depth -= 1,
                '(' => {
                    open = Some(i);
                    break;
                }
                '[' | '{' => return None,
                ',' if depth == 0 => commas += 1,
                _ => {}
            }
        }
        let open = open?;
        let is_name = |c: char| c.is_alphanumeric() || c == '_';
        let name_start = text[..open].rfind(|c: char| !is_name(c)).map_or(0, |i| i + 1);
        let name = &text[name_start..open];
        if name.is_empty() {
            return None;
        }
        let index = analyzed.analysis.index.as_ref()?;
        let callee = Span::new(source.base + name_start as u32, source.base + open as u32);
        let declared =
            index.uses.iter().find(|(span, _)| *span == callee).map(|&(_, def)| &index.defs[def]).or_else(|| {
                index.defs.iter().find(|d| matches!(d.kind, DefKind::Function | DefKind::Method) && d.name == name)
            });
        let (header, docs) = match declared.filter(|d| matches!(d.kind, DefKind::Function | DefKind::Method)) {
            Some(def) => {
                let file = analyzed.analysis.sources.file_of(def.decl_span.start);
                (header(file.slice(def.decl_span)), docs_before(file, def.decl_span.start))
            }
            None => self.builtin(analyzed, file, name_start, name)?,
        };
        let header = header.trim_start_matches("pub ").to_string();
        // The parameters, as written between the header's parentheses.
        let start = header.find('(')? + 1;
        let (mut depth, mut from, mut parameters) = (0usize, start, Vec::new());
        for (i, c) in header[start..].char_indices().map(|(i, c)| (i + start, c)) {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' if depth == 0 => {
                    parameters.push(header[from..i].trim().to_string());
                    break;
                }
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    parameters.push(header[from..i].trim().to_string());
                    from = i + 1;
                }
                _ => {}
            }
        }
        // A method called on a value takes it as `self`.
        if text[..name_start].ends_with('.')
            && parameters
                .first()
                .is_some_and(|p| p.starts_with("self") || p.starts_with("mut self") || p.starts_with("var self"))
        {
            parameters.remove(0);
        }
        parameters.retain(|p| !p.is_empty());
        let parameters: Vec<Value> = parameters.iter().map(|p| json!({ "label": p })).collect();
        let active = commas.min(parameters.len().saturating_sub(1));
        let mut signature = json!({ "label": header, "parameters": parameters, "activeParameter": active });
        if !docs.is_empty() {
            signature["documentation"] = json!({ "kind": "markdown", "value": docs });
        }
        Some(json!({ "signatures": [signature], "activeSignature": 0, "activeParameter": active }))
    }

    /// The type of each binding the file declares without writing it.
    fn inlay_hints(&mut self, params: &Value) -> Option<Value> {
        let start = json!({ "textDocument": params["textDocument"], "position": params["range"]["start"] });
        let end = json!({ "textDocument": params["textDocument"], "position": params["range"]["end"] });
        self.prepare(&start)?;
        let (analyzed, file, from) = self.at(&start)?;
        let to = self.at(&end).map_or(u32::MAX, |(_, _, to)| to);
        let index = analyzed.analysis.index.as_ref()?;
        let source = analyzed.analysis.sources.file(file);
        let mut hints = Vec::new();
        for def in &index.defs {
            let (Some(ty), DefKind::Local) = (&def.ty, def.kind) else { continue };
            let span = def.name_span;
            if def.name == "_" || span.start < from.max(source.base) || span.end > to {
                continue;
            }
            if span.end > source.base + source.text.len() as u32 {
                continue;
            }
            let after = &source.text[(span.end - source.base) as usize..];
            if after.trim_start_matches([' ', '\t']).starts_with(':') {
                continue;
            }
            let position = position(&source.text, (span.end - source.base) as usize, self.units);
            hints.push(json!({ "position": position, "label": format!(": {ty}"), "kind": 1 }));
        }
        // The parameter each argument goes to, where the call does not name
        // it and the argument is not its name already.
        let tokens = crate::lexer::lex(source, None, &mut crate::diag::Diagnostics::default());
        let end = source.base + source.text.len() as u32;
        for &(span, used) in &index.uses {
            let callee = &index.defs[used];
            let callable = matches!(callee.kind, DefKind::Function | DefKind::Method);
            let declared = span == callee.name_span || index.implemented.iter().any(|&(_, s)| s == span);
            if !callable || declared || span.start < from.max(source.base) || span.end > to.min(end) {
                continue;
            }
            let Some(at) = tokens.iter().position(|t| t.span.start >= span.end) else { continue };
            if tokens[at].tok != Tok::LParen || tokens[at].span.start != span.end {
                continue;
            }
            let declared = analyzed.analysis.sources.file_of(callee.decl_span.start);
            let names = parameter_names(&header(declared.slice(callee.decl_span)));
            // `value.method(...)` passes `self` before the parentheses;
            // `Type.method(value)` within them.
            let before = &source.text[..(span.start - source.base) as usize];
            let receiver = before
                .strip_suffix('.')
                .map(|b| b.rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or_default());
            let skip = receiver.is_some_and(|r| !r.starts_with(char::is_uppercase))
                && names.first().is_some_and(|n| n == "self");
            let names = &names[skip as usize..];
            for (argument, first, named, alone) in arguments(&tokens[at + 1..]) {
                let Some(name) = names.get(argument) else { break };
                if named || name == "self" || alone.as_deref() == Some(name.as_str()) {
                    continue;
                }
                let position = position(&source.text, (first - source.base) as usize, self.units);
                hints.push(
                    json!({ "position": position, "label": format!("{name}:"), "kind": 2, "paddingRight": true }),
                );
            }
        }
        Some(Value::Array(hints))
    }

    fn definition(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        // The file the path of `embed.text` or `embed.bytes` reads.
        if let Some((analyzed, file, offset)) = self.at(params)
            && let source = analyzed.analysis.sources.file(file)
            && let Some((_, whole)) = embed_path(&source.text, (offset - source.base) as usize)
        {
            let path = analyzed.paths[file].clone()?;
            let found = crate::embed::resolve(&path, &whole).ok().filter(|found| found.is_file())?;
            let start = json!({ "line": 0, "character": 0 });
            return Some(json!({ "uri": self.uri(&found), "range": { "start": start, "end": start } }));
        }
        if let Some((analyzed, _, target, _)) = self.imported(params) {
            let base = analyzed.analysis.sources.file(target?).base;
            return self.location(analyzed, Span::new(base, base));
        }
        let (analyzed, def, _) = self.def_at(params)?;
        let def = &analyzed.analysis.index.as_ref()?.defs[def];
        let span = if def.kind == DefKind::Module { def.decl_span } else { def.name_span };
        let here = self.location(analyzed, span)?;
        // A declaration of a file of a system has one for each other
        // system: all of them, as implementations (language 6.8).
        let mut locations = vec![here];
        for (other, found) in self.for_systems(analyzed, def) {
            if let Some(location) = self.location(other, found)
                && !locations.contains(&location)
            {
                locations.push(location);
            }
        }
        Some(if locations.len() == 1 { locations.remove(0) } else { Value::Array(locations) })
    }

    /// For a declaration of a file of a system, those of its name in the
    /// files of every system, its own included, each with its analysis.
    fn for_systems<'s>(&'s self, analyzed: &'s Analyzed, def: &crate::index::Def) -> Vec<(&'s Analyzed, Span)> {
        let file = analyzed.analysis.sources.file_of(def.name_span.start);
        if crate::project::systems_of(&file.path).is_none() {
            return Vec::new();
        }
        let (root, _) = root_of(&file.path);
        let keys = std::iter::once(root.clone())
            .chain(crate::project::System::ALL.map(|system| root.join(format!("#{}", system.name()))));
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        for other in keys.filter_map(|key| self.analyses.get(&key)) {
            let Some(index) = other.analysis.index.as_ref() else { continue };
            for candidate in index.defs.iter().filter(|d| d.name == def.name && d.kind == def.kind) {
                let there = other.analysis.sources.file_of(candidate.name_span.start);
                if there.module == file.module
                    && crate::project::systems_of(&there.path).is_some()
                    && seen.insert((there.path.clone(), candidate.name_span.start - there.base))
                {
                    found.push((other, candidate.name_span));
                }
            }
        }
        found
    }

    fn references(&mut self, params: &Value) -> Option<Value> {
        let declaration = params["context"]["includeDeclaration"].as_bool().unwrap_or(true);
        self.prepare(params)?;
        let (analyzed, def, _) = self.def_at(params)?;
        let index = analyzed.analysis.index.as_ref()?;
        let own = index.defs[def].name_span;
        let locations: Vec<Value> = index
            .references(def)
            .into_iter()
            .filter(|&span| declaration || span != own)
            .filter_map(|span| self.location(analyzed, span))
            .collect();
        Some(Value::Array(locations))
    }

    /// Whether a declaration can be renamed: one of the program's own, in a
    /// file the editor can change.
    fn renamable(&self, analyzed: &Analyzed, index: &Index, def: usize) -> bool {
        let def = &index.defs[def];
        if matches!(def.kind, DefKind::Module) || def.name == "self" {
            return false;
        }
        let std = self.std.as_deref();
        match self.place(analyzed, def.name_span) {
            Some((path, _)) => std.is_none_or(|std| !path.starts_with(std)),
            None => false,
        }
    }

    fn prepare_rename(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        let (analyzed, def, span) = self.def_at(params)?;
        let index = analyzed.analysis.index.as_ref()?;
        if !self.renamable(analyzed, index, def) {
            return None;
        }
        let (_, range) = self.place(analyzed, span)?;
        Some(json!({ "range": range, "placeholder": index.defs[def].name }))
    }

    fn rename(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params["newName"].as_str().unwrap_or_default().to_string();
        if !crate::project::is_identifier(&name) || crate::lexer::Kw::lookup(&name).is_some() {
            return Err((-32602, format!("`{name}` is not a name Tondo accepts")));
        }
        self.prepare(params);
        let Some((analyzed, def, _)) = self.def_at(params) else { return Ok(Value::Null) };
        let Some(index) = analyzed.analysis.index.as_ref() else { return Ok(Value::Null) };
        if !self.renamable(analyzed, index, def) {
            return Err((-32602, "only the program's own declarations can be renamed".into()));
        }
        let mut changes: HashMap<String, Vec<Value>> = HashMap::new();
        let field = index.defs[def].kind == DefKind::Field;
        for span in index.references(def) {
            // `User { name }` names a field and a binding at once.
            let old = analyzed.analysis.sources.slice(span);
            let text = match index.shorthands.contains(&span) {
                true if field => format!("{name}: {old}"),
                true => format!("{old}: {name}"),
                false => name.clone(),
            };
            if let Some((path, range)) = self.place(analyzed, span) {
                changes.entry(self.uri(&path)).or_default().push(json!({ "range": range, "newText": text }));
            }
        }
        Ok(json!({ "changes": changes }))
    }

    fn symbols(&mut self, params: &Value) -> Option<Value> {
        let start = json!({ "textDocument": params["textDocument"], "position": { "line": 0, "character": 0 } });
        self.prepare(&start)?;
        let (analyzed, file, _) = self.at(&start)?;
        let index = analyzed.analysis.index.as_ref()?;
        let source = analyzed.analysis.sources.file(file);
        let inside = |span: Span| span.start >= source.base && span.end <= source.base + source.text.len() as u32;
        let symbol = |def: &crate::index::Def, children: Vec<Value>| {
            let kind = match def.kind {
                DefKind::Function => 12,
                DefKind::Method => 6,
                DefKind::Type => 23,
                DefKind::Enum => 10,
                DefKind::Trait => 11,
                DefKind::Const => 14,
                DefKind::Field => 8,
                DefKind::Variant => 22,
                _ => 13,
            };
            json!({
                "name": def.name,
                "kind": kind,
                "range": self.range(source, def.decl_span),
                "selectionRange": self.range(source, def.name_span),
                "children": children,
            })
        };
        let top = |kind: DefKind| {
            matches!(
                kind,
                DefKind::Function | DefKind::Method | DefKind::Type | DefKind::Enum | DefKind::Trait | DefKind::Const
            )
        };
        let mut symbols = Vec::new();
        for def in index.defs.iter().filter(|d| top(d.kind) && inside(d.name_span)) {
            let children = index
                .defs
                .iter()
                .filter(|d| matches!(d.kind, DefKind::Field | DefKind::Variant) && inside(d.name_span))
                .filter(|d| d.decl_span.start >= def.decl_span.start && d.decl_span.end <= def.decl_span.end)
                .map(|d| symbol(d, Vec::new()))
                .collect();
            symbols.push(symbol(def, children));
        }
        symbols.sort_by_key(|s| (s["range"]["start"]["line"].as_u64(), s["range"]["start"]["character"].as_u64()));
        Some(Value::Array(symbols))
    }

    fn completion(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        let root = key_of(&path);
        let (latest, file, offset) = self.at(params)?;
        let source = latest.analysis.sources.file(file);
        let text = source.text.clone();
        let cursor = (offset - source.base) as usize;
        // In the path of `std.embed`, what the directory written has.
        if let Some((typed, _)) = embed_path(&text, cursor) {
            return Some(json!({ "isIncomplete": false, "items": embed_entries(&path, &typed) }));
        }
        // In an import, the modules there are.
        let line = text[..cursor].rfind('\n').map_or(0, |at| at + 1);
        if let Some(written) = text[line..cursor].strip_prefix("import ") {
            return Some(json!({ "isIncomplete": false, "items": importable(latest, &source.module, written) }));
        }
        let start = text[..cursor].rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
        // After `@`, the program's annotations; in their arguments, their fields.
        if text[..start].ends_with('@') {
            return Some(json!({ "isIncomplete": false, "items": self.annotation_names(latest, offset) }));
        }
        if let Some(items) = self.annotation_fields(latest, offset, &text[line..cursor]) {
            return Some(json!({ "isIncomplete": false, "items": items }));
        }
        if !text[..start].ends_with('.') {
            return Some(json!({ "isIncomplete": false, "items": self.names(latest, offset) }));
        }
        let end = start - 1;
        // A module the file imports, a type, or a type of a module: what it
        // declares. Otherwise a value, whose members follow.
        let chain_start =
            text[..end].rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).map_or(0, |i| i + 1);
        let parts: Vec<&str> = text[chain_start..end].split('.').collect();
        let imports = imports(&text);
        let here = source.module.clone();
        let call = text[cursor..].starts_with('(');
        let analyses: Vec<&Analyzed> =
            std::iter::once(latest).chain(self.earlier.get(&root).into_iter().flatten()).collect();
        let annotation = text[..chain_start].ends_with('@');
        let found = match parts[..] {
            [alias] if annotation => {
                let module = imports.get(alias)?;
                Some(analyses.iter().find_map(|analyzed| self.annotations_of(analyzed, module)).unwrap_or_default())
            }
            [alias] if let Some(module) = imports.get(alias) => {
                analyses.iter().find_map(|analyzed| self.module_members(analyzed, module, call))
            }
            [alias, name] if let Some(module) = imports.get(alias) => {
                analyses.iter().find_map(|analyzed| self.type_members(analyzed, Some(module), name, call))
            }
            [name] if name.starts_with(|c: char| c.is_uppercase()) => analyses.iter().find_map(|analyzed| {
                self.type_members(analyzed, None, name, call)
                    .or_else(|| self.type_members(analyzed, Some(&here), name, call))
            }),
            _ => None,
        };
        if let Some(items) = found {
            return Some(json!({ "isIncomplete": false, "items": items }));
        }
        // A value: what is before the dot parses once a name follows it.
        let mut written = text.clone();
        if start == cursor {
            written.insert_str(cursor, "completed");
        }
        let mut open: HashMap<PathBuf, String> =
            self.open.iter().map(|(path, document)| (path.clone(), document.text.clone())).collect();
        open.insert(path.clone(), written);
        let analyzed = self.analyzed(&root, open)?;
        let file = analyzed.paths.iter().position(|p| p.as_deref() == Some(path.as_path()))?;
        let base = analyzed.analysis.sources.file(file).base;
        let items = self.members(&analyzed, base + end as u32, &here, call).unwrap_or_default();
        Some(json!({ "isIncomplete": false, "items": items }))
    }

    /// The annotations a file writes after `@` (27.13): its module's, and
    /// the modules it imports that declare some, whose follow a dot.
    fn annotation_names(&self, analyzed: &Analyzed, offset: u32) -> Vec<Value> {
        let sources = &analyzed.analysis.sources;
        let here = sources.file_of(offset);
        let mut items = self.annotations_of(analyzed, &here.module).unwrap_or_default();
        for (alias, module) in imports(&here.text) {
            if self.annotations_of(analyzed, &module).is_some_and(|found| !found.is_empty()) {
                items.push(json!({ "label": alias, "kind": 9, "detail": format!("import {module}") }));
            }
        }
        items
    }

    /// The annotations `module` declares.
    fn annotations_of(&self, analyzed: &Analyzed, module: &str) -> Option<Vec<Value>> {
        let index = analyzed.analysis.index.as_ref()?;
        let sources = &analyzed.analysis.sources;
        let items = index
            .defs
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                let text = sources.slice(d.decl_span);
                d.kind == DefKind::Type
                    && sources.file_of(d.decl_span.start).module == module
                    && text.strip_prefix("pub ").unwrap_or(text).starts_with("annotation ")
            })
            .map(|(id, d)| self.item(analyzed, &d.name, d.kind, Some(id), false))
            .collect();
        Some(items)
    }

    /// In the arguments of an annotation, where a field's name goes, as in
    /// `@Check(mi`: its fields not written yet.
    fn annotation_fields(&self, analyzed: &Analyzed, offset: u32, line: &str) -> Option<Vec<Value>> {
        let open = line.rfind('(')?;
        let args = &line[open + 1..];
        let written = args.rsplit(',').next().unwrap_or(args).trim_start();
        if args.contains(')') || !written.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }
        let path = &line[line[..open].rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))? + 1..open];
        if !line[..open - path.len()].ends_with('@') {
            return None;
        }
        let index = analyzed.analysis.index.as_ref()?;
        let sources = &analyzed.analysis.sources;
        let here = sources.file_of(offset);
        let (module, name) = match path.split_once('.') {
            Some((alias, name)) => (imports(&here.text).get(alias)?.clone(), name),
            None => (here.module.clone(), path),
        };
        let of_module = |d: &crate::index::Def| sources.file_of(d.decl_span.start).module == module;
        let annotation = index.defs.iter().find(|d| d.kind == DefKind::Type && d.name == name && of_module(d))?;
        let given: Vec<&str> = args.split(',').filter_map(|a| Some(a.split_once(':')?.0.trim())).collect();
        let items = index
            .defs
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                d.kind == DefKind::Field
                    && of_module(d)
                    && annotation.decl_span.start <= d.decl_span.start
                    && d.decl_span.end <= annotation.decl_span.end
                    && !given.contains(&d.name.as_str())
            })
            .map(|(id, d)| {
                let mut item = self.item(analyzed, &d.name, d.kind, Some(id), false);
                item["insertText"] = json!(format!("{}: ", d.name));
                item
            })
            .collect();
        Some(items)
    }

    /// The public declarations of `module`: a standard one's, as `tondo
    /// api` shows them, or one of the program's.
    fn module_members(&self, analyzed: &Analyzed, module: &str, call: bool) -> Option<Vec<Value>> {
        if let Some(text) = crate::api::standard(module) {
            let items = crate::api::declarations(&text)
                .iter()
                .filter(|d| d["kind"] != "impl" && !d["name"].as_str().unwrap_or(".").contains('.'))
                .map(|d| declared_item(d, call))
                .collect();
            return Some(items);
        }
        let index = analyzed.analysis.index.as_ref()?;
        let sources = &analyzed.analysis.sources;
        (0..sources.len()).find(|&i| sources.file(i).module == module)?;
        let items = index
            .defs
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                matches!(d.kind, DefKind::Function | DefKind::Type | DefKind::Enum | DefKind::Trait | DefKind::Const)
                    && sources.file_of(d.decl_span.start).module == module
                    && sources.slice(d.decl_span).starts_with("pub ")
            })
            .map(|(id, d)| self.item(analyzed, &d.name, d.kind, Some(id), call))
            .collect();
        Some(items)
    }

    /// What a type gives by its name: its functions that take no `self`,
    /// as `Builder.new`, and an enum's variants. The type is of `module`,
    /// or with none, one the compiler gives, as `String`.
    fn type_members(&self, analyzed: &Analyzed, module: Option<&str>, name: &str, call: bool) -> Option<Vec<Value>> {
        let standard = match module {
            Some(module) => crate::api::standard(module),
            None => crate::api::standard("prelude"),
        };
        if let Some(text) = standard {
            let declarations = crate::api::declarations(&text);
            let prefix = format!("{name}.");
            let mut items = Vec::new();
            for d in &declarations {
                let (kind, declared) = (d["kind"].as_str().unwrap_or(""), d["name"].as_str().unwrap_or(""));
                let code = d["declaration"].as_str().unwrap_or("");
                if kind == "fn" && declared.starts_with(&prefix) && !crate::api::takes_self(code) {
                    let mut d = d.clone();
                    d["name"] = json!(&declared[prefix.len()..]);
                    items.push(declared_item(&d, call));
                } else if kind == "enum" && declared == name {
                    items.extend(variants(code).into_iter().map(|v| json!({ "label": v, "kind": 20 })));
                } else if kind == "alias"
                    && declared == name
                    && let Some((owner, target)) = code.split_once('=').and_then(|(_, t)| t.trim().split_once('.'))
                {
                    // Another standard module's type, as `json.ErrorKind`.
                    let module = format!("std.{owner}");
                    return self.type_members(analyzed, Some(&module), target.trim(), call);
                }
            }
            let known = declarations
                .iter()
                .any(|d| d["name"] == name || d["name"].as_str().is_some_and(|n| n.starts_with(&prefix)));
            if known {
                return Some(items);
            }
            if module.is_some_and(|m| m.starts_with("std.")) {
                return None;
            }
        }
        let module = module?;
        let index = analyzed.analysis.index.as_ref()?;
        let sources = &analyzed.analysis.sources;
        let of_module = |d: &crate::index::Def| sources.file_of(d.decl_span.start).module == module;
        let ty = index
            .defs
            .iter()
            .find(|d| matches!(d.kind, DefKind::Type | DefKind::Enum) && d.name == name && of_module(d))?;
        let mut items = Vec::new();
        for (id, d) in index.defs.iter().enumerate().filter(|(_, d)| of_module(d)) {
            let text = sources.slice(d.decl_span);
            let owned = text.strip_prefix("pub ").unwrap_or(text).starts_with(&format!("fn {name}."));
            match d.kind {
                DefKind::Function | DefKind::Method if owned && !crate::api::takes_self(&header(text)) => {
                    items.push(self.item(analyzed, &d.name, DefKind::Function, Some(id), call))
                }
                DefKind::Variant if ty.decl_span.start <= d.decl_span.start && d.decl_span.end <= ty.decl_span.end => {
                    items.push(self.item(analyzed, &d.name, d.kind, Some(id), false))
                }
                _ => {}
            }
        }
        Some(items)
    }

    /// The members of the type of the innermost expression that ends at
    /// `end`, before a dot, that the module `here` sees: not another's
    /// private fields, nor the functions it does not make public.
    fn members(&self, analyzed: &Analyzed, end: u32, here: &str, call: bool) -> Option<Vec<Value>> {
        let index = analyzed.analysis.index.as_ref()?;
        let sources = &analyzed.analysis.sources;
        let (_, shape) =
            index.types.iter().filter(|(span, _)| span.end == end).min_by_key(|(span, _)| span.end - span.start)?;
        let shape = &index.shapes[*shape];
        let hidden = |member: &crate::index::Member| {
            let Some(def) = member.def.map(|d| &index.defs[d]) else { return false };
            let theirs = sources.file_of(def.decl_span.start).module != here;
            let public = !member.method || sources.slice(def.decl_span).starts_with("pub ");
            theirs && (member.private || !public)
        };
        let items = shape
            .members
            .iter()
            .filter(|member| !hidden(member))
            .map(|member| {
                let kind = if member.method { DefKind::Method } else { DefKind::Field };
                let mut item = self.item(analyzed, &member.name, kind, member.def, call);
                if let (None, Some(ty)) = (member.def, &member.ty) {
                    item["detail"] = json!(ty);
                }
                if let Some(owner) = &member.builtin
                    && let Some((header, docs)) = crate::api::builtin_doc(owner, &member.name)
                {
                    item = declared_item(
                        &json!({ "kind": "fn", "name": member.name, "declaration": header, "doc": docs }),
                        call,
                    );
                    item["kind"] = json!(2);
                }
                item
            })
            .collect();
        Some(items)
    }

    /// The names visible at `offset`: the bindings of its function before
    /// it, the declarations of its module, the modules it uses, keywords.
    fn names(&self, analyzed: &Analyzed, offset: u32) -> Vec<Value> {
        let Some(index) = analyzed.analysis.index.as_ref() else { return Vec::new() };
        let sources = &analyzed.analysis.sources;
        let here = sources.file_of(offset);
        let function = index
            .defs
            .iter()
            .filter(|d| matches!(d.kind, DefKind::Function | DefKind::Method))
            .filter(|d| d.decl_span.start <= offset && offset <= d.decl_span.end)
            .min_by_key(|d| d.decl_span.end - d.decl_span.start);
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for (id, def) in index.defs.iter().enumerate().rev() {
            let visible = match def.kind {
                DefKind::Local | DefKind::Parameter => {
                    function.is_some_and(|f| f.decl_span.start <= def.name_span.start && def.name_span.end <= offset)
                }
                DefKind::Function | DefKind::Type | DefKind::Enum | DefKind::Trait | DefKind::Const => {
                    sources.file_of(def.decl_span.start).module == here.module
                }
                _ => false,
            };
            if visible && seen.insert(def.name.clone()) {
                items.push(self.item(analyzed, &def.name, def.kind, Some(id), false));
            }
        }
        // The modules this file names.
        let (start, end) = (here.base, here.base + here.text.len() as u32);
        for &(span, def) in &index.uses {
            let found = &index.defs[def];
            if found.kind == DefKind::Module
                && span.start >= start
                && span.end <= end
                && seen.insert(found.name.clone())
            {
                items.push(self.item(analyzed, &found.name, DefKind::Module, Some(def), false));
            }
        }
        // The modules the file does not import yet, with their import.
        let imported: HashSet<&str> =
            here.text.lines().filter_map(|l| l.strip_prefix("import ")?.split_whitespace().next()).collect();
        let mut after = 0;
        let mut offset = 0;
        for line in here.text.split_inclusive('\n') {
            offset += line.len();
            if line.starts_with("import ") {
                after = offset;
            }
        }
        let place = position(&here.text, after, self.units);
        let mut modules: Vec<String> =
            crate::api::standard_modules().into_iter().filter(|m| m.starts_with("std.")).collect();
        modules.extend((0..sources.len()).filter(|&i| sources.file(i).own).map(|i| sources.file(i).module.clone()));
        modules.dedup();
        for module in modules {
            let name = module.rsplit('.').next().unwrap_or_default().to_string();
            if imported.contains(module.as_str()) || module == here.module || !seen.insert(name.clone()) {
                continue;
            }
            items.push(json!({
                "label": name, "kind": 9, "detail": format!("import {module}"), "sortText": format!("~{name}"),
                "additionalTextEdits": [{ "range": { "start": place, "end": place }, "newText": format!("import {module}\n") }],
            }));
        }
        for keyword in KEYWORDS {
            items.push(json!({ "label": keyword, "kind": 14 }));
        }
        items
    }

    /// A completion item, with the declaration's header and documentation;
    /// a function's with its parameters and result beside its name, and,
    /// unless the call is written already (`call`), its parentheses.
    fn item(&self, analyzed: &Analyzed, name: &str, kind: DefKind, def: Option<usize>, call: bool) -> Value {
        let number = match kind {
            DefKind::Function => 3,
            DefKind::Method => 2,
            DefKind::Type => 22,
            DefKind::Enum => 13,
            DefKind::Trait => 8,
            DefKind::Field => 5,
            DefKind::Variant => 20,
            DefKind::Const => 21,
            DefKind::Local | DefKind::Parameter => 6,
            DefKind::Module => 9,
        };
        let mut item = json!({ "label": name, "kind": number });
        let Some(def) = def.and_then(|d| analyzed.analysis.index.as_ref().map(|i| &i.defs[d])) else { return item };
        let file = analyzed.analysis.sources.file_of(def.decl_span.start);
        let detail = match def.kind {
            DefKind::Local | DefKind::Parameter => def.ty.clone().unwrap_or_default(),
            DefKind::Function | DefKind::Method => header(file.slice(def.decl_span)),
            DefKind::Module => format!("module {}", file.module),
            _ => file.slice(def.decl_span).lines().next().unwrap_or_default().trim().to_string(),
        };
        item["detail"] = json!(detail);
        let docs = docs_before(file, def.decl_span.start);
        if !docs.is_empty() {
            item["documentation"] = json!({ "kind": "markdown", "value": docs });
        }
        if matches!(def.kind, DefKind::Function | DefKind::Method) {
            callable(&mut item, &detail, call);
        }
        item
    }

    fn semantic_tokens(&mut self, params: &Value) -> Option<Value> {
        let start = json!({ "textDocument": params["textDocument"], "position": { "line": 0, "character": 0 } });
        self.prepare(&start)?;
        let (analyzed, file, _) = self.at(&start)?;
        let source = analyzed.analysis.sources.file(file);
        let (base, end) = (source.base, source.base + source.text.len() as u32);
        let std = self.std.as_deref();
        let mut tokens: Vec<(u32, u32, usize, u32)> = Vec::new();
        if let Some(index) = analyzed.analysis.index.as_ref() {
            for &(span, def) in index.uses.iter().filter(|(s, _)| s.start >= base && s.end <= end) {
                let found = &index.defs[def];
                let kind = match found.kind {
                    DefKind::Module => 0,
                    DefKind::Type => 1,
                    DefKind::Enum => 2,
                    DefKind::Trait => 3,
                    DefKind::Parameter => 5,
                    DefKind::Local => 6,
                    DefKind::Field => 7,
                    DefKind::Variant => 8,
                    DefKind::Function => 9,
                    DefKind::Method => 10,
                    DefKind::Const => 6,
                };
                let mut modifiers = 0;
                if span == found.name_span {
                    modifiers |= 1;
                }
                if found.kind == DefKind::Const {
                    modifiers |= 2;
                }
                let library =
                    self.place(analyzed, found.name_span).is_some_and(|(p, _)| std.is_some_and(|s| p.starts_with(s)));
                if library {
                    modifiers |= 4;
                }
                tokens.push((span.start - base, span.end - base, kind, modifiers));
            }
        }
        let names: Vec<(u32, u32)> = tokens.iter().map(|t| (t.0, t.1)).collect();
        let (lexed, comments) = crate::lexer::lex_with_comments(source, None, &mut crate::diag::Diagnostics::default());
        let mut add = |start: u32, end: u32, kind: usize| {
            // A string's interpolated names are tokens of their own.
            let mut at = start;
            for &(s, e) in names.iter().filter(|(s, e)| *s >= start && *e <= end) {
                if s > at {
                    tokens.push((at, s, kind, 0));
                }
                at = at.max(e);
            }
            if at < end {
                tokens.push((at, end, kind, 0));
            }
        };
        for (index, token) in lexed.iter().enumerate() {
            let (start, end) = (token.span.start - base, token.span.end - base);
            match &token.tok {
                crate::lexer::Tok::Kw(_) => add(start, end, 11),
                // `recover`, `context` and `within`, where they are words of the language.
                crate::lexer::Tok::Ident(_) if crate::parser::keyword_at(&lexed, index) => add(start, end, 11),
                crate::lexer::Tok::Str(_) | crate::lexer::Tok::Char(_) => add(start, end, 13),
                crate::lexer::Tok::Int { .. } | crate::lexer::Tok::Float { .. } => add(start, end, 14),
                // A type the program does not declare, as `Int` or `Array`.
                crate::lexer::Tok::Ident(name) if name.starts_with(char::is_uppercase) => add(start, end, 1),
                _ => {}
            }
        }
        for comment in &comments {
            add(comment.start - base, comment.end - base, 12);
        }
        tokens.sort_by_key(|t| (t.0, t.1));
        tokens.dedup_by_key(|t| t.0);
        // One line each, as relative positions.
        let text = &source.text;
        let mut data = Vec::new();
        let (mut last_line, mut last_column) = (0, 0);
        for (start, end, kind, modifiers) in tokens {
            let mut from = start as usize;
            let end = end as usize;
            while from < end {
                let line_end = text[from..end].find('\n').map_or(end, |i| from + i);
                if line_end > from {
                    let line_start = text[..from].rfind('\n').map_or(0, |i| i + 1);
                    let line = text[..from].matches('\n').count();
                    let column = self.units.count(&text[line_start..from]);
                    let length = self.units.count(&text[from..line_end]);
                    let delta_line = line - last_line;
                    let delta_column = if delta_line == 0 { column - last_column } else { column };
                    data.extend([delta_line, delta_column, length, kind, modifiers as usize]);
                    (last_line, last_column) = (line, column);
                }
                from = line_end + 1;
            }
        }
        Some(json!({ "data": data }))
    }

    /// The repairs of the diagnostics in a range, each a quick fix, and one
    /// action that applies every safe repair of the file.
    fn code_actions(&mut self, params: &Value) -> Option<Value> {
        let start = json!({ "textDocument": params["textDocument"], "position": params["range"]["start"] });
        let end = json!({ "textDocument": params["textDocument"], "position": params["range"]["end"] });
        self.prepare(&start)?;
        let (analyzed, file, from) = self.at(&start)?;
        let (_, _, to) = self.at(&end)?;
        let source = analyzed.analysis.sources.file(file);
        let inside = |span: Span| span.start >= source.base && span.end <= source.base + source.text.len() as u32;
        let wanted: Vec<&str> = params["context"]["only"]
            .as_array()
            .map(|kinds| kinds.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let wants = |kind: &str| wanted.is_empty() || wanted.iter().any(|w| kind.starts_with(w));
        let mut actions = Vec::new();
        let mut all: Vec<Value> = Vec::new();
        let mut taken: Vec<Span> = Vec::new();
        // The imports the file lacks and those it does not use.
        let mut imports: Vec<(Span, String)> = Vec::new();
        for diagnostic in &analyzed.analysis.diags.list {
            let Some(span) = diagnostic.span.filter(|&span| inside(span)) else { continue };
            for fix in &diagnostic.fixes {
                if fix.safe && (fix.title == "remove the import" || fix.title.starts_with("add `import ")) {
                    imports
                        .extend(fix.edits.iter().filter(|edit| !imports.contains(edit)).cloned().collect::<Vec<_>>());
                }
                let edits = self.edits(analyzed, &fix.edits);
                if span.start <= to && from <= span.end && wants("quickfix") {
                    actions.push(json!({
                        "title": fix.title,
                        "kind": "quickfix",
                        "isPreferred": fix.safe,
                        "edit": { "changes": edits },
                    }));
                }
                // Safe repairs that do not touch each other.
                let clear = fix.edits.iter().all(|(s, _)| taken.iter().all(|t| s.end < t.start || t.end < s.start));
                if fix.safe && clear {
                    taken.extend(fix.edits.iter().map(|(s, _)| *s));
                    for (uri, list) in edits.as_object().into_iter().flatten() {
                        if uri == params["textDocument"]["uri"].as_str().unwrap_or_default() {
                            all.extend(list.as_array().into_iter().flatten().cloned());
                        }
                    }
                }
            }
        }
        if !imports.is_empty() && wants("source.organizeImports") {
            imports.sort_by(|(a, x), (b, y)| (a.start, x).cmp(&(b.start, y)));
            actions.push(json!({
                "title": "Organize imports",
                "kind": "source.organizeImports",
                "edit": { "changes": self.edits(analyzed, &imports) },
            }));
        }
        if !all.is_empty() && wants("source.fixAll") {
            let uri = params["textDocument"]["uri"].clone();
            let mut changes = serde_json::Map::new();
            changes.insert(uri.as_str().unwrap_or_default().to_string(), Value::Array(all));
            actions.push(json!({
                "title": "Apply every safe repair",
                "kind": "source.fixAll.tondo",
                "edit": { "changes": changes },
            }));
        }
        Some(Value::Array(actions))
    }

    /// A repair's edits, by file.
    fn edits(&self, analyzed: &Analyzed, edits: &[(Span, String)]) -> Value {
        let mut changes = serde_json::Map::new();
        for (span, replacement) in edits {
            if let Some((path, range)) = self.place(analyzed, *span) {
                let list = changes.entry(self.uri(&path)).or_insert_with(|| json!([]));
                list.as_array_mut().unwrap().push(json!({ "range": range, "newText": replacement }));
            }
        }
        Value::Object(changes)
    }

    fn formatting(&mut self, params: &Value) -> Option<Value> {
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        let text = match self.open.get(&path) {
            Some(document) => document.text.clone(),
            None => std::fs::read_to_string(&path).ok()?,
        };
        let mut sources = crate::source::SourceMap::default();
        let file = sources.add(&path, text.clone());
        let formatted = crate::fmt::format(sources.file(file), &mut crate::diag::Diagnostics::default())?;
        if formatted == text {
            return Some(json!([]));
        }
        let end = position(&text, text.len(), self.units);
        Some(json!([{ "range": { "start": { "line": 0, "character": 0 }, "end": end }, "newText": formatted }]))
    }
}

/// The token types of semantic highlighting, by number.
const TOKEN_TYPES: [&str; 16] = [
    "namespace",
    "type",
    "enum",
    "interface",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "function",
    "method",
    "keyword",
    "comment",
    "string",
    "number",
    "operator",
];

/// The keywords completion offers.
const KEYWORDS: [&str; 29] = [
    "let",
    "var",
    "fn",
    "pub",
    "type",
    "enum",
    "trait",
    "impl",
    "const",
    "import",
    "if",
    "else",
    "for",
    "in",
    "match",
    "return",
    "break",
    "continue",
    "fail",
    "spawn",
    "defer",
    "select",
    "recover",
    "context",
    "annotation",
    "with",
    "within",
    "none",
    "some",
];

/// The root a file is analyzed under: the project around it, or the file
/// itself; and whether it is a project.
/// The analysis a file is read with: its root's, or, for a file of another
/// system than this machine's, its root's for that system, which this key
/// names (language 6.8).
fn key_of(path: &Path) -> PathBuf {
    let (root, project) = root_of(path);
    let host = crate::project::System::host();
    match crate::project::systems_of(path) {
        Some(systems) if project && !systems.contains(&host) => root.join(format!("#{}", systems[0].name())),
        _ => root,
    }
}

fn root_of(path: &Path) -> (PathBuf, bool) {
    let ancestors = || path.ancestors().skip(1);
    if let Some(dir) = ancestors().find(|dir| dir.join("tondo.toml").is_file()) {
        return (dir.to_path_buf(), true);
    }
    if let Some(dir) = ancestors().find(|dir| dir.join("src").join("main.to").is_file()) {
        return (dir.to_path_buf(), true);
    }
    (path.to_path_buf(), false)
}

/// A function's declaration up to the `{` that opens its body.
fn header(text: &str) -> String {
    let mut end = text.len();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_end().ends_with('{') {
            end = start + line.trim_end().len() - 1;
            break;
        }
        start += line.len();
    }
    text[..end].trim_end().to_string()
}

/// The path of a call of `std.embed` the cursor is in, as in
/// `embed.text("static/in`: the path written up to the cursor, and all of it.
fn embed_path(text: &str, cursor: usize) -> Option<(String, String)> {
    let line = text[..cursor].rfind('\n').map_or(0, |at| at + 1);
    let before = &text[line..cursor];
    let open = before.rfind('"')?;
    let call = before[..open].strip_suffix('(')?;
    let start = call.rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).map_or(0, |at| at + 1);
    let (alias, kind) = call[start..].rsplit_once('.')?;
    crate::embed::Kind::of(kind)?;
    if imports(text).get(alias).map(String::as_str) != Some("std.embed") {
        return None;
    }
    let typed = before[open + 1..].to_string();
    let rest = &text[cursor..];
    let end = rest.find(['"', '\n']).unwrap_or(rest.len());
    let whole = format!("{typed}{}", &rest[..end]);
    Some((typed, whole))
}

/// What the directory of a path being written for `std.embed` has, from the
/// file `source`: its files, and its directories with `/`, but hidden ones.
fn embed_entries(source: &Path, typed: &str) -> Vec<Value> {
    let folder = typed.rfind('/').map_or("", |at| &typed[..=at]);
    let Ok(entries) = std::fs::read_dir(source.parent().unwrap_or(Path::new(".")).join(folder)) else {
        return Vec::new();
    };
    let mut items: Vec<(String, bool)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (!name.starts_with('.')).then(|| (name, entry.path().is_dir()))
        })
        .collect();
    items.sort();
    items
        .into_iter()
        .map(|(name, dir)| {
            let label = if dir { format!("{name}/") } else { name };
            json!({ "label": label, "kind": if dir { 19 } else { 17 } })
        })
        .collect()
}

/// The names a file imports modules under: `json` for `import std.json`,
/// `j` for `import std.json as j`.
fn imports(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("import ")?;
            let (path, alias) = match rest.split_once(" as ") {
                Some((path, alias)) => (path.trim(), alias.trim()),
                None => (rest.trim(), rest.trim().rsplit('.').next()?),
            };
            Some((alias.to_string(), path.to_string()))
        })
        .collect()
}

/// A completion item of a declaration as `tondo api` gives it: its kind,
/// name, text and documentation.
fn declared_item(declared: &Value, call: bool) -> Value {
    let kind = match declared["kind"].as_str().unwrap_or("") {
        "fn" => 3,
        "enum" => 13,
        "trait" => 8,
        "const" => 21,
        _ => 22,
    };
    let code = declared["declaration"].as_str().unwrap_or("");
    let detail = one_line(code.split(" {").next().unwrap_or(code));
    let mut item = json!({ "label": declared["name"], "kind": kind, "detail": detail });
    if let Some(docs) = declared["doc"].as_str().filter(|docs| !docs.is_empty()) {
        item["documentation"] = json!({ "kind": "markdown", "value": docs });
    }
    if kind == 3 {
        callable(&mut item, &detail, call);
    }
    item
}

/// A declaration written over several lines, on one.
fn one_line(text: &str) -> String {
    let joined = text.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    joined.replace("( ", "(").replace(", )", ")").replace(",)", ")")
}

/// A function's item: its parameters but `self` and its result beside
/// its name, and its parentheses inserted with the cursor between them,
/// unless the call is written already, then the hints of its parameters.
fn callable(item: &mut Value, declaration: &str, call: bool) {
    let declaration = one_line(declaration);
    let Some(after) = crate::api::parameters(&declaration) else { return };
    let mut rest = after;
    if crate::api::takes_self(&declaration) {
        let close = after.find(')').unwrap_or(after.len());
        rest = match after.find(',').filter(|&comma| comma < close) {
            Some(comma) => after[comma + 1..].trim_start(),
            None => &after[close..],
        };
    }
    item["labelDetails"] = json!({ "detail": format!("({rest}") });
    if call {
        return;
    }
    let name = item["label"].as_str().unwrap_or_default().to_string();
    if rest.starts_with(')') {
        item["insertText"] = json!(format!("{name}()"));
    } else {
        item["insertText"] = json!(format!("{name}($0)"));
        item["insertTextFormat"] = json!(2);
        item["command"] = json!({ "title": "", "command": "editor.action.triggerParameterHints" });
    }
}

/// The variants an `enum` declaration names.
fn variants(declaration: &str) -> Vec<String> {
    let Some(body) = declaration.split_once('{').map(|(_, body)| body) else { return Vec::new() };
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with('}'))
        .filter_map(|line| line.split(|c: char| !(c.is_alphanumeric() || c == '_')).next())
        .filter(|name| !name.is_empty())
        .map(String::from)
        .collect()
}

/// The `///` lines just above an offset, without their marks.
fn docs_before(file: &SourceFile, offset: u32) -> String {
    let before = &file.text[..(offset - file.base) as usize];
    let before = &before[..before.rfind('\n').map_or(0, |i| i + 1)];
    let mut lines: Vec<&str> = Vec::new();
    for line in before.lines().rev() {
        let line = line.trim();
        match line.strip_prefix("///") {
            Some(doc) => lines.push(doc.strip_prefix(' ').unwrap_or(doc)),
            None if line.starts_with('@') => continue,
            None => break,
        }
    }
    lines.reverse();
    lines.join("\n")
}

/// An LSP position: the line, and the column in the server's units.
fn position(text: &str, offset: usize, units: Units) -> Value {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = text[..line_start].matches('\n').count();
    let column = &text[line_start..offset];
    let character = units.count(column);
    json!({ "line": line, "character": character })
}

/// The byte offset of an LSP position.
fn offset(text: &str, position: &Value, units: Units) -> Option<usize> {
    let line = position["line"].as_u64()? as usize;
    let character = position["character"].as_u64()? as usize;
    let start = match line {
        0 => 0,
        _ => text.match_indices('\n').nth(line - 1)?.0 + 1,
    };
    let rest = &text[start..];
    let rest = &rest[..rest.find('\n').unwrap_or(rest.len())];
    if units == Units::Utf8 {
        return Some(start + character.min(rest.len()));
    }
    let mut counted = 0;
    for (index, c) in rest.char_indices() {
        if counted >= character {
            return Some(start + index);
        }
        counted += if units == Units::Chars { 1 } else { c.len_utf16() };
    }
    Some(start + rest.len())
}

pub(crate) fn file_uri(path: &Path) -> String {
    format!("file://{}", encode(&path.to_string_lossy()))
}

pub(crate) fn uri_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 3 <= bytes.len()
            && let Ok(byte) = u8::from_str_radix(&path[i + 1..i + 3], 16)
        {
            decoded.push(byte);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    let path = PathBuf::from(String::from_utf8(decoded).ok()?);
    Some(path.canonicalize().unwrap_or(path))
}

fn encode(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// What an editor asks beyond reading and changing names: the structure of
/// a file, calls and implementations, and what a folder renamed changes.
impl Server {
    /// The text of a document: as the editor has it, or as saved.
    fn document(&self, params: &Value) -> Option<(PathBuf, String)> {
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        let text = match self.open.get(&path) {
            Some(document) => document.text.clone(),
            None => std::fs::read_to_string(&path).ok()?,
        };
        Some((path, text))
    }

    /// Blocks, brackets that span lines, runs of imports and of comments.
    fn folding_ranges(&self, params: &Value) -> Option<Value> {
        let (path, text) = self.document(params)?;
        let mut sources = crate::source::SourceMap::default();
        let added = sources.add(&path, text);
        let file = sources.file(added);
        let (tokens, comments) = crate::lexer::lex_with_comments(file, None, &mut crate::diag::Diagnostics::default());
        let line = |offset: u32| file.line_col(offset).0 - 1;
        let mut ranges = Vec::new();
        let mut open = Vec::new();
        for token in &tokens {
            match token.tok {
                Tok::LBrace | Tok::LBracket | Tok::LParen => open.push(line(token.span.start)),
                Tok::RBrace | Tok::RBracket | Tok::RParen => {
                    if let Some(start) = open.pop() {
                        let end = line(token.span.start);
                        if end > start + 1 {
                            ranges.push(json!({ "startLine": start, "endLine": end - 1 }));
                        }
                    }
                }
                _ => {}
            }
        }
        // Lines in a row: imports, or comments.
        let mut runs = |lines: Vec<usize>, kind: &str| {
            let mut start = None;
            for (i, &l) in lines.iter().enumerate() {
                let first = *start.get_or_insert(l);
                let last = lines.get(i + 1).is_none_or(|&next| next != l + 1);
                if last {
                    if l > first {
                        ranges.push(json!({ "startLine": first, "endLine": l, "kind": kind }));
                    }
                    start = None;
                }
            }
        };
        let imports: Vec<usize> =
            tokens.iter().filter(|t| matches!(t.tok, Tok::Kw(Kw::Import))).map(|t| line(t.span.start)).collect();
        runs(imports, "imports");
        let mut commented: Vec<usize> = comments.iter().map(|c| line(c.start)).collect();
        commented.dedup();
        runs(commented, "comment");
        Some(Value::Array(ranges))
    }

    /// From the token at each position out: the brackets around it, what
    /// is between them, its line, and the file.
    fn selection_ranges(&self, params: &Value) -> Option<Value> {
        let (path, text) = self.document(params)?;
        let mut sources = crate::source::SourceMap::default();
        let added = sources.add(&path, text.clone());
        let file = sources.file(added);
        let tokens = crate::lexer::lex(file, None, &mut crate::diag::Diagnostics::default());
        let mut pairs = Vec::new();
        let mut open = Vec::new();
        for token in &tokens {
            match token.tok {
                Tok::LBrace | Tok::LBracket | Tok::LParen => open.push(token.span),
                Tok::RBrace | Tok::RBracket | Tok::RParen => {
                    if let Some(start) = open.pop() {
                        pairs.push((start.end, token.span.start));
                        pairs.push((start.start, token.span.end));
                    }
                }
                _ => {}
            }
        }
        let mut results = Vec::new();
        for asked in params["positions"].as_array().into_iter().flatten() {
            let at = offset(&text, asked, self.units)? as u32;
            let mut spans: Vec<(u32, u32)> = Vec::new();
            if let Some(token) = tokens
                .iter()
                .find(|t| t.span.start <= at && at <= t.span.end && !matches!(t.tok, Tok::Newline | Tok::Eof))
            {
                spans.push((token.span.start, token.span.end));
            }
            let mut around: Vec<(u32, u32)> = pairs.iter().copied().filter(|&(s, e)| s <= at && at <= e).collect();
            around.sort_by_key(|&(s, e)| e - s);
            spans.extend(around);
            let start = text[..at as usize].rfind('\n').map_or(0, |i| i + 1);
            let end = text[at as usize..].find('\n').map_or(text.len(), |i| at as usize + i);
            let indent = text[start..end].len() - text[start..end].trim_start().len();
            spans.push(((start + indent) as u32, end as u32));
            spans.push((0, text.len() as u32));
            // Each one around the one before.
            let mut kept: Vec<(u32, u32)> = Vec::new();
            for span in spans {
                if kept.last().is_none_or(|&(s, e)| span.0 <= s && e <= span.1 && span != (s, e)) {
                    kept.push(span);
                }
            }
            let mut range = Value::Null;
            for &(s, e) in kept.iter().rev() {
                let here = json!({ "start": position(&text, s as usize, self.units), "end": position(&text, e as usize, self.units) });
                range = match range {
                    Value::Null => json!({ "range": here }),
                    parent => json!({ "range": here, "parent": parent }),
                };
            }
            results.push(range);
        }
        Some(Value::Array(results))
    }

    /// The canonical format of the lines a range touches, where it differs.
    fn range_formatting(&self, params: &Value) -> Option<Value> {
        let (path, text) = self.document(params)?;
        let mut sources = crate::source::SourceMap::default();
        let file = sources.add(&path, text.clone());
        let formatted = crate::fmt::format(sources.file(file), &mut crate::diag::Diagnostics::default())?;
        let (first, last) =
            (params["range"]["start"]["line"].as_u64()? as usize, params["range"]["end"]["line"].as_u64()? as usize);
        let old: Vec<&str> = text.split_inclusive('\n').collect();
        let new: Vec<&str> = formatted.split_inclusive('\n').collect();
        let starts: Vec<usize> = std::iter::once(0)
            .chain(old.iter().scan(0, |at, l| {
                *at += l.len();
                Some(*at)
            }))
            .collect();
        let mut edits = Vec::new();
        for (from, to, new_from, new_to) in hunks(&old, &new) {
            // A hunk the range touches; one that adds lines, where it adds them.
            if to.max(from + 1) <= first || from > last {
                continue;
            }
            let range = json!({ "start": position(&text, starts[from], self.units), "end": position(&text, starts[to], self.units) });
            edits.push(json!({ "range": range, "newText": new[new_from..new_to].concat() }));
        }
        Some(Value::Array(edits))
    }

    /// A function or method as an item of a hierarchy, or a type.
    fn item_of(&self, analyzed: &Analyzed, def: usize) -> Option<Value> {
        let index = analyzed.analysis.index.as_ref()?;
        let found = &index.defs[def];
        let kind = match found.kind {
            DefKind::Function => 12,
            DefKind::Method => 6,
            DefKind::Type => 23,
            DefKind::Enum => 10,
            DefKind::Trait => 11,
            _ => return None,
        };
        let (path, range) = self.place(analyzed, found.decl_span)?;
        let (_, selection) = self.place(analyzed, found.name_span)?;
        let uri = self.uri(&path);
        Some(
            json!({ "name": found.name, "kind": kind, "uri": uri, "range": range, "selectionRange": selection, "data": { "def": def } }),
        )
    }

    /// The root of the analysis an item of a hierarchy came from, and its
    /// declaration.
    fn item_def(&mut self, params: &Value) -> Option<(PathBuf, usize)> {
        let item = &params["item"];
        self.prepare(&json!({ "textDocument": { "uri": item["uri"] } }))?;
        let path = uri_path(item["uri"].as_str()?)?;
        Some((key_of(&path), item["data"]["def"].as_u64()? as usize))
    }

    fn prepare_hierarchy(&mut self, params: &Value, kinds: &[DefKind]) -> Option<Value> {
        self.prepare(params)?;
        let (analyzed, def, _) = self.def_at(params)?;
        let index = analyzed.analysis.index.as_ref()?;
        if !kinds.contains(&index.defs[def].kind) {
            return None;
        }
        Some(json!([self.item_of(analyzed, def)?]))
    }

    /// Where a function is called from, grouped by the function that calls.
    fn incoming_calls(&mut self, params: &Value) -> Option<Value> {
        let (root, def) = self.item_def(params)?;
        let analyzed = self.analyses.get(&root)?;
        let index = analyzed.analysis.index.as_ref()?;
        let own = index.defs[def].name_span;
        let mut by_caller: Vec<(usize, Vec<Value>)> = Vec::new();
        for &(span, used) in &index.uses {
            if used != def || span == own || index.implemented.iter().any(|&(_, s)| s == span) {
                continue;
            }
            let Some(caller) = enclosing(index, span) else { continue };
            let (_, range) = self.place(analyzed, span)?;
            match by_caller.iter_mut().find(|(c, _)| *c == caller) {
                Some((_, ranges)) => ranges.push(range),
                None => by_caller.push((caller, vec![range])),
            }
        }
        let calls: Vec<Value> = by_caller
            .into_iter()
            .filter_map(|(caller, ranges)| {
                Some(json!({ "from": self.item_of(analyzed, caller)?, "fromRanges": ranges }))
            })
            .collect();
        Some(Value::Array(calls))
    }

    /// What a function calls, grouped by the function called.
    fn outgoing_calls(&mut self, params: &Value) -> Option<Value> {
        let (root, def) = self.item_def(params)?;
        let analyzed = self.analyses.get(&root)?;
        let index = analyzed.analysis.index.as_ref()?;
        let body = index.defs[def].decl_span;
        let mut by_callee: Vec<(usize, Vec<Value>)> = Vec::new();
        for &(span, used) in &index.uses {
            let callable = matches!(index.defs[used].kind, DefKind::Function | DefKind::Method);
            if !callable || span.start < body.start || span.end > body.end || span == index.defs[def].name_span {
                continue;
            }
            let (_, range) = self.place(analyzed, span)?;
            match by_callee.iter_mut().find(|(c, _)| *c == used) {
                Some((_, ranges)) => ranges.push(range),
                None => by_callee.push((used, vec![range])),
            }
        }
        let calls: Vec<Value> = by_callee
            .into_iter()
            .filter_map(|(callee, ranges)| Some(json!({ "to": self.item_of(analyzed, callee)?, "fromRanges": ranges })))
            .collect();
        Some(Value::Array(calls))
    }

    /// The `impl`s of a trait or of a type, or the methods that implement a
    /// trait's method.
    fn implementation(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        let (analyzed, def, _) = self.def_at(params)?;
        let index = analyzed.analysis.index.as_ref()?;
        let found = &index.defs[def];
        let spans: Vec<Span> = match found.kind {
            DefKind::Trait => index.impls.iter().filter(|(t, ..)| *t == Some(def)).map(|(_, span, _)| *span).collect(),
            DefKind::Type | DefKind::Enum => {
                index.impls.iter().filter(|(.., target)| *target == found.name).map(|(_, span, _)| *span).collect()
            }
            _ => index.implemented.iter().filter(|(d, _)| *d == def).map(|(_, span)| *span).collect(),
        };
        let locations: Vec<Value> = spans.into_iter().filter_map(|span| self.location(analyzed, span)).collect();
        Some(Value::Array(locations))
    }

    /// The declaration of the type a binding, field or parameter has.
    fn type_definition(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        let (analyzed, def, _) = self.def_at(params)?;
        let index = analyzed.analysis.index.as_ref()?;
        let found = &index.defs[def];
        let named = match found.kind {
            DefKind::Type | DefKind::Enum | DefKind::Trait => found.name.clone(),
            _ => found.ty.clone()?,
        };
        // `User`, of `Array[User]` or `User?` the type inside.
        let words: Vec<&str> =
            named.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
        let declared = words.iter().rev().find_map(|word| {
            index
                .defs
                .iter()
                .find(|d| matches!(d.kind, DefKind::Type | DefKind::Enum | DefKind::Trait) && d.name == *word)
        })?;
        self.location(analyzed, declared.name_span)
    }

    /// The traits a type implements, or the types that implement a trait.
    fn type_hierarchy(&mut self, params: &Value, supertypes: bool) -> Option<Value> {
        let (root, def) = self.item_def(params)?;
        let analyzed = self.analyses.get(&root)?;
        let index = analyzed.analysis.index.as_ref()?;
        let found = &index.defs[def];
        let mut related: Vec<usize> = Vec::new();
        for (trait_def, _, target) in &index.impls {
            let other = match (supertypes, found.kind) {
                (true, DefKind::Type | DefKind::Enum) if *target == found.name => *trait_def,
                (false, DefKind::Trait) if *trait_def == Some(def) => {
                    index.defs.iter().position(|d| matches!(d.kind, DefKind::Type | DefKind::Enum) && d.name == *target)
                }
                _ => None,
            };
            if let Some(other) = other.filter(|o| !related.contains(o)) {
                related.push(other);
            }
        }
        Some(Value::Array(related.into_iter().filter_map(|d| self.item_of(analyzed, d)).collect()))
    }

    /// A folder of a project's sources renamed is its module renamed: the
    /// imports of it and of the modules inside it, and the names it is
    /// used by, change with it.
    fn will_rename_files(&mut self, params: &Value) -> Option<Value> {
        let mut changes: HashMap<String, Vec<Value>> = HashMap::new();
        for renamed in params["files"].as_array().into_iter().flatten() {
            let (Some(old), Some(new)) = (uri_path(renamed["oldUri"].as_str()?), uri_path(renamed["newUri"].as_str()?))
            else {
                continue;
            };
            let (root, project) = root_of(&old);
            if !project || !old.is_dir() {
                continue;
            }
            if self.dirty.remove(&root) || !self.analyses.contains_key(&root) {
                self.analyze(&root);
            }
            let Some(analyzed) = self.analyses.get(&root) else { continue };
            let sources = &analyzed.analysis.sources;
            // The module of the folder, from a file of it.
            let Some(module) = (0..sources.len())
                .find(|&i| {
                    sources.file(i).own && analyzed.paths[i].as_deref().and_then(Path::parent) == Some(old.as_path())
                })
                .map(|i| sources.file(i).module.clone())
            else {
                continue;
            };
            let depth = module.matches('.').count();
            let Some(source_root) = old.ancestors().nth(depth) else { continue };
            let Ok(relative) = new.strip_prefix(source_root) else { continue };
            let package = module.split('.').next().unwrap_or_default();
            let parts: Vec<String> =
                relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            if !parts.iter().all(|p| crate::project::is_identifier(p)) {
                continue;
            }
            let renamed_to = std::iter::once(package.to_string()).chain(parts).collect::<Vec<_>>().join(".");
            let (old_name, new_name) =
                (module.rsplit('.').next().unwrap_or_default(), renamed_to.rsplit('.').next().unwrap_or_default());
            for i in 0..sources.len() {
                let (file, Some(path)) = (sources.file(i), analyzed.paths[i].as_ref()) else { continue };
                if !file.own && !path.to_string_lossy().ends_with("_test.to") {
                    continue;
                }
                let uri = self.uri(path);
                let mut offset = 0;
                for line in file.text.split_inclusive('\n') {
                    if let Some(rest) = line.strip_prefix("import ")
                        && let Some(after) = rest.strip_prefix(module.as_str())
                        && (after.starts_with('.') || after.trim().is_empty() || after.starts_with(' '))
                    {
                        let start = offset + "import ".len();
                        let range = json!({ "start": position(&file.text, start, self.units), "end": position(&file.text, start + module.len(), self.units) });
                        changes.entry(uri.clone()).or_default().push(json!({ "range": range, "newText": renamed_to }));
                    }
                    offset += line.len();
                }
            }
            // The names the module is used by, where it is imported as itself.
            if let Some(index) = analyzed.analysis.index.as_ref()
                && let Some(def) = index.of(&crate::index::Key::Module(module.clone()))
                && old_name != new_name
            {
                for span in index.references(def) {
                    if sources.slice(span) == old_name
                        && let Some((path, range)) = self.place(analyzed, span)
                    {
                        changes
                            .entry(self.uri(&path))
                            .or_default()
                            .push(json!({ "range": range, "newText": new_name }));
                    }
                }
            }
        }
        Some(json!({ "changes": changes }))
    }

    /// The projects in the editor's folders, analyzed as it starts, so that
    /// their problems show before their files are opened.
    fn analyze_workspace(&mut self) {
        let mut found = Vec::new();
        for folder in std::mem::take(&mut self.folders) {
            find_projects(&folder, 0, &mut found);
        }
        self.dirty.extend(found);
    }
}

/// The function whose declaration holds `span`, the innermost.
fn enclosing(index: &Index, span: Span) -> Option<usize> {
    (0..index.defs.len())
        .filter(|&d| matches!(index.defs[d].kind, DefKind::Function | DefKind::Method))
        .filter(|&d| index.defs[d].decl_span.start <= span.start && span.end <= index.defs[d].decl_span.end)
        .min_by_key(|&d| index.defs[d].decl_span.end - index.defs[d].decl_span.start)
}

/// The directories under `dir` that are projects, a few levels down.
fn find_projects(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if dir.join("tondo.toml").is_file() || dir.join("src/main.to").is_file() {
        found.push(dir.to_path_buf());
        return;
    }
    if depth >= 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let skipped = name.starts_with('.') || ["target", "node_modules", "vendor"].contains(&name.as_str());
        if !skipped && entry.file_type().is_ok_and(|t| t.is_dir()) {
            find_projects(&entry.path(), depth + 1, found);
        }
    }
}

/// The hunks where two texts' lines differ: each one's lines in the first
/// and in the second, from one to the line after.
fn hunks(old: &[&str], new: &[&str]) -> Vec<(usize, usize, usize, usize)> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (a, b) = (&old[prefix..old.len() - suffix], &new[prefix..new.len() - suffix]);
    if a.is_empty() && b.is_empty() {
        return Vec::new();
    }
    // The longest common subsequence of the middle, unless it is too big.
    if a.len() * b.len() > 4_000_000 {
        return vec![(prefix, prefix + a.len(), prefix, prefix + b.len())];
    }
    let mut lengths = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lengths[i][j] =
                if a[i] == b[j] { lengths[i + 1][j + 1] + 1 } else { lengths[i + 1][j].max(lengths[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    let (mut from, mut new_from) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            if (i, j) != (from, new_from) {
                out.push((prefix + from, prefix + i, prefix + new_from, prefix + j));
            }
            i += 1;
            j += 1;
            (from, new_from) = (i, j);
        } else if j < b.len() && (i == a.len() || lengths[i][j + 1] >= lengths[i + 1][j]) {
            j += 1;
        } else {
            i += 1;
        }
    }
    if (i, j) != (from, new_from) {
        out.push((prefix + from, prefix + i, prefix + new_from, prefix + j));
    }
    out
}

/// The names of the parameters a function's header declares, `self` too.
fn parameter_names(header: &str) -> Vec<String> {
    let Some(open) = header.find('(') else { return Vec::new() };
    let (mut depth, mut from, mut names) = (0usize, open + 1, Vec::new());
    let mut push = |text: &str| {
        let text = text.trim();
        let text = text.strip_prefix("mut ").or(text.strip_prefix("var ")).unwrap_or(text);
        let name = text.split(':').next().unwrap_or_default().trim();
        if !name.is_empty() {
            names.push(name.to_string());
        }
    };
    for (i, c) in header.char_indices().skip(open + 1) {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' if depth == 0 => {
                push(&header[from..i]);
                break;
            }
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                push(&header[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    names
}

/// The arguments of a call, from the tokens after its `(`: each one's
/// place among them, where it starts, whether it names its parameter
/// (`name: value`), and the name it is when it is a name alone.
fn arguments(tokens: &[crate::lexer::Token]) -> Vec<(usize, u32, bool, Option<String>)> {
    let mut out = Vec::new();
    let (mut depth, mut place) = (0usize, 0);
    let mut current: Vec<&crate::lexer::Token> = Vec::new();
    let mut finish = |current: &mut Vec<&crate::lexer::Token>, place: usize| {
        if let Some(first) = current.first() {
            let named = matches!((&first.tok, current.get(1).map(|t| &t.tok)), (Tok::Ident(_), Some(Tok::Colon)));
            let alone = match current.as_slice() {
                [token] => match &token.tok {
                    Tok::Ident(name) => Some(name.clone()),
                    _ => None,
                },
                _ => None,
            };
            out.push((place, first.span.start, named, alone));
        }
        current.clear();
    };
    for token in tokens {
        match token.tok {
            Tok::Newline => continue,
            Tok::RParen if depth == 0 => {
                finish(&mut current, place);
                break;
            }
            Tok::Comma if depth == 0 => {
                finish(&mut current, place);
                place += 1;
                continue;
            }
            Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
            Tok::RParen | Tok::RBracket | Tok::RBrace => depth = depth.saturating_sub(1),
            _ => {}
        }
        current.push(token);
    }
    out
}
