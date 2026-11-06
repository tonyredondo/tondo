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
    /// What the last analysis said of each file, for a client that asks.
    latest: HashMap<PathBuf, Vec<Value>>,
    /// The client asks for diagnostics, and is told of none.
    pulling: bool,
    /// The dump the editor has open, whose values a hover shows.
    dump: Option<Value>,
    /// Where the standard modules are written, for definitions in them.
    std: Option<PathBuf>,
    /// Whether the editor asked for the tests of each project
    /// (`tondo/tests`), and the last it was sent of each.
    tests: bool,
    /// Whether the editor watches the files for it, as it asks once it
    /// starts: a change on disk, as a checkout's, is analyzed again.
    watches: bool,
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
        // The editor's answer to what the server asked, as to watch files.
        if method.is_empty() {
            return true;
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
            "textDocument/definition" | "textDocument/declaration" => {
                Some(Ok(self.definition(params).unwrap_or(Value::Null)))
            }
            "textDocument/references" => Some(Ok(self.references(params).unwrap_or(json!([])))),
            "textDocument/prepareRename" => Some(Ok(self.prepare_rename(params).unwrap_or(Value::Null))),
            "textDocument/rename" => Some(self.rename(params)),
            "textDocument/documentSymbol" => Some(Ok(self.symbols(params).unwrap_or(json!([])))),
            "textDocument/formatting" => Some(Ok(self.formatting(params).unwrap_or(Value::Null))),
            "textDocument/completion" => Some(Ok(self.completion(params).unwrap_or(json!([])))),
            "textDocument/codeAction" => Some(Ok(self.code_actions(params).unwrap_or(json!([])))),
            "textDocument/documentHighlight" => Some(Ok(self.highlights(params).unwrap_or(json!([])))),
            "textDocument/linkedEditingRange" => Some(Ok(self.linked_names(params).unwrap_or(Value::Null))),
            "textDocument/diagnostic" => Some(Ok(self.pulled(params))),
            "workspace/symbol" => Some(Ok(self.workspace_symbols(params))),
            "textDocument/signatureHelp" => Some(Ok(self.signature_help(params).unwrap_or(Value::Null))),
            "textDocument/inlayHint" => Some(Ok(self.inlay_hints(params).unwrap_or(json!([])))),
            "textDocument/codeLens" => {
                let mut lenses = self.lenses(params).and_then(|l| l.as_array().cloned()).unwrap_or_default();
                lenses.extend(self.test_lenses(params).unwrap_or_default());
                lenses.extend(self.tested_by(params).unwrap_or_default());
                lenses.extend(self.reference_lenses(params).unwrap_or_default());
                Some(Ok(Value::Array(lenses)))
            }
            "textDocument/foldingRange" => Some(Ok(self.folding_ranges(params).unwrap_or(json!([])))),
            "textDocument/documentColor" => Some(Ok(self.colors(params).unwrap_or(json!([])))),
            "textDocument/documentLink" => Some(Ok(self.links(params).unwrap_or(json!([])))),
            "textDocument/colorPresentation" => Some(Ok(self.color_presentation(params).unwrap_or(json!([])))),
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
            "textDocument/onTypeFormatting" => Some(Ok(self.on_type_formatting(params).unwrap_or(json!([])))),
            "workspace/executeCommand" => Some(Ok(self.execute(params).unwrap_or(Value::Null))),
            "initialized" => {
                if self.watches {
                    let watchers: Vec<Value> = ["**/*.to", "**/tondo.toml", "**/tondo.lock"]
                        .iter()
                        .map(|glob| json!({ "globPattern": glob }))
                        .collect();
                    send(json!({ "jsonrpc": "2.0", "id": "watch", "method": "client/registerCapability",
                        "params": { "registrations": [{ "id": "watch", "method": "workspace/didChangeWatchedFiles",
                            "registerOptions": { "watchers": watchers } }] } }));
                }
                self.analyze_workspace();
                None
            }
            // Files that changed on disk, not in the editor: their projects
            // are analyzed again.
            "workspace/didChangeWatchedFiles" => {
                for change in params["changes"].as_array().into_iter().flatten() {
                    if let Some(path) = change["uri"].as_str().and_then(uri_path) {
                        self.dirty.insert(root_of(&path).0);
                    }
                }
                None
            }
            "textDocument/semanticTokens/full" | "textDocument/semanticTokens/range" => {
                Some(Ok(self.semantic_tokens(params).unwrap_or(json!({ "data": [] }))))
            }
            "textDocument/inlineValue" => Some(Ok(self.inline_values(params).unwrap_or(json!([])))),
            _ if request => Some(Err((-32601, format!("`{method}` is not supported")))),
            _ => None,
        }
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let encodings = &params["capabilities"]["general"]["positionEncodings"];
        let utf8 = encodings.as_array().is_some_and(|e| e.iter().any(|e| e == "utf-8"));
        self.units = if utf8 { Units::Utf8 } else { Units::Utf16 };
        self.tests = params["initializationOptions"]["tests"] == true;
        self.pulling = params["initializationOptions"]["pullDiagnostics"] == true;
        self.watches = params["capabilities"]["workspace"]["didChangeWatchedFiles"]["dynamicRegistration"] == true;
        let folders = params["workspaceFolders"].as_array().into_iter().flatten().filter_map(|f| f["uri"].as_str());
        self.folders = folders.chain(params["rootUri"].as_str()).filter_map(uri_path).collect();
        self.folders.dedup();
        let mut answer = json!({
            "capabilities": {
                "positionEncoding": if utf8 { "utf-8" } else { "utf-16" },
                "textDocumentSync": { "openClose": true, "change": 1 },
                "hoverProvider": true,
                "definitionProvider": true,
                "declarationProvider": true,
                "inlineValueProvider": true,
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
                "colorProvider": true,
                "documentLinkProvider": { "resolveProvider": false },
                "documentOnTypeFormattingProvider": { "firstTriggerCharacter": "}" },
                "executeCommandProvider": { "commands": [
                    "tondo.runTest", "tondo.signature", "tondo.moveTo", "tondo.modules", "tondo.useDump", "tondo.group", "tondo.trait", "tondo.toMethod", "tondo.toFunction", "tondo.inlineAlias", "tondo.ifMatch",
                ] },
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
                    "legend": { "tokenTypes": TOKEN_TYPES, "tokenModifiers": ["declaration", "readonly", "defaultLibrary", "mutable"] },
                    "full": true,
                    "range": true,
                },
            },
            "serverInfo": { "name": "tondo", "version": driver::VERSION },
        });
        answer["capabilities"]["linkedEditingRangeProvider"] = json!(true);
        if self.pulling {
            let provider = json!({ "interFileDependencies": true, "workspaceDiagnostics": false });
            answer["capabilities"]["diagnosticProvider"] = provider;
        }
        answer
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

    /// *Run test* over each test and suite of a file, for an editor that
    /// does not run them itself from `tondo/tests`, as VS Code does.
    fn test_lenses(&mut self, params: &Value) -> Option<Vec<Value>> {
        if self.tests {
            return None;
        }
        self.prepare(params)?;
        let uri = params["textDocument"]["uri"].as_str()?;
        let path = uri_path(uri)?;
        let analyzed = self.analyses.get(&key_of(&path))?;
        let file = analyzed.paths.iter().position(|p| p.as_deref() == Some(path.as_path()))?;
        let source = analyzed.analysis.sources.file(file);
        let inside = |span: Span| span.start >= source.base && span.end <= source.base + source.text.len() as u32;
        let nodes = analyzed.tests.as_ref()?;
        let lenses = nodes.iter().skip(1).filter(|node| inside(node.span)).filter_map(|node| {
            let (_, range) = self.place(analyzed, node.span)?;
            let title = if node.suite { "Run suite" } else { "Run test" };
            let arguments = json!([uri, node.id, node.suite]);
            Some(json!({ "range": range, "command": { "title": title, "command": "tondo.runTest", "arguments": arguments } }))
        });
        Some(lenses.collect())
    }

    /// Over each function of a project, the tests that reach it, which the
    /// lens runs.
    fn tested_by(&mut self, params: &Value) -> Option<Vec<Value>> {
        self.prepare(params)?;
        let uri = params["textDocument"]["uri"].as_str()?;
        let path = uri_path(uri)?;
        let analyzed = self.analyses.get(&key_of(&path))?;
        let file = analyzed.paths.iter().position(|p| p.as_deref() == Some(path.as_path()))?;
        let source = analyzed.analysis.sources.file(file);
        let index = analyzed.analysis.index.as_ref()?;
        let tests = crate::affected::reaching(index, analyzed.tests.as_ref()?);
        let inside = |span: Span| span.start >= source.base && span.end <= source.base + source.text.len() as u32;
        let functions = index.defs.iter().enumerate().filter(|(_, declared)| {
            matches!(declared.kind, DefKind::Function | DefKind::Method) && inside(declared.name_span)
        });
        let lenses = functions.filter_map(|(def, declared)| {
            let ids: Vec<&str> =
                tests.iter().filter(|(_, reached)| reached.contains(&def)).map(|(node, _)| node.id.as_str()).collect();
            let (_, range) = self.place(analyzed, declared.name_span).filter(|_| !ids.is_empty())?;
            let title = if ids.len() == 1 { "1 test reaches this".to_string() } else { format!("{} tests reach this", ids.len()) };
            let arguments = json!([uri, ids, false]);
            Some(json!({ "range": range, "command": { "title": title, "command": "tondo.runTest", "arguments": arguments } }))
        });
        Some(lenses.collect())
    }

    /// A command a lens gives: `tondo.runTest` runs a test or a suite of the
    /// file's project with `tondo test`, apart from the server, and says how
    /// it went.
    fn execute(&mut self, params: &Value) -> Option<Value> {
        let arguments = &params["arguments"];
        let answer = |result: Result<Value, String>| match result {
            Ok(edit) => json!({ "edit": edit }),
            Err(message) => json!({ "error": message }),
        };
        match params["command"].as_str()? {
            "tondo.runTest" => {}
            "tondo.signature" => return Some(answer(self.change_signature(arguments))),
            "tondo.moveTo" => return Some(answer(self.move_to(arguments))),
            "tondo.group" => return Some(answer(self.group_parameters(arguments))),
            "tondo.trait" => return Some(answer(self.extract_trait(arguments))),
            "tondo.toMethod" => return Some(answer(self.function_as_method(arguments))),
            "tondo.toFunction" => return Some(answer(self.method_as_function(arguments))),
            "tondo.inlineAlias" => return Some(answer(self.inline_alias(arguments))),
            "tondo.ifMatch" => return Some(answer(self.if_match(arguments))),
            "tondo.modules" => return self.own_modules(arguments),
            "tondo.useDump" => {
                let file = arguments[0].as_str().map(Path::new);
                self.dump = file.and_then(|file| crate::observe::load(file).ok());
                return Some(Value::Null);
            }
            _ => return None,
        }
        let path = uri_path(arguments[0].as_str()?)?;
        let ids: Vec<String> = match arguments[1].as_array() {
            Some(ids) => ids.iter().filter_map(|id| id.as_str().map(str::to_string)).collect(),
            None => vec![arguments[1].as_str()?.to_string()],
        };
        let id = ids.join(", ");
        let suite = arguments[2].as_bool().unwrap_or(false);
        let (root, _) = root_of(&path);
        let tondo = std::env::current_exe().ok()?;
        std::thread::spawn(move || {
            let chosen: Vec<String> = match suite {
                true => vec!["--filter".to_string(), format!("{id}::")],
                false => ids.into_iter().flat_map(|id| ["--exact".to_string(), id]).collect(),
            };
            let ran = std::process::Command::new(tondo).arg("test").args(chosen).current_dir(&root).output();
            let (kind, message) = match ran {
                Ok(ran) if ran.status.success() => (3, format!("`{id}` passed")),
                Ok(ran) => {
                    let output =
                        format!("{}{}", String::from_utf8_lossy(&ran.stdout), String::from_utf8_lossy(&ran.stderr));
                    send(
                        json!({ "jsonrpc": "2.0", "method": "window/logMessage", "params": { "type": 3, "message": output } }),
                    );
                    // Why: the panic or the error, as the report writes it.
                    let why = output.lines().find(|l| l.starts_with("panic[") || l.starts_with("error"));
                    (1, format!("`{id}` failed: {}", why.unwrap_or("see the output").trim()))
                }
                Err(error) => (1, format!("`{id}` did not run: {error}")),
            };
            send(
                json!({ "jsonrpc": "2.0", "method": "window/showMessage", "params": { "type": kind, "message": message } }),
            );
        });
        Some(Value::Null)
    }

    /// On `}`, the block it closes in the canonical layout, when the file
    /// reads.
    fn on_type_formatting(&self, params: &Value) -> Option<Value> {
        let (_, text) = self.document(params)?;
        let at = offset(&text, &params["position"], self.units)?;
        let close = text[..at].rfind('}')?;
        // The `{` the brace closes, by counting from it back.
        let mut depth = 0;
        let open = text[..close].char_indices().rev().find_map(|(i, c)| match c {
            '}' => {
                depth += 1;
                None
            }
            '{' if depth == 0 => Some(i),
            '{' => {
                depth -= 1;
                None
            }
            _ => None,
        })?;
        let first = text[..open].matches('\n').count();
        let last = text[..close].matches('\n').count();
        let range = json!({ "start": { "line": first, "character": 0 }, "end": { "line": last, "character": 0 } });
        self.range_formatting(&json!({ "textDocument": params["textDocument"], "range": range }))
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

    /// Over each function, method, type and trait of a file: how many
    /// places refer to it and, for a trait, how many types implement it,
    /// which open them as the editor shows references.
    fn reference_lenses(&mut self, params: &Value) -> Option<Vec<Value>> {
        self.prepare(params)?;
        let uri = params["textDocument"]["uri"].as_str()?;
        let path = uri_path(uri)?;
        let analyzed = self.analyses.get(&key_of(&path))?;
        let index = analyzed.analysis.index.as_ref()?;
        let file = analyzed.paths.iter().position(|p| p.as_deref() == Some(path.as_path()))?;
        let source = analyzed.analysis.sources.file(file);
        let inside = |span: Span| span.start >= source.base && span.end <= source.base + source.text.len() as u32;
        let mut uses: HashMap<usize, Vec<Span>> = HashMap::new();
        for &(span, def) in &index.uses {
            if span != index.defs[def].name_span {
                uses.entry(def).or_default().push(span);
            }
        }
        let kinds = [DefKind::Function, DefKind::Method, DefKind::Type, DefKind::Enum, DefKind::Trait];
        let mut lenses = Vec::new();
        for (id, def) in index.defs.iter().enumerate() {
            if !kinds.contains(&def.kind) || !inside(def.name_span) || def.name == "main" {
                continue;
            }
            let (_, range) = self.place(analyzed, def.name_span)?;
            let mut lens = |title: String, spans: Vec<Span>| {
                let mut spans = spans;
                spans.sort_by_key(|span| (span.start, span.end));
                spans.dedup();
                let locations: Vec<Value> = spans.iter().filter_map(|&span| self.location(analyzed, span)).collect();
                let command = json!({ "title": title, "command": "tondo.showReferences", "arguments": [uri, range["start"], locations] });
                lenses.push(json!({ "range": range, "command": command }));
            };
            let found = uses.get(&id).cloned().unwrap_or_default();
            let count = found.iter().collect::<HashSet<_>>().len();
            lens(if count == 1 { "1 reference".into() } else { format!("{count} references") }, found);
            if def.kind == DefKind::Trait {
                let impls: Vec<Span> =
                    index.impls.iter().filter(|(d, ..)| *d == Some(id)).map(|(_, span, _)| *span).collect();
                if !impls.is_empty() {
                    let count = impls.len();
                    lens(
                        if count == 1 { "1 implementation".into() } else { format!("{count} implementations") },
                        impls,
                    );
                }
            }
        }
        Some(lenses)
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
            // The other places it concerns, as the declaration it repeats.
            let related: Vec<Value> = diagnostic
                .related
                .iter()
                .filter_map(|(span, message)| {
                    Some(json!({ "location": self.location(analyzed, *span)?, "message": message }))
                })
                .collect();
            if !related.is_empty() {
                published["relatedInformation"] = json!(related);
            }
            // What nothing uses or nothing reaches, which editors fade.
            if matches!(code, "W1001" | "W1002" | "W1003" | "W1006" | "W1007") {
                published["tags"] = json!([1]);
            }
            // What is on its way out, which editors strike through.
            if code == "W1015" {
                published["tags"] = json!([2]);
            }
            // The code opens what it means (`tondo explain`).
            if crate::explain::CODES.iter().any(|(c, ..)| *c == code) {
                published["codeDescription"] = json!({ "href": format!("tondo-explain:{code}") });
            }
            by_file.entry(path).or_default().push(published);
        }
        // What is `pub` for nothing, which only the editor says: a library's
        // are for whoever uses it.
        for (span, message) in unused_public(analyzed) {
            let Some((path, range)) = self.place(analyzed, span) else { continue };
            let hint = json!({ "range": range, "severity": 4, "tags": [1], "code": "H1001", "source": "tondo", "message": message });
            by_file.entry(path).or_default().push(hint);
        }
        // What neither `main` nor a test reaches, as far as names say.
        if let (Some(index), Some(tests)) = (analyzed.analysis.index.as_ref(), analyzed.tests.as_ref()) {
            for def in crate::affected::unreached(index, &analyzed.analysis.sources, tests) {
                let declared = &index.defs[def];
                let Some((path, range)) = self.place(analyzed, declared.name_span) else { continue };
                let message = format!("neither `main` nor a test reaches `{}`", declared.name);
                let hint = json!({ "range": range, "severity": 4, "tags": [1], "code": "H1002", "source": "tondo", "message": message });
                by_file.entry(path).or_default().push(hint);
            }
        }
        let before = self.published.remove(root).unwrap_or_default();
        for path in before.iter().filter(|path| !by_file.contains_key(*path)) {
            self.latest.remove(path);
            let uri = self.uri(path);
            if !self.pulling {
                send(json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                    "params": { "uri": uri, "diagnostics": [] } }));
            }
        }
        for (path, diagnostics) in &by_file {
            self.latest.insert(path.clone(), diagnostics.clone());
            let uri = self.uri(path);
            if self.pulling {
                continue;
            }
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
                // The modules the compiler implements too, by what `tondo
                // api` shows of them.
                let _ = std::fs::create_dir_all(dir.join("api"));
                let intrinsic = crate::api::intrinsic_files().map(|(name, text)| (format!("api/{name}"), text));
                for (name, text) in crate::stdlib::sources().iter().map(|&(n, t)| (n.to_string(), t)).chain(intrinsic) {
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

    /// What a hover says, and what the dump the editor has open held in the
    /// binding it points at.
    fn hover(&mut self, params: &Value) -> Option<Value> {
        let mut found = self.described(params)?;
        if let Some(values) = self.dumped_at(params).filter(|values| !values.is_empty())
            && let Some(text) = found["contents"]["value"].as_str()
        {
            let lines: Vec<String> = values.iter().take(5).map(|value| format!("- {value}")).collect();
            found["contents"]["value"] = json!(format!("{text}\n\nIn the dump:\n{}", lines.join("\n")));
        }
        if let Some(ways) = self.failures_at(params).filter(|ways| !ways.is_empty())
            && let Some(text) = found["contents"]["value"].as_str()
        {
            let lines: Vec<String> = ways.iter().take(8).map(|way| format!("- {way}")).collect();
            let more = if ways.len() > 8 { format!("\n- and {} more", ways.len() - 8) } else { String::new() };
            found["contents"]["value"] = json!(format!("{text}\n\nCan fail:\n{}{more}", lines.join("\n")));
        }
        if let Some(ways) = self.waits_at(params).filter(|ways| !ways.is_empty())
            && let Some(text) = found["contents"]["value"].as_str()
        {
            let lines: Vec<String> = ways.iter().take(8).map(|way| format!("- {way}")).collect();
            let more = if ways.len() > 8 { format!("\n- and {} more", ways.len() - 8) } else { String::new() };
            found["contents"]["value"] = json!(format!("{text}\n\nCan wait:\n{}{more}", lines.join("\n")));
        }
        Some(found)
    }

    /// Where the function a request points at can wait, in its own code or
    /// in what it calls: where a cancelled task stops, and where a `defer`
    /// of one ends.
    fn waits_at(&self, params: &Value) -> Option<Vec<String>> {
        self.ways_at(params, false, |first, second| match (first, second) {
            (Tok::Dot, Tok::Ident(name)) => match name.as_str() {
                "receive" => Some("waits for a value"),
                "send" => Some("waits for room in a channel"),
                "wait" => Some("waits for a task"),
                "sleep" => Some("waits for time"),
                "update" | "updateWhen" => Some("waits for a lock"),
                "acquire" => Some("waits for a permit"),
                _ => None,
            },
            _ => None,
        })
    }

    /// How the function a request points at can fail, in its own code or in
    /// what it calls of the project's: where it fails, propagates or panics.
    fn failures_at(&self, params: &Value) -> Option<Vec<String>> {
        self.ways_at(params, true, |first, second| match (first, second) {
            (Tok::Ident(name), Tok::LParen) if name == "panic" => Some("panics"),
            (Tok::Dot, Tok::Ident(name)) if name == "unwrap" => Some("panics on `none`"),
            _ => None,
        })
    }

    /// What the function a request points at does, in its own code or in
    /// what it calls: its exits, if asked, and the pairs of tokens `word` names.
    fn ways_at(
        &self,
        params: &Value,
        exits: bool,
        word: fn(&Tok, &Tok) -> Option<&'static str>,
    ) -> Option<Vec<String>> {
        let (analyzed, def, _) = self.def_at(params)?;
        let index = analyzed.analysis.index.as_ref()?;
        if !matches!(index.defs[def].kind, DefKind::Function | DefKind::Method) {
            return None;
        }
        let sources = &analyzed.analysis.sources;
        let (mut seen, mut left, mut ways) = (HashSet::new(), vec![(def, 0)], Vec::new());
        while let Some((function, depth)) = left.pop() {
            if !seen.insert(function) || seen.len() > 50 {
                continue;
            }
            let declared = &index.defs[function];
            let file = sources.file_of(declared.name_span.start);
            if !file.own {
                continue;
            }
            let span = declared.decl_span;
            let via = if function == def { String::new() } else { format!(" in `{}`", declared.name) };
            let mut way = |what: &str, at: u32| {
                let (_, line, _) = sources.position(at);
                ways.push(format!("{what}{via} at {}:{line}", file.logical));
            };
            let own = index.exits.iter().filter(|(name, _)| exits && *name == declared.name_span);
            for &(_, exit) in own {
                let text = &file.text[(exit.start - file.base) as usize..(exit.end - file.base) as usize];
                if text.starts_with("fail") {
                    way("fails", exit.start);
                } else if text.ends_with('?') {
                    way("passes on a failure", exit.end - 1);
                }
            }
            let tokens = crate::lexer::lex(file, Some(span), &mut crate::diag::Diagnostics::default());
            for pair in tokens.windows(2) {
                if let Some(what) = word(&pair[0].tok, &pair[1].tok) {
                    way(what, pair[1].span.start);
                }
            }
            for &(used, target) in
                index.uses.iter().filter(|(used, _)| span.start <= used.start && used.end <= span.end)
            {
                let callable = matches!(index.defs[target].kind, DefKind::Function | DefKind::Method);
                if callable && target != function && used != declared.name_span && depth < 3 {
                    left.push((target, depth + 1));
                }
            }
        }
        ways.sort();
        ways.dedup();
        Some(ways)
    }

    fn dumped_at(&self, params: &Value) -> Option<Vec<String>> {
        let dump = self.dump.as_ref()?;
        let (analyzed, def, span) = self.def_at(params)?;
        let declared = &analyzed.analysis.index.as_ref()?.defs[def];
        if !matches!(declared.kind, DefKind::Local | DefKind::Parameter) {
            return None;
        }
        let file = analyzed.analysis.sources.file_of(span.start);
        Some(crate::observe::dumped_bindings(dump, &file.logical, &declared.name))
    }

    fn described(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        if let Some((analyzed, module, _, span)) = self.imported(params) {
            let (_, range) = self.place(analyzed, span)?;
            return Some(json!({ "contents": { "kind": "markdown", "value": module_hover(&module) }, "range": range }));
        }
        // What a `?` or a `fail` does there.
        if let Some((analyzed, _, offset)) = self.at(params)
            && let Some(index) = analyzed.analysis.index.as_ref()
            && let Some((span, note)) = index.notes.iter().find(|(s, _)| s.start <= offset && offset < s.end)
        {
            let (_, range) = self.place(analyzed, *span)?;
            return Some(json!({ "contents": { "kind": "markdown", "value": note }, "range": range }));
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
        let mut value = match def.kind {
            DefKind::Module => module_hover(&file.module),
            _ => format!("```tondo\n{header}\n```"),
        };
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
        let start = after_last(&text[..at], |c: char| !is_name(c));
        let end = text[at..].find(|c: char| !is_name(c)).map_or(text.len(), |i| at + i);
        let name = &text[start..end];
        let (header, docs) = match self.builtin(analyzed, file, start, name) {
            Some(found) => found,
            // A word of the language: what it does, and where the guide
            // says more.
            None => {
                let (_, about) = KEYWORD_DOCS.iter().find(|(word, _)| *word == name)?;
                (name.to_string(), about.to_string())
            }
        };
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
        let dot = text[..start].ends_with('.').then(|| start - 1)?;
        let receiver_start = after_last(&text[..dot], |c: char| !(c.is_alphanumeric() || c == '_'));
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

    /// A binding's name everywhere in its function, to edit as one: what
    /// other files cannot name.
    fn linked_names(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        let (analyzed, file, _) = self.at(params)?;
        let (_, def, _) = self.def_at(params)?;
        let index = analyzed.analysis.index.as_ref()?;
        if !matches!(index.defs[def].kind, DefKind::Local | DefKind::Parameter) {
            return None;
        }
        let source = analyzed.analysis.sources.file(file);
        let ranges: Vec<Value> = index.references(def).into_iter().map(|span| self.range(source, span)).collect();
        Some(json!({ "ranges": ranges }))
    }

    /// What is wrong with a file, for a client that asks rather than hears.
    fn pulled(&mut self, params: &Value) -> Value {
        self.prepare(params);
        let path = params["textDocument"]["uri"].as_str().and_then(uri_path);
        let items = path.and_then(|path| self.latest.get(&path).cloned()).unwrap_or_default();
        json!({ "kind": "full", "items": items })
    }

    fn highlights(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        if let Some(exits) = self.exits(params) {
            return Some(exits);
        }
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

    /// On `fn`, `return`, `fail` or a `?`: every place the function may
    /// leave.
    fn exits(&self, params: &Value) -> Option<Value> {
        let (analyzed, file, offset) = self.at(params)?;
        let index = analyzed.analysis.index.as_ref()?;
        let source = analyzed.analysis.sources.file(file);
        let at = (offset - source.base) as usize;
        let text = &source.text;
        let word_start = after_last(&text[..at], |c: char| !(c.is_alphanumeric() || c == '_'));
        let word_end = text[at..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(text.len(), |i| at + i);
        let word = &text[word_start..word_end];
        let question = text[at..].starts_with('?') || text[..at].ends_with('?');
        if !matches!(word, "fn" | "return" | "fail") && !question {
            return None;
        }
        let function = index
            .defs
            .iter()
            .filter(|d| matches!(d.kind, DefKind::Function | DefKind::Method))
            .filter(|d| d.decl_span.start <= offset && offset <= d.decl_span.end)
            .min_by_key(|d| d.decl_span.end - d.decl_span.start)?;
        let exits: Vec<Value> = index
            .exits
            .iter()
            .filter(|(name, _)| *name == function.name_span)
            .map(|&(_, span)| json!({ "range": self.range(source, span), "kind": 1 }))
            .collect();
        (!exits.is_empty()).then_some(Value::Array(exits))
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
                let Some(score) = matched(&def.name, &query) else { continue };
                let Some((path, range)) = self.place(analyzed, def.name_span) else { continue };
                if std.as_ref().is_some_and(|std| path.starts_with(std)) {
                    continue;
                }
                let symbol =
                    json!({ "name": def.name, "kind": kind, "location": { "uri": self.uri(&path), "range": range } });
                symbols.push((score, symbol));
            }
        }
        symbols.extend(self.std_symbols(&query));
        // The closest matches first: the same name, its start, a part of
        // it, then its letters in order.
        symbols.sort_by_key(|(score, _)| *score);
        Value::Array(symbols.into_iter().take(500).map(|(_, symbol)| symbol).collect())
    }

    /// The public declarations of the standard library whose names hold
    /// `query`, each with its module, where the editor opens them: the
    /// modules written in Tondo, with the parts they are made of, and those
    /// the compiler implements.
    fn std_symbols(&mut self, query: &str) -> Vec<(u8, Value)> {
        let Some(dir) = self.std_dir() else { return Vec::new() };
        let mut found = Vec::new();
        for module in crate::api::standard_modules() {
            let name = module.strip_prefix("std.").unwrap_or(&module).to_string();
            let mut files: Vec<String> = match crate::stdlib::text(&name) {
                Some(_) => vec![name.clone()],
                None => vec![format!("api/{name}")],
            };
            files.extend(crate::stdlib::companions(&name).iter().map(|c| c.to_string()));
            if name == "prelude" {
                files.push("api/builtins".into());
            }
            for file in files {
                let path = dir.join(format!("{file}.to"));
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                for (line, code) in text.lines().enumerate() {
                    let Some(rest) = code.strip_prefix("pub ") else { continue };
                    let (word, rest) = rest.split_once(' ').unwrap_or((rest, ""));
                    let declared: String =
                        rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.').collect();
                    let kind = match word {
                        "fn" if declared.contains('.') => 6,
                        "fn" => 12,
                        "type" | "alias" | "context" => 23,
                        "enum" => 10,
                        "trait" => 11,
                        "const" => 14,
                        _ => continue,
                    };
                    let Some(score) = matched(&declared, query).filter(|_| !declared.is_empty()) else { continue };
                    let column = code.find(&declared).unwrap_or(0);
                    let range = json!({
                        "start": { "line": line, "character": column },
                        "end": { "line": line, "character": column + declared.len() },
                    });
                    found.push((
                        score,
                        json!({
                            "name": declared, "kind": kind, "containerName": module,
                            "location": { "uri": file_uri(&path), "range": range },
                        }),
                    ));
                }
            }
        }
        found
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
        let name_start = after_last(&text[..open], |c: char| !is_name(c));
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
        // The type of each binding written without one: a `let`, and a
        // closure's parameter.
        for def in &index.defs {
            let (Some(ty), DefKind::Local | DefKind::Parameter) = (&def.ty, def.kind) else { continue };
            let span = def.name_span;
            if def.name == "_" || def.name == "self" || span.start < from.max(source.base) || span.end > to {
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
        // The type of each line of a chain of calls written over several.
        let text = &source.text;
        let mut line_start = 0;
        for line in text.split_inclusive('\n') {
            let line_end = line_start + line.trim_end().len();
            let next = &text[line_start + line.len()..];
            let continued =
                next.lines().find(|l| !l.trim().is_empty()).is_some_and(|l| l.trim_start().starts_with('.'));
            let at = source.base + line_end as u32;
            if continued && !line.trim().is_empty() && at >= from && at <= to {
                let step = index.types.iter().filter(|(span, _)| span.end == at).min_by_key(|(span, _)| span.start);
                if let Some(&(_, shape)) = step {
                    let position = position(text, line_end, self.units);
                    let label = format!(": {}", index.shapes[shape].name);
                    hints.push(json!({ "position": position, "label": label, "kind": 1, "paddingLeft": true }));
                }
            }
            line_start += line.len();
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
        let start = after_last(&text[..cursor], |c: char| !(c.is_alphanumeric() || c == '_'));
        // After `@`, the program's annotations; in their arguments, their fields.
        if text[..start].ends_with('@') {
            return Some(json!({ "isIncomplete": false, "items": self.annotation_names(latest, offset) }));
        }
        if let Some(items) = self.annotation_fields(latest, offset, &text[line..cursor]) {
            return Some(json!({ "isIncomplete": false, "items": items }));
        }
        if !text[..start].ends_with('.') {
            let mut items = self.names(latest, offset);
            items.extend(self.unimported(latest, offset, &text[start..cursor]));
            // In a record's braces, its fields not written yet; where a
            // call's argument starts, its parameters' names.
            let (before, written) = (text[..start].trim_end(), &text[..cursor]);
            let mut first = self.record_fields(latest, &text, start).unwrap_or_default();
            if first.is_empty() && before.ends_with(['(', ',']) {
                first = self.named_arguments(params, written).unwrap_or_default();
            }
            let declaring = expected_type(&text[line..start]);
            let expected = declaring.clone().map(|(_, ty)| ty).or_else(|| self.argument_type(params));
            if let Some(expected) = expected {
                for item in items.iter_mut() {
                    // Not the binding whose value this is.
                    if declaring.as_ref().is_some_and(|(name, _)| item["label"] == name.as_str()) {
                        continue;
                    }
                    let detail = item["detail"].as_str().unwrap_or_default();
                    let gives = if detail.starts_with("fn ") || detail.starts_with("pub fn ") {
                        returns(detail)
                    } else {
                        detail.trim()
                    };
                    if gives == expected {
                        let label = item["label"].as_str().unwrap_or_default().to_string();
                        item["sortText"] = json!(format!("!{label}"));
                        item["preselect"] = json!(true);
                    }
                }
            }
            first.extend(items);
            items = first;
            return Some(json!({ "isIncomplete": false, "items": items }));
        }
        let end = start - 1;
        // A module the file imports, a type, or a type of a module: what it
        // declares. Otherwise a value, whose members follow.
        let chain_start = after_last(&text[..end], |c: char| !(c.is_alphanumeric() || c == '_' || c == '.'));
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
        let mut items = self.members(&analyzed, base + end as u32, &here, call).unwrap_or_default();
        items.extend(self.postfix(&analyzed, &text, base, end, cursor));
        Some(json!({ "isIncomplete": false, "items": items }))
    }

    /// After a value's dot, what turns it into a statement or an
    /// expression around it: `.match`, `.if`, `.for`, `.let`...
    fn postfix(&self, analyzed: &Analyzed, text: &str, base: u32, end: usize, cursor: usize) -> Vec<Value> {
        let Some(index) = analyzed.analysis.index.as_ref() else { return Vec::new() };
        let at = base + end as u32;
        let receiver =
            index.types.iter().filter(|(span, _)| span.end == at).min_by_key(|(span, _)| span.end - span.start);
        let Some(&(span, shape)) = receiver else { return Vec::new() };
        let ty = index.shapes[shape].name.as_str();
        let start = (span.start - base) as usize;
        let value = &text[start..end];
        let escaped = value.replace('\\', "\\\\").replace('$', "\\$").replace('}', "\\}");
        let boolean = ty == "Bool";
        let iterable = ["Array[", "Map[", "Set[", "Deque[", "Range["].iter().any(|p| ty.starts_with(p));
        let name = value_name(value);
        let templates = [
            ("if", format!("if {escaped} {{\n\t$0\n}}"), boolean),
            ("not", format!("not {escaped}"), boolean),
            ("match", format!("match {escaped} {{\n\t$0\n}}"), true),
            ("for", format!("for ${{1:item}} in {escaped} {{\n\t$0\n}}"), iterable),
            ("let", format!("let ${{1:{name}}} = {escaped}"), true),
            ("some", format!("some({escaped})"), true),
            ("ok", format!("ok({escaped})"), true),
        ];
        let range = json!({ "start": position(text, start, self.units), "end": position(text, cursor, self.units) });
        templates
            .into_iter()
            .filter(|(_, _, applies)| *applies)
            .map(|(label, snippet, _)| {
                json!({
                    "label": label,
                    "kind": 15,
                    "detail": format!("{label} {value}"),
                    "filterText": format!("{value}.{label}"),
                    "sortText": format!("~{label}"),
                    "insertTextFormat": 2,
                    "textEdit": { "range": range, "newText": snippet },
                })
            })
            .collect()
    }

    /// The type of the parameter whose argument the cursor is in.
    fn argument_type(&mut self, params: &Value) -> Option<String> {
        let help = self.signature_help(params)?;
        let signature = &help["signatures"][0];
        let active = help["activeParameter"].as_u64().or(signature["activeParameter"].as_u64())? as usize;
        let label = match &signature["parameters"][active]["label"] {
            Value::String(label) => label.clone(),
            Value::Array(range) => {
                let (from, to) = (range[0].as_u64()? as usize, range[1].as_u64()? as usize);
                signature["label"].as_str()?.chars().skip(from).take(to - from).collect()
            }
            _ => return None,
        };
        let ty = label.split_once(':')?.1;
        Some(ty.split(" = ").next()?.trim().to_string())
    }

    /// What other modules declare whose names start with what is written,
    /// as `uuid.Uuid` for `Uui`, with the import the file lacks: the
    /// standard modules' and the project's own.
    fn unimported(&self, analyzed: &Analyzed, offset: u32, typed: &str) -> Vec<Value> {
        if typed.chars().count() < 2 {
            return Vec::new();
        }
        let sources = &analyzed.analysis.sources;
        let here = sources.file_of(offset);
        let imported = imports(&here.text);
        let mut after = 0;
        let mut at = 0;
        for line in here.text.split_inclusive('\n') {
            at += line.len();
            if line.starts_with("import ") {
                after = at;
            }
        }
        let place = position(&here.text, after, self.units);
        let lower = typed.to_lowercase();
        let mut items = Vec::new();
        let mut offer = |name: &str, module: &str, kind: u64, detail: &str| {
            let alias = imported.iter().find(|(_, m)| m.as_str() == module).map(|(a, _)| a.clone());
            let shown = alias.clone().unwrap_or_else(|| module.rsplit('.').next().unwrap_or(module).to_string());
            let mut item = json!({
                "label": name,
                "kind": kind,
                "labelDetails": { "description": module },
                "detail": detail,
                "filterText": name,
                "insertText": format!("{shown}.{name}"),
                "sortText": format!("~~{name}"),
            });
            if alias.is_none() {
                let edit =
                    json!({ "range": { "start": place, "end": place }, "newText": format!("import {module}\n") });
                item["additionalTextEdits"] = json!([edit]);
            }
            items.push(item);
        };
        if let Some(index) = analyzed.analysis.index.as_ref() {
            for def in &index.defs {
                let kind = match def.kind {
                    DefKind::Function => 3,
                    DefKind::Type => 22,
                    DefKind::Enum => 13,
                    DefKind::Trait => 8,
                    DefKind::Const => 21,
                    _ => continue,
                };
                let file = sources.file_of(def.decl_span.start);
                let text = file.slice(def.decl_span);
                let public = text.starts_with("pub ");
                if !file.own || file.module == here.module || !public || !def.name.to_lowercase().starts_with(&lower) {
                    continue;
                }
                offer(&def.name, &file.module, kind, text.lines().next().unwrap_or_default().trim_end_matches(" {"));
            }
        }
        for declared in crate::api::standard_declarations() {
            if declared.name.to_lowercase().starts_with(&lower) {
                let kind = match declared.kind.as_str() {
                    "fn" => 3,
                    "enum" => 13,
                    "trait" => 8,
                    "const" => 21,
                    _ => 22,
                };
                offer(&declared.name, &declared.module, kind, &declared.head);
            }
        }
        items.truncate(60);
        items
    }

    /// In the braces of a record written by its type's name, `User { ... }`,
    /// the fields not written yet, each as `name: `.
    fn record_fields(&self, analyzed: &Analyzed, text: &str, start: usize) -> Option<Vec<Value>> {
        // The innermost brace still open before the name being written.
        let mut depth = 0usize;
        let mut open = None;
        for (i, c) in text[..start].char_indices().rev() {
            match c {
                ')' | ']' | '}' => depth += 1,
                '(' | '[' if depth > 0 => depth -= 1,
                '{' if depth > 0 => depth -= 1,
                '(' | '[' => return None,
                '{' => {
                    open = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let open = open?;
        // Where a field starts: nothing but space since the brace, a comma
        // or the end of a line.
        let entry = text[open..start].rfind(['{', ',', '\n']).map_or(open, |at| open + at + 1);
        if !text[entry..start].trim().is_empty() {
            return None;
        }
        let head = text[..open].trim_end();
        let named = after_last(head, |c: char| !(c.is_alphanumeric() || c == '_' || c == '.'));
        let name = head[named..].rsplit('.').next()?;
        if !name.starts_with(|c: char| c.is_uppercase()) {
            return None;
        }
        let index = analyzed.analysis.index.as_ref()?;
        let def = index.defs.iter().find(|d| d.kind == DefKind::Type && d.name == name)?;
        let file = analyzed.analysis.sources.file_of(def.decl_span.start);
        let declaration = file.slice(def.decl_span);
        let body = declaration.split_once('{')?.1;
        let given = &text[open..start];
        let mut items = Vec::new();
        for line in body.lines().map(str::trim) {
            if line.starts_with("//") || line.starts_with('@') || line.starts_with("priv ") {
                continue;
            }
            let Some((field, ty)) = line.split_once(':') else { continue };
            let field = field.trim();
            if field.is_empty() || !field.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            if given.contains(&format!("{field}:")) || given.contains(&format!("{field},")) {
                continue;
            }
            items.push(json!({
                "label": format!("{field}:"), "kind": 5, "detail": ty.trim(), "sortText": format!("!{field}"),
                "insertText": format!("{field}: "),
            }));
        }
        Some(items)
    }

    /// Where an argument of a call starts, the names of the parameters the
    /// call does not name yet, each as `name: `.
    fn named_arguments(&mut self, params: &Value, written: &str) -> Option<Vec<Value>> {
        let signature = self.signature_help(params)?;
        let call = &written[written.rfind('(')?..];
        // Those before the argument being written are given by position.
        let given = signature["activeParameter"].as_u64().unwrap_or(0) as usize;
        let mut items = Vec::new();
        for parameter in signature["signatures"][0]["parameters"].as_array()?.iter().skip(given) {
            let label = parameter["label"].as_str().unwrap_or_default();
            let Some((name, _)) = label.split_once(':') else { continue };
            let name = name.trim().trim_start_matches("...");
            if name.is_empty() || name.ends_with("self") || call.contains(&format!("{name}:")) {
                continue;
            }
            items.push(json!({
                "label": format!("{name}:"), "kind": 5, "detail": label, "sortText": format!("!{name}"),
                "insertText": format!("{name}: "),
            }));
        }
        Some(items)
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
        let from = after_last(&line[..open], |c: char| !(c.is_alphanumeric() || c == '_' || c == '.'));
        let path = &line[Some(from).filter(|&from| from > 0)?..open];
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
            let mut item = json!({ "label": keyword, "kind": 14 });
            if let Some((_, snippet)) = SNIPPETS.iter().find(|(k, _)| *k == keyword) {
                item["insertText"] = json!(snippet);
                item["insertTextFormat"] = json!(2);
            }
            items.push(item);
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
        // Of the lines of `range`, when it is given.
        let lines = params["range"]["start"]["line"].as_u64().zip(params["range"]["end"]["line"].as_u64());
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
                if matches!(found.kind, DefKind::Local | DefKind::Parameter) && changeable(analyzed, found) {
                    modifiers |= 8;
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
        // Tokens do not overlap: one that starts inside another goes.
        let mut reached = 0;
        tokens.retain(|t| {
            let apart = t.0 >= reached;
            reached = reached.max(t.1);
            apart
        });
        // One line each, as relative positions; the lines counted as the
        // tokens go, once.
        let text = &source.text;
        let mut data = Vec::new();
        let (mut last_line, mut last_column) = (0, 0);
        let (mut line, mut line_start, mut counted) = (0usize, 0usize, 0usize);
        for (start, end, kind, modifiers) in tokens {
            let mut from = start as usize;
            let end = end as usize;
            while from < end {
                for (at, _) in text[counted..from].match_indices('\n') {
                    line += 1;
                    line_start = counted + at + 1;
                }
                counted = from;
                let line_end = text[from..end].find('\n').map_or(end, |i| from + i);
                let shown = lines.is_none_or(|(first, last)| (first..=last).contains(&(line as u64)));
                if line_end > from && shown {
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
        // Refactors are offered when asked for, not for the light bulb.
        let refactor = wants("refactor") && params["context"]["triggerKind"].as_u64() != Some(2) && from < to;
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
        for (span, _) in unused_public(analyzed).into_iter().filter(|(span, _)| span.start <= to && from <= span.end) {
            if wants("quickfix") {
                let edit = json!({ "changes": self.edits(analyzed, &[(span, String::new())]) });
                actions.push(json!({ "title": "remove `pub`", "kind": "quickfix", "edit": edit }));
            }
        }
        if refactor {
            actions.extend(self.refactors(params));
        }
        if wants("refactor") && params["context"]["triggerKind"].as_u64() != Some(2) {
            actions.extend(self.inline(params));
            actions.extend(self.inline_call(params));
            actions.extend(self.declaration_actions(params));
            actions.extend(self.branch_action(params));
        }
        if wants("refactor") {
            actions.extend(self.test_action(params));
        }
        Some(Value::Array(actions))
    }

    /// On a function's name: a test of it, in the module's tests, which
    /// calls it and says what is left to write.
    fn test_action(&mut self, params: &Value) -> Option<Value> {
        let at = json!({ "textDocument": params["textDocument"], "position": params["range"]["start"] });
        let (analyzed, def, span) = self.def_at(&at)?;
        let index = analyzed.analysis.index.as_ref()?;
        let declared = &index.defs[def];
        if declared.kind != DefKind::Function || span != declared.name_span || declared.name == "main" {
            return None;
        }
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        let (_, project) = root_of(&path);
        let stem = path.file_stem()?.to_str()?;
        if !project || stem.ends_with("_test") {
            return None;
        }
        let file = analyzed.analysis.sources.file_of(declared.decl_span.start);
        let header = header(file.slice(declared.decl_span));
        let (arguments, fails, gives) = call_of(&header)?;
        let tests = path.with_file_name(format!("{stem}_test.to"));
        let existing = match self.open.get(&tests) {
            Some(document) => Some(document.text.clone()),
            None => std::fs::read_to_string(&tests).ok(),
        };
        let name = &declared.name;
        let test = unused_name(&format!("{name}Works"), existing.as_deref().unwrap_or_default());
        let question = if fails { "?" } else { "" };
        let call = format!("{name}({}){question}", arguments.join(", "));
        let body = match gives {
            true => format!("    let result = {call}\n    _ = result\n"),
            false => format!("    {call}\n"),
        };
        let written = format!(
            "test {test} {{\n{body}    testing.failNow(\"write what `{name}` should do, with `assert`\")\n}}\n"
        );
        let uri = self.uri(&tests);
        let zero = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } });
        let mut changes = Vec::new();
        let edits = match &existing {
            None => {
                changes.push(json!({ "kind": "create", "uri": uri, "options": { "ignoreIfExists": true } }));
                vec![json!({ "range": zero, "newText": format!("import std.testing\n\n{written}") })]
            }
            Some(text) => {
                let end = position(text, text.len(), self.units);
                let mut edits = Vec::new();
                if !text.lines().any(|l| l.trim() == "import std.testing") {
                    edits.push(json!({ "range": zero, "newText": "import std.testing\n" }));
                }
                let gap = if text.ends_with("\n\n") || text.is_empty() {
                    ""
                } else if text.ends_with('\n') {
                    "\n"
                } else {
                    "\n\n"
                };
                edits.push(json!({ "range": { "start": end, "end": end }, "newText": format!("{gap}{written}") }));
                edits
            }
        };
        changes.push(json!({ "textDocument": { "uri": uri, "version": Value::Null }, "edits": edits }));
        Some(
            json!({ "title": format!("Write a test of `{name}`"), "kind": "refactor", "edit": { "documentChanges": changes } }),
        )
    }

    /// On a `let` binding read once, in the statement after it: its value
    /// written there instead, when the program checks the same.
    fn inline(&mut self, params: &Value) -> Option<Value> {
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        let at = json!({ "textDocument": params["textDocument"], "position": params["range"]["start"] });
        let (analyzed, def, span) = self.def_at(&at)?;
        let index = analyzed.analysis.index.as_ref()?;
        let declared = &index.defs[def];
        if declared.kind != DefKind::Local || span != declared.name_span {
            return None;
        }
        let source = analyzed.analysis.sources.file_of(declared.name_span.start);
        let (text, base) = (source.text.clone(), source.base);
        let name_at = (declared.name_span.start - base) as usize;
        let line = text[..name_at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[name_at..].find('\n').map_or(text.len(), |i| name_at + i);
        let statement = &text[line..line_end];
        let indent: String = statement.chars().take_while(|c| *c == ' ').collect();
        let written = statement.trim_start().strip_prefix("let ")?;
        let value = written.split_once(" = ")?.1.trim();
        if !written.starts_with(&declared.name) || value.is_empty() {
            return None;
        }
        let uses: Vec<Span> = index.references(def).into_iter().filter(|s| *s != declared.name_span).collect();
        let [used] = uses[..] else { return None };
        // Read in the next statement, before anything there that may skip
        // it or run it later.
        let after = line_end + 1;
        let next = text[after.min(text.len())..].lines().position(|l| !l.trim().is_empty())?;
        let next_start = after + text[after..].lines().take(next).map(|l| l.len() + 1).sum::<usize>();
        let next_end = text[next_start..].find('\n').map_or(text.len(), |i| next_start + i);
        let use_at = (used.start - base) as usize;
        if use_at < next_start || use_at > next_end || !text[next_start..next_end].starts_with(&indent) {
            return None;
        }
        let head = &text[next_start..use_at];
        let opened = head.matches('{').count() != head.matches('}').count();
        if opened || [" and ", " or ", "=>"].iter().any(|w| head.contains(w)) {
            return None;
        }
        let simple = value.chars().all(|c| c.is_alphanumeric() || "._\"()[]".contains(c)) || value.starts_with('"');
        let replacement = if simple { value.to_string() } else { format!("({value})") };
        let edits = vec![
            (Span::new(base + line as u32, base + (line_end + 1).min(text.len()) as u32), String::new()),
            (used, replacement),
        ];
        let before = errors_of(analyzed);
        let mut edited = text.clone();
        for (span, new) in edits.iter().rev() {
            edited.replace_range((span.start - base) as usize..(span.end - base) as usize, new);
        }
        if !self.checks_as_before(&path, &before, edited) {
            return None;
        }
        let analyzed = self.analyses.get(&key_of(&path))?;
        let title = format!("Inline `{}`", index_name(analyzed, def));
        Some(json!({ "title": title, "kind": "refactor.inline", "edit": { "changes": self.edits(analyzed, &edits) } }))
    }

    /// On a call of a function that is one expression: that expression, with
    /// the arguments in place of its parameters, when the program checks
    /// the same.
    fn inline_call(&mut self, params: &Value) -> Option<Value> {
        let at = json!({ "textDocument": params["textDocument"], "position": params["range"]["start"] });
        let (analyzed, def, span) = self.def_at(&at)?;
        let index = analyzed.analysis.index.as_ref()?;
        let declared = &index.defs[def];
        if declared.kind != DefKind::Function || span == declared.name_span {
            return None;
        }
        let sources = &analyzed.analysis.sources;
        let home = sources.file_of(declared.name_span.start);
        let from = (declared.decl_span.start - home.base) as usize;
        let text = &home.text[from..(declared.decl_span.end - home.base) as usize];
        let name_end = (declared.name_span.end - home.base) as usize - from;
        let list = crate::refactor::list_at(text, name_end)?;
        let (body_from, body_to) = crate::refactor::single_expression(text, list.close)?;
        let here = sources.file_of(span.start);
        let call = crate::refactor::list_at(&here.text, (span.end - here.base) as usize)?;
        if call.items.len() != list.items.len() {
            return None;
        }
        // Each parameter's reads in the body.
        let mut reads: Vec<(usize, usize, String)> = Vec::new();
        for (&(a, b), &(c, d)) in list.items.iter().zip(&call.items) {
            let name = crate::refactor::parameter_name(&text[a..b]);
            let argument = here.text[c..d].to_string();
            let inside = |def: &crate::index::Def| {
                def.kind == DefKind::Parameter
                    && def.name == name
                    && declared.decl_span.start <= def.name_span.start
                    && def.name_span.end <= declared.decl_span.end
            };
            let parameter = index.defs.iter().position(inside)?;
            let used: Vec<Span> =
                index.references(parameter).into_iter().filter(|s| *s != index.defs[parameter].name_span).collect();
            let simple = argument.chars().all(|c| c.is_alphanumeric() || "._\"".contains(c));
            // What is read more than once, or never, must not run a call.
            if used.len() != 1 && !simple {
                return None;
            }
            let written = if simple { argument } else { format!("({argument})") };
            for read in used {
                let at = (read.start - home.base) as usize - from;
                reads.push((at, at + (read.end - read.start) as usize, written.clone()));
            }
        }
        reads.sort_by_key(|&(at, ..)| std::cmp::Reverse(at));
        let mut body = text[body_from..body_to].to_string();
        for (start, end, written) in reads {
            if start < body_from || end > body_to {
                return None;
            }
            body.replace_range(start - body_from..end - body_from, &written);
        }
        let name = declared.name.clone();
        let start = (span.start - here.base) as usize;
        // With what names it: `module.f(...)` goes whole.
        let qualified = after_last(&here.text[..start], |c: char| !(c.is_alphanumeric() || c == '_' || c == '.'));
        let whole = Span::new(here.base + qualified as u32, here.base + call.close as u32 + 1);
        let edits = vec![(whole, format!("({body})"))];
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        self.checked_edit(&path, &edits).ok().map(
            |edit| json!({ "title": format!("Inline this call of `{name}`"), "kind": "refactor.inline", "edit": edit }),
        )
    }

    /// What can be done with the declaration a request points at: its
    /// signature, and where it lives. Each asks the editor for what it needs.
    fn declaration_actions(&mut self, params: &Value) -> Vec<Value> {
        let at = json!({ "textDocument": params["textDocument"], "position": params["range"]["start"] });
        let Some((analyzed, def, _)) = self.def_at(&at) else { return Vec::new() };
        let Some(index) = analyzed.analysis.index.as_ref() else { return Vec::new() };
        let declared = &index.defs[def];
        let mut actions = Vec::new();
        let place = json!([params["textDocument"]["uri"], params["range"]["start"]]);
        if matches!(declared.kind, DefKind::Function | DefKind::Method)
            && let Some((list, _)) = self.parameters_of(analyzed, def)
        {
            let mut arguments = place.clone();
            arguments.as_array_mut().expect("an array").push(json!(list));
            let command =
                json!({ "title": "Change signature", "command": "tondo.changeSignature", "arguments": arguments });
            actions.push(json!({ "title": format!("Change the signature of `{}`", declared.name), "kind": "refactor", "command": command }));
        }
        if declared.kind == DefKind::Function
            && let Some((list, false)) = self.parameters_of(analyzed, def)
            && crate::refactor::parameters(&list).is_ok_and(|parameters| parameters.len() > 1)
        {
            let mut arguments = place.clone();
            arguments.as_array_mut().expect("an array").push(json!(list));
            let command = json!({ "title": "Group", "command": "tondo.groupParameters", "arguments": arguments });
            actions.push(json!({ "title": format!("Group parameters of `{}` into a type", declared.name), "kind": "refactor", "command": command }));
        }
        if declared.kind == DefKind::Method {
            let command = json!({ "title": "Extract", "command": "tondo.extractTrait", "arguments": place });
            actions.push(json!({ "title": "Extract a trait from methods", "kind": "refactor", "command": command }));
        }
        // The rest ask nothing: the client runs them, and they say why not.
        let refactor = |title: String, name: &str| {
            let mut arguments = json!([name]);
            arguments.as_array_mut().expect("an array").extend(place.as_array().expect("an array").iter().cloned());
            let command = json!({ "title": title, "command": "tondo.refactor", "arguments": arguments });
            json!({ "title": title, "kind": "refactor", "command": command })
        };
        match declared.kind {
            DefKind::Function if declared.name != "main" => {
                actions.push(refactor(
                    format!("Make `{}` a method of its first parameter", declared.name),
                    "tondo.toMethod",
                ));
            }
            DefKind::Method => {
                actions.push(refactor(format!("Make `{}` a function", declared.name), "tondo.toFunction"));
            }
            DefKind::Type => {
                let home = analyzed.analysis.sources.file_of(declared.decl_span.start);
                let at = (declared.decl_span.start - home.base) as usize;
                if home.text[at..].trim_start_matches("pub ").starts_with("alias ") {
                    actions.push(refactor(format!("Inline the alias `{}`", declared.name), "tondo.inlineAlias"));
                }
            }
            _ => {}
        }
        if matches!(declared.kind, DefKind::Function | DefKind::Type | DefKind::Enum | DefKind::Const) {
            let command = json!({ "title": "Move", "command": "tondo.moveToModule", "arguments": place });
            actions.push(json!({ "title": format!("Move `{}` to another module", declared.name), "kind": "refactor.move", "command": command }));
        }
        actions.extend(self.safe_delete(def, params));
        actions
    }

    /// A declaration nothing uses, with its comments, gone: when the program
    /// checks the same.
    fn safe_delete(&mut self, def: usize, params: &Value) -> Option<Value> {
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        let analyzed = self.analyses.get(&key_of(&path))?;
        let index = analyzed.analysis.index.as_ref()?;
        let declared = &index.defs[def];
        let deletable = matches!(declared.kind, DefKind::Function | DefKind::Type | DefKind::Enum | DefKind::Const);
        if !deletable || declared.name == "main" {
            return None;
        }
        let home = analyzed.analysis.sources.file_of(declared.name_span.start);
        let inside = |span: &Span| declared.decl_span.start <= span.start && span.end <= declared.decl_span.end;
        if !index.references(def).iter().all(inside) {
            return None;
        }
        let (start, end) = declaration_lines(home, declared);
        let name = declared.name.clone();
        let edits = vec![(Span::new(home.base + start as u32, home.base + end as u32), String::new())];
        let edit = self.checked_edit(&path, &edits).ok()?;
        Some(json!({ "title": format!("Delete `{name}`, which nothing uses"), "kind": "refactor", "edit": edit }))
    }

    /// `tondo.toMethod`: `fn area(s: Square)` as `fn Square.area(self)`, what
    /// reads `s` as `self`, and its calls as `value.area()`.
    fn function_as_method(&mut self, arguments: &Value) -> Result<Value, String> {
        use crate::refactor::{list_at, parameter_name};
        let uri = &arguments[0];
        let at = json!({ "textDocument": { "uri": uri }, "position": arguments[1] });
        self.prepare(&at).ok_or("this file is not analyzed")?;
        let (analyzed, def, _) = self.def_at(&at).ok_or("there is no function here")?;
        let index = analyzed.analysis.index.as_ref().ok_or("this file is not analyzed")?;
        let sources = &analyzed.analysis.sources;
        let declared = &index.defs[def];
        let home = sources.file_of(declared.name_span.start);
        let list = list_at(&home.text, (declared.name_span.end - home.base) as usize).ok_or("no parameters here")?;
        let &(first_start, first_end) = list.items.first().ok_or("it has no parameter to be the method of")?;
        let first = &home.text[first_start..first_end];
        let (name, ty) = first.split_once(':').ok_or("its first parameter has no type here")?;
        let (name, ty) = (parameter_name(name), ty.split('=').next().unwrap_or_default().trim());
        if !ty.chars().all(|c| c.is_alphanumeric() || c == '_') || ty.is_empty() {
            return Err(format!("`{ty}` is not a type of this project a method can be declared on"));
        }
        let name_at = declared.name_span;
        let mut edits = vec![(name_at, format!("{ty}.{}", declared.name))];
        // `self`, then what came after it.
        let rest = list.items.get(1).map_or(list.close, |&(start, _)| start);
        edits.push((Span::new(home.base + first_start as u32, home.base + rest as u32), {
            if list.items.len() > 1 { "self, ".to_string() } else { "self".to_string() }
        }));
        for (id, parameter) in index.defs.iter().enumerate() {
            let inside = declared.decl_span.start <= parameter.name_span.start
                && parameter.name_span.end <= declared.decl_span.end;
            if parameter.kind == DefKind::Parameter && inside && parameter.name == name {
                for read in index.references(id).into_iter().filter(|read| *read != parameter.name_span) {
                    edits.push((read, "self".to_string()));
                }
            }
        }
        for span in index.references(def).into_iter().filter(|span| *span != name_at) {
            let file = sources.file_of(span.start);
            let call = list_at(&file.text, (span.end - file.base) as usize).ok_or_else(|| {
                let (_, line, _) = sources.position(span.start);
                format!("`{}` is used as a value at {}:{line}: change that first", declared.name, file.logical)
            })?;
            let &(start, end) = call.items.first().ok_or("a call passes nothing")?;
            let receiver = &file.text[start..end];
            let plain = receiver.chars().all(|c| c.is_alphanumeric() || "_.()[]".contains(c));
            let receiver = if plain { receiver.to_string() } else { format!("({receiver})") };
            // From a qualifier on, as `shapes.area`.
            let before = &file.text[..(span.start - file.base) as usize];
            let qualifier =
                before.len() - before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '.').len();
            let begin = file.base + (before.len() - qualifier) as u32;
            let others: Vec<&str> = call.items[1..].iter().map(|&(a, b)| &file.text[a..b]).collect();
            let written = format!("{receiver}.{}({})", declared.name, others.join(", "));
            edits.push((Span::new(begin, file.base + call.close as u32 + 1), written));
        }
        let path = uri_path(uri.as_str().unwrap_or_default()).ok_or("not a file")?;
        self.checked_edit(&path, &edits)
    }

    /// `tondo.toFunction`: `fn Square.area(self)` as `fn area(square: Square)`,
    /// and its calls as `area(value)`.
    fn method_as_function(&mut self, arguments: &Value) -> Result<Value, String> {
        use crate::refactor::list_at;
        let uri = &arguments[0];
        let at = json!({ "textDocument": { "uri": uri }, "position": arguments[1] });
        self.prepare(&at).ok_or("this file is not analyzed")?;
        let (analyzed, def, _) = self.def_at(&at).ok_or("there is no method here")?;
        let index = analyzed.analysis.index.as_ref().ok_or("this file is not analyzed")?;
        let sources = &analyzed.analysis.sources;
        let declared = &index.defs[def];
        let home = sources.file_of(declared.name_span.start);
        let start = (declared.name_span.start - home.base) as usize;
        let dot = home.text[..start].rfind('.').ok_or("this is not a method of a type")?;
        let ty_at = after_last(&home.text[..dot], |c: char| !c.is_alphanumeric() && c != '_');
        let ty = &home.text[ty_at..dot];
        let list = list_at(&home.text, start + declared.name.len()).ok_or("no parameters here")?;
        let &(first_start, first_end) = list.items.first().ok_or("it has no `self`")?;
        if home.text[first_start..first_end].trim() != "self" {
            return Err("only a method that takes `self` as it is becomes a function".to_string());
        }
        let bound = {
            let mut letters = ty.chars();
            letters.next().map_or(String::new(), |c| c.to_lowercase().chain(letters).collect())
        };
        let mut edits = vec![
            (Span::new(home.base + ty_at as u32, declared.name_span.end), declared.name.clone()),
            (Span::new(home.base + first_start as u32, home.base + first_end as u32), format!("{bound}: {ty}")),
        ];
        // What reads `self`, in the body.
        let body = &home.text[list.close..(declared.decl_span.end - home.base) as usize];
        let base = home.base + list.close as u32;
        let tokens = body.match_indices("self").filter(|(at, _)| {
            let before = body[..*at].chars().next_back();
            let after = body[at + 4..].chars().next();
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            !word(before) && !word(after)
        });
        edits.extend(tokens.map(|(at, _)| (Span::new(base + at as u32, base + at as u32 + 4), bound.clone())));
        let module = home.module.rsplit('.').next().unwrap_or_default().to_string();
        for span in index.references(def).into_iter().filter(|span| *span != declared.name_span) {
            let file = sources.file_of(span.start);
            let name_at = (span.start - file.base) as usize;
            let call = list_at(&file.text, (span.end - file.base) as usize).ok_or_else(|| {
                let (_, line, _) = sources.position(span.start);
                format!("`{}` is used as a value at {}:{line}: change that first", declared.name, file.logical)
            })?;
            let receiver_end =
                name_at.checked_sub(1).filter(|&at| file.text.as_bytes()[at] == b'.').ok_or_else(|| {
                    let (_, line, _) = sources.position(span.start);
                    format!("a call at {}:{line} does not go through a value: change that first", file.logical)
                })?;
            let receiver_start = crate::refactor::receiver_start(&file.text, receiver_end);
            let receiver = &file.text[receiver_start..receiver_end];
            let mut passed = vec![receiver.to_string()];
            passed.extend(call.items.iter().map(|&(a, b)| file.text[a..b].to_string()));
            let name =
                if file.base == home.base { declared.name.clone() } else { format!("{module}.{}", declared.name) };
            let written = format!("{name}({})", passed.join(", "));
            edits.push((Span::new(file.base + receiver_start as u32, file.base + call.close as u32 + 1), written));
        }
        let path = uri_path(uri.as_str().unwrap_or_default()).ok_or("not a file")?;
        self.checked_edit(&path, &edits)
    }

    /// On a line that starts an `if` or a `match`, the other way to write it.
    fn branch_action(&self, params: &Value) -> Option<Value> {
        let uri = params["textDocument"]["uri"].as_str()?;
        let path = uri_path(uri)?;
        let line = params["range"]["start"]["line"].as_u64()? as usize;
        let text = self.open.get(&path).map(|document| document.text.clone())?;
        let written = text.lines().nth(line)?.trim_start();
        let title = match written.split(' ').next()? {
            "if" => "Write this `if` as a `match`",
            "match" => "Write this `match` as an `if`",
            _ => return None,
        };
        let arguments = json!(["tondo.ifMatch", uri, params["range"]["start"]]);
        let command = json!({ "title": title, "command": "tondo.refactor", "arguments": arguments });
        Some(json!({ "title": title, "kind": "refactor", "command": command }))
    }

    /// `tondo.ifMatch`: the `if` with an `else` at a place as a `match` of
    /// its condition, or such a `match` as an `if`.
    fn if_match(&mut self, arguments: &Value) -> Result<Value, String> {
        let uri = &arguments[0];
        let at = json!({ "textDocument": { "uri": uri }, "position": arguments[1] });
        self.prepare(&at).ok_or("this file is not analyzed")?;
        let path = uri_path(uri.as_str().unwrap_or_default()).ok_or("not a file")?;
        let analyzed = self.analyses.get(&key_of(&path)).ok_or("this file is not analyzed")?;
        let file = analyzed.paths.iter().position(|p| p.as_deref() == Some(path.as_path())).ok_or("not analyzed")?;
        let source = analyzed.analysis.sources.file(file);
        let line = arguments[1]["line"].as_u64().ok_or("no line")? as usize;
        let start: usize = source.text.split_inclusive('\n').take(line).map(str::len).sum();
        let (end, written) = crate::refactor::if_as_match(&source.text, start)
            .or_else(|| crate::refactor::match_as_if(&source.text, start))
            .ok_or("this line starts no `if` with an `else`, nor a `match` of `true` and `false`")?;
        let span = Span::new(source.base + start as u32, source.base + end as u32);
        self.checked_edit(&path, &[(span, written)])
    }

    /// `tondo.inlineAlias`: what names an alias names what it stands for, and
    /// the alias goes.
    fn inline_alias(&mut self, arguments: &Value) -> Result<Value, String> {
        let uri = &arguments[0];
        let at = json!({ "textDocument": { "uri": uri }, "position": arguments[1] });
        self.prepare(&at).ok_or("this file is not analyzed")?;
        let (analyzed, def, _) = self.def_at(&at).ok_or("there is no alias here")?;
        let index = analyzed.analysis.index.as_ref().ok_or("this file is not analyzed")?;
        let declared = &index.defs[def];
        let home = analyzed.analysis.sources.file_of(declared.name_span.start);
        let start = (declared.decl_span.start - home.base) as usize;
        let end = (declared.decl_span.end - home.base) as usize;
        let (_, stands_for) = home.text[start..end].split_once('=').ok_or("this is not an alias")?;
        let stands_for = stands_for.trim().to_string();
        let (first, last) = declaration_lines(home, declared);
        let mut edits = vec![(Span::new(home.base + first as u32, home.base + last as u32), String::new())];
        let uses = index.references(def).into_iter().filter(|span| *span != declared.name_span);
        edits.extend(uses.map(|span| (span, stands_for.clone())));
        let path = uri_path(uri.as_str().unwrap_or_default()).ok_or("not a file")?;
        self.checked_edit(&path, &edits)
    }

    /// `tondo.trait`: `Named: area, name` gives the type of the method at a
    /// place a trait of those methods, and its `impl`.
    fn extract_trait(&mut self, arguments: &Value) -> Result<Value, String> {
        let usage = "write the trait and the methods, as `Shape: area, name`";
        let uri = &arguments[0];
        let at = json!({ "textDocument": { "uri": uri }, "position": arguments[1] });
        self.prepare(&at).ok_or("this file is not analyzed")?;
        let (name, wanted) = arguments[2].as_str().and_then(|text| text.split_once(':')).ok_or(usage)?;
        let name = name.trim();
        let wanted: Vec<&str> = wanted.split(',').map(str::trim).filter(|method| !method.is_empty()).collect();
        if name.is_empty() || wanted.is_empty() {
            return Err(usage.to_string());
        }
        let (analyzed, def, _) = self.def_at(&at).ok_or("there is no method here")?;
        let index = analyzed.analysis.index.as_ref().ok_or("this file is not analyzed")?;
        let declared = &index.defs[def];
        let home = analyzed.analysis.sources.file_of(declared.name_span.start);
        let before = &home.text[..(declared.name_span.start - home.base) as usize];
        let ty = before.trim_end().trim_end_matches('.');
        let ty = &ty[after_last(ty, |c: char| !c.is_alphanumeric() && c != '_')..];
        let (mut signatures, mut bodies, mut public, mut last) = (Vec::new(), Vec::new(), false, 0);
        for method in &wanted {
            let found = index.defs.iter().find(|d| {
                let same_file = d.name_span.start >= home.base && d.name_span.end <= home.base + home.text.len() as u32;
                let prefix = same_file.then(|| &home.text[..(d.name_span.start - home.base) as usize]);
                d.kind == DefKind::Method && d.name == *method && prefix.is_some_and(|p| p.ends_with(&format!("{ty}.")))
            });
            let found = found.ok_or_else(|| format!("`{ty}` has no method `{method}` in this file"))?;
            last = last.max(declaration_lines(home, found).1);
            let start = (found.decl_span.start - home.base) as usize;
            let text = &home.text[start..(found.decl_span.end - home.base) as usize];
            public |= text.starts_with("pub ");
            let text = text.trim_start_matches("pub ").replacen(&format!("fn {ty}."), "fn ", 1);
            let header = text.find('{').map_or(text.as_str(), |open| text[..open].trim_end());
            signatures.push(format!("    {header}\n"));
            // A call on a type goes to its own methods (language 12.7), which
            // stay: the implementation gives them to the trait.
            let list = crate::refactor::list_at(&home.text, (found.name_span.end - home.base) as usize)
                .ok_or("this method has no parameters")?;
            let given: Vec<&str> =
                list.items.iter().map(|&(a, b)| crate::refactor::parameter_name(&home.text[a..b])).collect();
            let given: Vec<&str> = given.into_iter().filter(|name| *name != "self").collect();
            bodies.push(format!("    {header} {{\n        self.{method}({})\n    }}\n", given.join(", ")));
        }
        let keyword = if public { "pub trait" } else { "trait" };
        let written = format!(
            "{keyword} {name} {{\n{}}}\n\nimpl {name} for {ty} {{\n{}}}\n\n",
            signatures.concat(),
            bodies.join("\n")
        );
        let place = home.base + last as u32;
        let path = uri_path(uri.as_str().unwrap_or_default()).ok_or("not a file")?;
        self.checked_edit(&path, &[(Span::new(place, place), written)])
    }

    /// `tondo.group`: `Point: x, y` takes those parameters of the function
    /// at a place into a new record, and its calls follow.
    fn group_parameters(&mut self, arguments: &Value) -> Result<Value, String> {
        use crate::refactor::{list_at, parameter_name};
        let usage = "write the type and the parameters, as `Point: x, y`";
        let uri = &arguments[0];
        let at = json!({ "textDocument": { "uri": uri }, "position": arguments[1] });
        self.prepare(&at).ok_or("this file is not analyzed")?;
        let (ty, names) = arguments[2].as_str().and_then(|text| text.split_once(':')).ok_or(usage)?;
        let ty = ty.trim();
        let names: Vec<&str> = names.split(',').map(str::trim).filter(|name| !name.is_empty()).collect();
        if ty.is_empty() || names.len() < 2 {
            return Err(usage.to_string());
        }
        let (analyzed, def, _) = self.def_at(&at).ok_or("there is no function here")?;
        let index = analyzed.analysis.index.as_ref().ok_or("this file is not analyzed")?;
        let declared = &index.defs[def];
        let sources = &analyzed.analysis.sources;
        let home = sources.file_of(declared.name_span.start);
        let list = list_at(&home.text, (declared.name_span.end - home.base) as usize).ok_or("no parameters here")?;
        let written: Vec<&str> = list.items.iter().map(|&(a, b)| &home.text[a..b]).collect();
        let named: Vec<&str> = written.iter().map(|item| parameter_name(item)).collect();
        if let Some(missing) = names.iter().find(|name| !named.contains(name)) {
            return Err(format!("`{}` has no parameter `{missing}`", declared.name));
        }
        let first = named.iter().position(|name| names.contains(name)).expect("a parameter it groups");
        let record = {
            let mut letters = ty.chars();
            letters.next().map_or(String::new(), |c| c.to_lowercase().chain(letters).collect())
        };
        // The record, before the function, as visible as the function is.
        let (start, _) = declaration_lines(home, declared);
        let public = home.text[(declared.decl_span.start - home.base) as usize..].starts_with("pub ");
        let fields: Vec<String> = written
            .iter()
            .zip(&named)
            .filter(|(_, name)| names.contains(name))
            .map(|(item, _)| format!("    {}\n", item.trim_start_matches("mut ")))
            .collect();
        let keyword = if public { "pub type" } else { "type" };
        let declaration = format!("{keyword} {ty} = {{\n{}}}\n\n", fields.concat());
        let place = home.base + start as u32;
        let mut edits = vec![(Span::new(place, place), declaration)];
        // Its parameters, and what reads them.
        let kept: Vec<String> = written
            .iter()
            .zip(&named)
            .enumerate()
            .filter_map(|(at, (item, name))| match (at == first, names.contains(name)) {
                (true, _) => Some(format!("{record}: {ty}")),
                (_, true) => None,
                _ => Some(item.to_string()),
            })
            .collect();
        let inside =
            |file: u32, list: &crate::refactor::List| Span::new(file + list.open as u32 + 1, file + list.close as u32);
        edits.push((inside(home.base, &list), kept.join(", ")));
        for (id, parameter) in index.defs.iter().enumerate() {
            let own = declared.decl_span.start <= parameter.name_span.start
                && parameter.name_span.end <= declared.decl_span.end;
            if parameter.kind == DefKind::Parameter && own && names.contains(&parameter.name.as_str()) {
                for read in index.references(id).into_iter().filter(|read| *read != parameter.name_span) {
                    edits.push((read, format!("{record}.{}", parameter.name)));
                }
            }
        }
        // Its calls: the arguments of those parameters, as a record.
        let module = home.module.rsplit('.').next().unwrap_or_default().to_string();
        for span in index.references(def).into_iter().filter(|span| *span != declared.name_span) {
            let file = sources.file_of(span.start);
            let call = list_at(&file.text, (span.end - file.base) as usize).ok_or_else(|| {
                let (_, line, _) = sources.position(span.start);
                format!("`{}` is used as a value at {}:{line}: change that first", declared.name, file.logical)
            })?;
            let given: Vec<&str> = call.items.iter().map(|&(a, b)| &file.text[a..b]).collect();
            if given.len() != named.len() {
                return Err(format!("a call of `{}` in {} does not pass all it takes", declared.name, file.logical));
            }
            let parts: Vec<String> = named
                .iter()
                .zip(&given)
                .filter(|(name, _)| names.contains(name))
                .map(|(name, argument)| format!("{name}: {argument}"))
                .collect();
            let qualified = if file.base == home.base { ty.to_string() } else { format!("{module}.{ty}") };
            let made = format!("{qualified} {{ {} }}", parts.join(", "));
            let passed: Vec<String> = named
                .iter()
                .zip(&given)
                .enumerate()
                .filter_map(|(at, (name, argument))| match (at == first, names.contains(name)) {
                    (true, _) => Some(made.clone()),
                    (_, true) => None,
                    _ => Some(argument.to_string()),
                })
                .collect();
            edits.push((inside(file.base, &call), passed.join(", ")));
        }
        let path = uri_path(uri.as_str().unwrap_or_default()).ok_or("not a file")?;
        self.checked_edit(&path, &edits)
    }

    /// The parameters a function declares, as written, but `self`; and
    /// whether it has that one.
    fn parameters_of(&self, analyzed: &Analyzed, def: usize) -> Option<(String, bool)> {
        let declared = &analyzed.analysis.index.as_ref()?.defs[def];
        let home = analyzed.analysis.sources.file_of(declared.name_span.start);
        let list = crate::refactor::list_at(&home.text, (declared.name_span.end - home.base) as usize)?;
        let items: Vec<&str> = list.items.iter().map(|&(a, b)| &home.text[a..b]).collect();
        let receiver = items.first().is_some_and(|first| crate::refactor::parameter_name(first) == "self");
        Some((items[usize::from(receiver)..].join(", "), receiver))
    }

    /// `tondo.signature`: the function at a place takes the parameters of a
    /// list, and each of its calls follows.
    fn change_signature(&mut self, arguments: &Value) -> Result<Value, String> {
        use crate::refactor::{arguments as moved, list_at, parameters};
        let uri = &arguments[0];
        let at = json!({ "textDocument": { "uri": uri }, "position": arguments[1] });
        self.prepare(&at).ok_or("this file is not analyzed")?;
        let wanted = parameters(arguments[2].as_str().unwrap_or_default())?;
        let (analyzed, def, _) = self.def_at(&at).ok_or("there is no function here")?;
        let index = analyzed.analysis.index.as_ref().ok_or("this file is not analyzed")?;
        let declared = &index.defs[def];
        let (written, receiver) = self.parameters_of(analyzed, def).ok_or("this declaration takes no parameters")?;
        let old: Vec<String> =
            crate::refactor::parameters(&written)?.into_iter().map(|parameter| parameter.name).collect();
        let sources = &analyzed.analysis.sources;
        let home = sources.file_of(declared.name_span.start);
        let list = list_at(&home.text, (declared.name_span.end - home.base) as usize).ok_or("no parameters here")?;
        let declaration = wanted.iter().map(|parameter| parameter.declared.clone()).collect::<Vec<_>>().join(", ");
        let declaration = match (receiver, declaration.is_empty()) {
            (true, true) => "self".to_string(),
            (true, false) => {
                format!("{}, {declaration}", list.items.first().map_or("self", |&(a, b)| &home.text[a..b]))
            }
            _ => declaration,
        };
        let inside =
            |base: u32, list: &crate::refactor::List| Span::new(base + list.open as u32 + 1, base + list.close as u32);
        let mut edits = vec![(inside(home.base, &list), declaration)];
        for span in index.references(def).into_iter().filter(|span| *span != declared.name_span) {
            let file = sources.file_of(span.start);
            let name_end = (span.end - file.base) as usize;
            let Some(call) = list_at(&file.text, name_end) else {
                let (_, line, _) = sources.position(span.start);
                return Err(format!(
                    "`{}` is used as a value at {}:{line}: change that first",
                    declared.name, file.logical
                ));
            };
            let given: Vec<&str> = call.items.iter().map(|&(a, b)| &file.text[a..b]).collect();
            // `Type.method(value, ...)` passes `self` first.
            let (kept, given) = match receiver && given.len() == old.len() + 1 {
                true => (Some(given[0]), &given[1..]),
                false => (None, &given[..]),
            };
            let written = moved(&old, &wanted, given)?;
            let written = kept.map(str::to_string).into_iter().chain(written).collect::<Vec<_>>().join(", ");
            edits.push((inside(file.base, &call), written));
        }
        let path = uri_path(uri.as_str().unwrap_or_default()).ok_or("not a file")?;
        self.checked_edit(&path, &edits)
    }

    /// The modules of the project, for the editor to offer.
    fn own_modules(&mut self, arguments: &Value) -> Option<Value> {
        let at = json!({ "textDocument": { "uri": arguments[0] }, "position": arguments[1] });
        self.prepare(&at)?;
        let (analyzed, here, _) = self.at(&at)?;
        let sources = &analyzed.analysis.sources;
        let mut modules: Vec<String> = (0..sources.len())
            .filter(|&i| sources.file(i).own && i != here)
            .map(|i| sources.file(i).module.clone())
            .filter(|module| *module != sources.file(here).module)
            .collect();
        modules.sort();
        modules.dedup();
        Some(json!(modules))
    }

    /// `tondo.moveTo`: the declaration at a place goes to the end of a
    /// module, with the imports it needs, and all that names it follows.
    fn move_to(&mut self, arguments: &Value) -> Result<Value, String> {
        let uri = &arguments[0];
        let at = json!({ "textDocument": { "uri": uri }, "position": arguments[1] });
        self.prepare(&at).ok_or("this file is not analyzed")?;
        let target_module = arguments[2].as_str().unwrap_or_default().to_string();
        let (analyzed, def, _) = self.def_at(&at).ok_or("there is nothing to move here")?;
        let index = analyzed.analysis.index.as_ref().ok_or("this file is not analyzed")?;
        let declared = &index.defs[def];
        let sources = &analyzed.analysis.sources;
        let home = sources.file_of(declared.name_span.start);
        let target = (0..sources.len()).map(|i| sources.file(i)).find(|file| file.module == target_module && file.own);
        let target = target.ok_or_else(|| format!("the project has no module `{target_module}`"))?;
        if target.base == home.base {
            return Err(format!("`{}` is in that module already", declared.name));
        }
        let last = target_module.rsplit('.').next().unwrap_or(&target_module);
        let (start, end) = declaration_lines(home, declared);
        let moved = home.text[start..end].trim_end();
        let name_line = (declared.name_span.start - home.base) as usize;
        let public =
            home.text[..name_line].rsplit('\n').next().is_some_and(|line| line.trim_start().starts_with("pub "));
        let written = match public {
            true => moved.to_string(),
            false => {
                let at = (declared.decl_span.start - home.base) as usize - start;
                format!("{}pub {}", &moved[..at], &moved[at..])
            }
        };
        let import_line = |module: &str| format!("import {module}\n");
        let import_place = |text: &str| {
            let last = text
                .match_indices("\nimport ")
                .map(|(at, _)| at + 1)
                .chain(text.starts_with("import ").then_some(0))
                .last();
            last.map_or(0, |at| text[at..].find('\n').map_or(text.len(), |i| at + i + 1))
        };
        let mut edits = vec![(Span::new(home.base + start as u32, home.base + end as u32), String::new())];
        // The imports the moved code reads, which the module may lack.
        let mut needed = Vec::new();
        for line in home.text.lines().filter_map(|line| line.strip_prefix("import ")) {
            let module = line.split_whitespace().next().unwrap_or(line);
            let short = module.rsplit('.').next().unwrap_or(module);
            let already = target.text.lines().any(|l| l.trim() == format!("import {module}"));
            if moved.contains(&format!("{short}.")) && !already && module != target_module {
                needed.push(import_line(module));
            }
        }
        if !needed.is_empty() {
            let place = target.base + import_place(&target.text) as u32;
            edits.push((Span::new(place, place), needed.concat()));
        }
        let gap = if target.text.ends_with("\n\n") {
            ""
        } else if target.text.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        let target_end = target.base + target.text.len() as u32;
        edits.push((Span::new(target_end, target_end), format!("{gap}{written}\n")));
        // All that names it, but in what moves.
        let mut imported: Vec<u32> = Vec::new();
        for span in index.references(def).into_iter().filter(|span| *span != declared.name_span) {
            let file = sources.file_of(span.start);
            let offset = (span.start - file.base) as usize;
            if file.base == home.base && (start..end).contains(&offset) {
                continue;
            }
            let before = &file.text[..offset];
            let qualifier = before.strip_suffix('.').map(|head| {
                head.len()
                    - head
                        .chars()
                        .rev()
                        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
                        .map(char::len_utf8)
                        .sum::<usize>()
            });
            let from = file.base + qualifier.unwrap_or(offset) as u32;
            let name = &declared.name;
            let replacement = if file.base == target.base { name.clone() } else { format!("{last}.{name}") };
            edits.push((Span::new(from, span.end), replacement));
            let has = file.text.lines().any(|line| line.trim() == format!("import {target_module}"));
            if file.base != target.base && !has && !imported.contains(&file.base) {
                imported.push(file.base);
                let place = file.base + import_place(&file.text) as u32;
                edits.push((Span::new(place, place), import_line(&target_module)));
            }
        }
        let path = uri_path(uri.as_str().unwrap_or_default()).ok_or("not a file")?;
        self.checked_edit(&path, &edits).map_err(|message| {
            format!("{message}; what moves may use what stays: move that too, or make it `pub` and import it")
        })
    }

    /// The edits of `edits` for the editor, when the program checks after
    /// them as it did before; or the error it would have.
    fn checked_edit(&mut self, path: &Path, edits: &[(Span, String)]) -> Result<Value, String> {
        let analyzed = self.analyses.get(&key_of(path)).ok_or("this file is not analyzed")?;
        let before = errors_of(analyzed);
        let sources = &analyzed.analysis.sources;
        let mut files: HashMap<PathBuf, FileEdits> = HashMap::new();
        for (span, replacement) in edits {
            let index = (0..sources.len()).rev().find(|&i| sources.file(i).base <= span.start).ok_or("no such file")?;
            let file = sources.file(index);
            let file_path = analyzed.paths[index].clone().ok_or_else(|| format!("{} cannot change", file.logical))?;
            let entry = files.entry(file_path).or_insert_with(|| (file.text.clone(), file.base, Vec::new()));
            entry.2.push((*span, replacement.clone()));
        }
        let mut open: HashMap<PathBuf, String> =
            self.open.iter().map(|(path, document)| (path.clone(), document.text.clone())).collect();
        for (file_path, (text, base, mut changes)) in files {
            changes.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
            let mut edited = text;
            for (span, replacement) in changes {
                edited.replace_range((span.start - base) as usize..(span.end - base) as usize, &replacement);
            }
            open.insert(file_path, edited);
        }
        let (root, _) = root_of(path);
        let after = self.analyzed(&root, open).ok_or("the program does not check after that")?;
        let new = errors_of(&after).into_iter().find(|error| !before.contains(error));
        if let Some((code, message)) = new {
            return Err(format!("the program would not check: {code} {message}"));
        }
        let analyzed = self.analyses.get(&key_of(path)).ok_or("this file is not analyzed")?;
        Ok(json!({ "changes": self.edits(analyzed, edits) }))
    }

    /// Refactors of a selection: an expression into a variable, whole lines
    /// into a function. Each is offered only when the program checks after
    /// it as it did before.
    fn refactors(&mut self, params: &Value) -> Vec<Value> {
        let start = json!({ "textDocument": params["textDocument"], "position": params["range"]["start"] });
        let end = json!({ "textDocument": params["textDocument"], "position": params["range"]["end"] });
        let Some(path) = params["textDocument"]["uri"].as_str().and_then(uri_path) else { return Vec::new() };
        let (Some((analyzed, file, from)), Some((_, _, to))) = (self.at(&start), self.at(&end)) else {
            return Vec::new();
        };
        let Some(index) = analyzed.analysis.index.as_ref() else { return Vec::new() };
        let source = analyzed.analysis.sources.file(file);
        let (base, text) = (source.base, source.text.clone());
        // The selection without the blanks around it.
        let (mut a, mut b) = ((from - base) as usize, (to - base) as usize);
        while a < b && text.as_bytes()[a].is_ascii_whitespace() {
            a += 1;
        }
        while b > a && text.as_bytes()[b - 1].is_ascii_whitespace() {
            b -= 1;
        }
        if a >= b {
            return Vec::new();
        }
        let selected = Span::new(base + a as u32, base + b as u32);
        let mut candidates = Vec::new();
        if let Some(edits) = extract_variable(index, &text, base, selected) {
            candidates.push(("Extract into a variable", edits));
        }
        if let Some(edits) = extract_constant(index, &text, base, selected) {
            candidates.push(("Extract into a constant", edits));
        }
        if let Some(edits) = extract_function(index, source, selected) {
            candidates.push(("Extract into a function", edits));
        }
        let before = errors_of(analyzed);
        let mut actions = Vec::new();
        for (title, edits) in candidates {
            let mut edited = text.clone();
            let mut ordered = edits.clone();
            ordered.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
            for (span, replacement) in &ordered {
                edited.replace_range((span.start - base) as usize..(span.end - base) as usize, replacement);
            }
            if !self.checks_as_before(&path, &before, edited) {
                continue;
            }
            let Some(analyzed) = self.analyses.get(&key_of(&path)) else { continue };
            actions.push(json!({ "title": title, "kind": "refactor.extract", "edit": { "changes": self.edits(analyzed, &edits) } }));
        }
        actions
    }

    /// Whether the program, with `text` as this file, has no error it did
    /// not have before.
    fn checks_as_before(&mut self, path: &Path, before: &[(String, String)], text: String) -> bool {
        let mut open: HashMap<PathBuf, String> =
            self.open.iter().map(|(path, document)| (path.clone(), document.text.clone())).collect();
        open.insert(path.to_path_buf(), text);
        let (root, _) = root_of(path);
        let Some(after) = self.analyzed(&root, open) else { return false };
        let after = errors_of(&after);
        after.len() <= before.len() && after.iter().all(|error| before.contains(error))
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
/// What each word of the language does, which hovering it shows.
const KEYWORD_DOCS: &[(&str, &str)] = &[
    (
        "let",
        "A binding that does not change: `let total = 3`; `var` for one that does. A name never hides another visible one. `tondo guide syntax`.",
    ),
    ("var", "A binding that changes: `var count = 0`, then `count += 1`. `tondo guide syntax`."),
    (
        "fn",
        "A function: `fn add(a: Int, b: Int): Int { a + b }`; a method is `fn User.greet(self): String`. Its errors go in its signature: `fn load(): Config ! LoadError`. `tondo guide syntax`.",
    ),
    ("pub", "Visible to other modules; without it, a declaration is its module's. `tondo guide projects`."),
    ("type", "A record, `type User = { name: String, age: Int }`, or a newtype, `type Id = Int`. `tondo guide types`."),
    (
        "enum",
        "A value that is one of its variants, each with its own data: `enum Shape { Circle(Float), Square(Float) }`. `tondo guide types`.",
    ),
    (
        "trait",
        "What a type must offer: `trait Shape { fn area(self): Float }`, implemented with `impl`. `tondo guide types`.",
    ),
    (
        "impl",
        "A type's implementation of a trait: `impl Display for User { fn display(self): String { ... } }`. `tondo guide types`.",
    ),
    ("const", "A value the program computes once, when it compiles: `const Limit: Int = 100`. `tondo guide syntax`."),
    ("import", "Brings a module in by its path: `import std.http`, then `http.App`. `tondo guide projects`."),
    (
        "if",
        "Runs a block when the condition holds; it is an expression: `let size = if big { 10 } else { 1 }`. `tondo guide syntax`.",
    ),
    ("else", "The block of an `if` when its condition does not hold. `tondo guide syntax`."),
    (
        "for",
        "A loop over what an iterator gives, `for item in items`, while a condition holds, `for n < 10`, or for ever, `for { ... }`. `tondo guide syntax`.",
    ),
    ("in", "What a `for` goes through, or whether a value is in a collection: `x in [1, 2]`. `tondo guide syntax`."),
    (
        "match",
        "Chooses by a value's shape, every case covered: `match shape { Shape.Circle(r) => ..., _ => ... }`. `tondo guide syntax`.",
    ),
    (
        "return",
        "Leaves the function early with a value; the last expression of a block is its value without it. `tondo guide syntax`.",
    ),
    ("break", "Leaves the loop it is in. `tondo guide syntax`."),
    ("continue", "Goes on to the next turn of the loop it is in. `tondo guide syntax`."),
    (
        "fail",
        "Ends the function with an error of its `!` type: `fail ParseError.Empty`. `?` passes on one that a call gives. `tondo guide errors`.",
    ),
    (
        "spawn",
        "Runs a call in a task of its own: `let job = spawn fetch(url)`, then `job.wait()`. `tondo guide tasks`.",
    ),
    (
        "defer",
        "Runs a call or a block when the scope ends, however it ends: `defer file.close()`. `tondo guide errors`.",
    ),
    ("select", "Waits for the first of several channels or tasks. `tondo guide tasks`."),
    ("recover", "Stops a panic of its block, as a `T ! Panic`: `recover { risky() }`. `tondo guide errors`."),
    (
        "context",
        "Values that go with the work that runs, read anywhere: `context Request = { id: String = \"\" }`, set by `with`. `tondo guide tasks`.",
    ),
    (
        "annotation",
        "A type whose values are written on declarations, `@Check(min: 3)`, and read by reflection or a `derive`. `tondo guide meta`.",
    ),
    (
        "with",
        "Changes fields of a context until the block ends: `with Request { id: id }`; on a record, a copy with fields changed: `user with { age: 37 }`. `tondo guide tasks`.",
    ),
    (
        "within",
        "Runs a block with a deadline; its waits end when it passes: `within time.seconds(2) { fetch() }`. `tondo guide tasks`.",
    ),
    ("none", "An option without a value; `some(x)` has one. `tondo guide errors`."),
    ("some", "An option with a value: `some(3)`; `none` has none. `tondo guide errors`."),
];

/// What a keyword completes to, its parts to fill in turn.
const SNIPPETS: &[(&str, &str)] = &[
    ("fn", "fn ${1:name}($2) {\n\t$0\n}"),
    ("type", "type ${1:Name} = {\n\t$0\n}"),
    ("enum", "enum ${1:Name} {\n\t$0\n}"),
    ("trait", "trait ${1:Name} {\n\t$0\n}"),
    ("impl", "impl ${1:Trait} for ${2:Type} {\n\t$0\n}"),
    ("if", "if ${1:condition} {\n\t$0\n}"),
    ("for", "for ${1:item} in ${2:items} {\n\t$0\n}"),
    ("match", "match ${1:value} {\n\t${2:pattern} => $0\n}"),
    ("defer", "defer {\n\t$0\n}"),
    ("recover", "recover {\n\t$0\n}"),
    ("within", "within ${1:time.seconds(1)} {\n\t$0\n}"),
    ("context", "context ${1:Name} = {\n\t${2:field}: ${3:String} = $0\n}"),
    ("with", "with ${1:Context} { ${2:field}: $0 }"),
];

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
    let start = after_last(call, |c: char| !(c.is_alphanumeric() || c == '_' || c == '.'));
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

/// Where `text` goes on after the last character that `stops`, which may
/// take more than a byte; its start when none does.
pub(crate) fn after_last(text: &str, stops: impl Fn(char) -> bool) -> usize {
    text.char_indices().rev().find(|&(_, c)| stops(c)).map_or(0, |(at, c)| at + c.len_utf8())
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
/// What a hover over a module says: its name, and for a standard one
/// what it is for and its example.
fn module_hover(module: &str) -> String {
    let mut value = format!("```tondo\nmodule {module}\n```");
    if let Some(summary) = crate::api::summary(module) {
        value.push_str(&format!("\n\n{summary}"));
    }
    if let Some(example) = crate::api::example(module) {
        value.push_str(&format!("\n\n```tondo\n{example}```"));
    }
    value
}

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
        // The start of a character, when it is asked for a place inside one.
        let inside = (0..=character.min(rest.len())).rev().find(|&at| rest.is_char_boundary(at));
        return Some(start + inside.unwrap_or(0));
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
    /// Links of each `import` to its module's first file, and of each path
    /// of `std.embed` to the file it reads.
    fn links(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        let analyzed = self.analyses.get(&key_of(&path))?;
        let file = analyzed.paths.iter().position(|p| p.as_deref() == Some(path.as_path()))?;
        let source = analyzed.analysis.sources.file(file);
        let text = &source.text;
        let sources = &analyzed.analysis.sources;
        let mut links = Vec::new();
        let mut start = 0;
        for line in text.split_inclusive('\n') {
            let range = |from: usize, to: usize| json!({ "start": position(text, start + from, self.units), "end": position(text, start + to, self.units) });
            if let Some(module) = line.strip_prefix("import ").and_then(|rest| rest.split_whitespace().next()) {
                let target =
                    (0..sources.len()).find(|&i| sources.file(i).module == module && analyzed.paths[i].is_some());
                if let Some(target) = target.and_then(|i| analyzed.paths[i].as_ref()) {
                    links.push(json!({ "range": range(7, 7 + module.len()), "target": self.uri(target) }));
                }
            }
            for (at, _) in line.match_indices("(\"") {
                let inside = start + at + 2;
                let Some((_, whole)) = embed_path(text, inside) else { continue };
                let read = crate::embed::resolve(&path, &whole).ok().filter(|found| found.is_file());
                if let Some(read) = read {
                    links.push(json!({ "range": range(at + 2, at + 2 + whole.len()), "target": self.uri(&read) }));
                }
            }
            start += line.len();
        }
        Some(Value::Array(links))
    }

    /// The colours a file writes as `Color.hex("#rrggbb")` or
    /// `Color.rgb(r, g, b)`, which an editor shows and picks.
    fn colors(&self, params: &Value) -> Option<Value> {
        let (_, text) = self.document(params)?;
        let mut found = Vec::new();
        for (at, _) in text.match_indices("Color.hex(\"") {
            let start = at + "Color.hex(".len();
            let Some(close) = text[start + 1..].find('"').map(|i| start + 1 + i) else { continue };
            let digits = text[start + 1..close].trim_start_matches('#');
            let channel = |i: usize| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok();
            let alpha = if digits.len() == 8 { channel(6) } else { Some(255) };
            if let (6 | 8, Some(r), Some(g), Some(b), Some(a)) =
                (digits.len(), channel(0), channel(2), channel(4), alpha)
            {
                found.push((start, close + 1, [r, g, b, a]));
            }
        }
        for (at, _) in text.match_indices("Color.rgb(") {
            let start = at + "Color.rgb(".len();
            let Some(close) = text[start..].find(')').map(|i| start + i) else { continue };
            let values: Vec<Option<u8>> = text[start..close].split(',').map(|v| v.trim().parse().ok()).collect();
            if let [Some(r), Some(g), Some(b)] = values[..] {
                found.push((start, close, [r, g, b, 255]));
            }
        }
        let colors: Vec<Value> = found
            .into_iter()
            .map(|(start, end, [r, g, b, a])| {
                let range = json!({ "start": position(&text, start, self.units), "end": position(&text, end, self.units) });
                let color = json!({ "red": r as f64 / 255.0, "green": g as f64 / 255.0, "blue": b as f64 / 255.0, "alpha": a as f64 / 255.0 });
                json!({ "range": range, "color": color })
            })
            .collect();
        Some(Value::Array(colors))
    }

    /// How a colour picked is written: the text of `Color.hex`, or the
    /// numbers of `Color.rgb`, as the range had it.
    fn color_presentation(&self, params: &Value) -> Option<Value> {
        let (_, text) = self.document(params)?;
        let start = offset(&text, &params["range"]["start"], self.units)?;
        let channel = |name: &str| (params["color"][name].as_f64().unwrap_or(0.0) * 255.0).round() as u8;
        let (r, g, b, a) = (channel("red"), channel("green"), channel("blue"), channel("alpha"));
        let label = match (text[start..].starts_with('"'), a) {
            (true, 255) => format!("\"#{r:02x}{g:02x}{b:02x}\""),
            (true, _) => format!("\"#{r:02x}{g:02x}{b:02x}{a:02x}\""),
            (false, _) => format!("{r}, {g}, {b}"),
        };
        Some(json!([{ "label": label }]))
    }

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
    /// While a debugger is stopped: the bindings of the lines shown, from
    /// the start of the function it stopped in to the line it stopped at,
    /// for the editor to show their values beside them.
    fn inline_values(&mut self, params: &Value) -> Option<Value> {
        let stopped = &params["context"]["stoppedLocation"]["end"];
        let first = json!({ "textDocument": params["textDocument"], "position": params["range"]["start"] });
        let at = json!({ "textDocument": params["textDocument"], "position": stopped });
        self.prepare(&first)?;
        let (analyzed, file, from) = self.at(&first)?;
        let to = self.at(&at)?.2;
        let index = analyzed.analysis.index.as_ref()?;
        let source = analyzed.analysis.sources.file(file);
        // The function the stop is in: what is before it belongs to others.
        let function = index
            .defs
            .iter()
            .filter(|d| matches!(d.kind, DefKind::Function | DefKind::Method))
            .filter(|d| d.decl_span.start <= to && to <= d.decl_span.end)
            .map(|d| d.decl_span.start)
            .max()
            .unwrap_or(source.base);
        let from = from.max(function);
        let bound = |def: usize| matches!(index.defs[def].kind, DefKind::Local | DefKind::Parameter);
        let mut values = Vec::new();
        for (span, def) in index.uses.iter().filter(|(_, def)| bound(*def)) {
            let name = &index.defs[*def].name;
            if span.start < from || span.end > to || name == "_" {
                continue;
            }
            let Some((_, range)) = self.place(analyzed, *span) else { continue };
            values.push(json!({ "range": range, "variableName": name, "caseSensitiveLookup": true }));
        }
        Some(Value::Array(values))
    }

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

/// The errors of an analysis, by code and message.
/// Where a declaration's text is in its file: from the comments and
/// annotations above it to the blank line after it.
fn declaration_lines(home: &SourceFile, declared: &crate::index::Def) -> (usize, usize) {
    let mut start = (declared.decl_span.start - home.base) as usize;
    start = home.text[..start].rfind('\n').map_or(0, |i| i + 1);
    while start > 0 {
        let above = home.text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
        let line = home.text[above..start].trim_start();
        if !(line.starts_with("//") || line.starts_with('@')) {
            break;
        }
        start = above;
    }
    let end = (declared.decl_span.end - home.base) as usize;
    let end = home.text[end..].find('\n').map_or(home.text.len(), |i| end + i + 1);
    let end = if home.text[end..].starts_with('\n') { end + 1 } else { end };
    (start, end)
}

/// A literal selected, as a constant before the function it is in, with a
/// name the editor then renames.
fn extract_constant(index: &Index, text: &str, base: u32, selected: Span) -> Option<Vec<(Span, String)>> {
    let (a, b) = ((selected.start - base) as usize, (selected.end - base) as usize);
    let written = text.get(a..b)?;
    let number = written.chars().next().is_some_and(|c| c.is_ascii_digit());
    let plain = written.len() > 1 && written.starts_with('"') && written.ends_with('"') && !written.contains('{');
    if !(number && written.chars().all(|c| c.is_ascii_alphanumeric() || "._".contains(c)) || plain) {
        return None;
    }
    let enclosing = index
        .defs
        .iter()
        .filter(|d| matches!(d.kind, DefKind::Function | DefKind::Method))
        .filter(|d| d.decl_span.start <= selected.start && selected.end <= d.decl_span.end)
        .min_by_key(|d| d.decl_span.end - d.decl_span.start)?;
    let mut start = (enclosing.decl_span.start - base) as usize;
    start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    while start > 0 {
        let above = text[..start - 1].rfind('\n').map_or(0, |at| at + 1);
        if !text[above..start].trim_start().starts_with("//") && !text[above..start].trim_start().starts_with('@') {
            break;
        }
        start = above;
    }
    let name = unused_name("NewConstant", text);
    let place = base + start as u32;
    Some(vec![(Span::new(place, place), format!("const {name} = {written}\n\n")), (selected, name)])
}

/// A file's text and base, and the edits of it.
type FileEdits = (String, u32, Vec<(Span, String)>);

/// The `pub` of each declaration of a program's files that no other file
/// uses, and what to say of it.
fn unused_public(analyzed: &Analyzed) -> Vec<(Span, String)> {
    let Some(index) = analyzed.analysis.index.as_ref() else { return Vec::new() };
    let sources = &analyzed.analysis.sources;
    let exported = |def: &crate::index::Def| {
        matches!(def.kind, DefKind::Function | DefKind::Type | DefKind::Enum | DefKind::Const | DefKind::Trait)
    };
    let own = |def: &crate::index::Def| sources.file_of(def.name_span.start).own;
    // A package without `main` is a library.
    if !index.defs.iter().any(|def| def.kind == DefKind::Function && def.name == "main" && own(def)) {
        return Vec::new();
    }
    let mut elsewhere = HashSet::new();
    for &(span, def) in &index.uses {
        if sources.file_of(span.start).base != sources.file_of(index.defs[def].name_span.start).base {
            elsewhere.insert(def);
        }
    }
    let mut found = Vec::new();
    for (_, def) in
        index.defs.iter().enumerate().filter(|(id, def)| exported(def) && own(def) && !elsewhere.contains(id))
    {
        let file = sources.file_of(def.name_span.start);
        let at = (def.name_span.start - file.base) as usize;
        let start = file.text[..at].rfind('\n').map_or(0, |i| i + 1);
        let indent = file.text[start..at].len() - file.text[start..at].trim_start().len();
        if file.text[start + indent..at].starts_with("pub ") {
            let keyword = file.base + (start + indent) as u32;
            found.push((
                Span::new(keyword, keyword + 4),
                format!("no other file uses `{}`, so it need not be `pub`", def.name),
            ));
        }
    }
    found
}

fn errors_of(analyzed: &Analyzed) -> Vec<(String, String)> {
    let diagnostics = analyzed.analysis.diags.list.iter();
    let errors = diagnostics.filter(|d| d.severity == crate::diag::Severity::Error);
    errors.map(|d| (d.code.unwrap_or_default().to_string(), d.message.clone())).collect()
}

/// `name`, or `name2`, `name3`... when the file already uses it.
fn unused_name(name: &str, text: &str) -> String {
    let taken = |candidate: &str| {
        text.match_indices(candidate).any(|(at, _)| {
            let before = text[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
            let after = text[at + candidate.len()..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');
            !before && !after
        })
    };
    (1..).map(|n| if n == 1 { name.to_string() } else { format!("{name}{n}") }).find(|c| !taken(c)).unwrap_or_default()
}

/// A name for the value of an expression: what its last call or field
/// says it is, as `upper` for `name.toUpper()`, or `value`.
fn value_name(expression: &str) -> String {
    let expression = expression.trim_end_matches('?');
    let mut head = expression;
    if expression.ends_with(')') {
        let mut depth = 0;
        for (at, c) in expression.char_indices().rev() {
            match c {
                ')' => depth += 1,
                '(' => {
                    depth -= 1;
                    if depth == 0 {
                        head = &expression[..at];
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let last = head.rsplit('.').next().unwrap_or_default();
    let name = ["to", "get", "read", "as"]
        .iter()
        .find_map(|prefix| last.strip_prefix(prefix).filter(|rest| rest.starts_with(|c: char| c.is_uppercase())))
        .unwrap_or(last);
    let mut chars = name.chars();
    let name: String = chars.next().map_or_else(String::new, |first| first.to_lowercase().chain(chars).collect());
    let plain = !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_numeric());
    if !plain || Kw::lookup(&name).is_some() {
        return if expression.starts_with('"') { "text".into() } else { "value".into() };
    }
    name
}

/// The edits that move an expression into a `let` before its statement,
/// when moving it there cannot change what the program does.
fn extract_variable(index: &Index, text: &str, base: u32, selected: Span) -> Option<Vec<(Span, String)>> {
    // A whole expression of the program.
    if !index.types.iter().any(|(span, _)| *span == selected) {
        return None;
    }
    let (a, b) = ((selected.start - base) as usize, (selected.end - base) as usize);
    // The statement starts on this line, or on the lines this one continues.
    let mut line = text[..a].rfind('\n').map_or(0, |at| at + 1);
    while line > 0 {
        let above = text[..line - 1].rfind('\n').map_or(0, |at| at + 1);
        let previous = text[above..line - 1].trim_end();
        let continues = previous.ends_with(['(', '[', ',', '=', '+', '-', '*', '/'])
            || previous.ends_with(" and")
            || previous.ends_with(" or");
        if !continues {
            break;
        }
        line = above;
    }
    let head = &text[line..a];
    // Not inside a block or closure the line opens, an arm, or past what
    // may skip it.
    let opened = head.matches('{').count() != head.matches('}').count();
    let skips = [" and ", " or ", "=>"].iter().any(|word| head.contains(word));
    let trimmed = head.trim_start();
    if opened || skips || trimmed.starts_with('}') || trimmed.starts_with("else") {
        return None;
    }
    let expression = &text[a..b];
    let indent: String = text[line..].chars().take_while(|c| *c == ' ').collect();
    // A name its function does not use, nor anything the module declares.
    let enclosing = index
        .defs
        .iter()
        .filter(|d| matches!(d.kind, DefKind::Function | DefKind::Method))
        .filter(|d| d.decl_span.start <= selected.start && selected.end <= d.decl_span.end)
        .min_by_key(|d| d.decl_span.end - d.decl_span.start);
    let scope =
        enclosing.map_or(text, |d| &text[(d.decl_span.start - base) as usize..(d.decl_span.end - base) as usize]);
    let declared: Vec<&str> = index
        .defs
        .iter()
        .filter(|d| !matches!(d.kind, DefKind::Local | DefKind::Parameter | DefKind::Field))
        .filter(|d| d.decl_span.start >= base && d.decl_span.end <= base + text.len() as u32)
        .map(|d| d.name.as_str())
        .collect();
    let mut name = unused_name(&value_name(expression), scope);
    while declared.contains(&name.as_str()) {
        name = unused_name(&format!("{name}Value"), scope);
    }
    let at = Span::new(base + line as u32, base + line as u32);
    Some(vec![(at, format!("{indent}let {name} = {expression}\n")), (selected, name)])
}

/// The edits that move whole lines of a block into a new function, with
/// the bindings they read as its parameters and those read after them as
/// its result.
fn extract_function(index: &Index, source: &SourceFile, selected: Span) -> Option<Vec<(Span, String)>> {
    let (text, base) = (&source.text, source.base);
    let (a, b) = ((selected.start - base) as usize, (selected.end - base) as usize);
    let first = text[..a].rfind('\n').map_or(0, |at| at + 1);
    let last = text[b..].find('\n').map_or(text.len(), |at| b + at);
    if !text[first..a].trim().is_empty() || !text[b..last].trim().is_empty() {
        return None;
    }
    let lines = &text[first..last];
    let indent: String = lines.chars().take_while(|c| *c == ' ').collect();
    let level = |line: &&str| line.trim().is_empty() || line.starts_with(&indent);
    if !lines.lines().all(|line| level(&line)) {
        return None;
    }
    let block = Span::new(base + first as u32, base + last as u32);
    // What leaves the block other than at its end stays where it is.
    let tokens = crate::lexer::lex(source, Some(block), &mut crate::diag::Diagnostics::default());
    let leaves = tokens.iter().any(|t| matches!(t.tok, Tok::Kw(Kw::Return | Kw::Break | Kw::Continue | Kw::Defer)));
    let fails = tokens.iter().any(|t| matches!(t.tok, Tok::Question | Tok::Kw(Kw::Fail)));
    if leaves {
        return None;
    }
    let enclosing = index
        .defs
        .iter()
        .filter(|d| matches!(d.kind, DefKind::Function | DefKind::Method))
        .filter(|d| d.decl_span.start <= block.start && block.end <= d.decl_span.end)
        .min_by_key(|d| d.decl_span.end - d.decl_span.start);
    let scope = enclosing.map_or(Span::new(base, base + text.len() as u32), |d| d.decl_span);
    let local = |def: &crate::index::Def| matches!(def.kind, DefKind::Local | DefKind::Parameter);
    let within = |span: Span, outer: Span| outer.start <= span.start && span.end <= outer.end;
    // The bindings declared before it that it reads, in the order it does.
    let mut inputs: Vec<(u32, usize)> = Vec::new();
    for &(span, def) in &index.uses {
        let declared = &index.defs[def];
        if within(span, block)
            && local(declared)
            && declared.name_span.start < block.start
            && within(declared.name_span, scope)
        {
            match inputs.iter_mut().find(|(_, d)| *d == def) {
                Some(first) => first.0 = first.0.min(span.start),
                None => inputs.push((span.start, def)),
            }
        }
    }
    inputs.sort();
    // Those it declares that are read after it.
    let mut outputs: Vec<usize> = Vec::new();
    for &(span, def) in &index.uses {
        let declared = &index.defs[def];
        if declared.kind == DefKind::Local
            && within(declared.name_span, block)
            && span.start >= block.end
            && within(span, scope)
            && !outputs.contains(&def)
        {
            outputs.push(def);
        }
    }
    outputs.sort_by_key(|&def| index.defs[def].name_span.start);
    let var = |def: &crate::index::Def| {
        let at = (def.name_span.start - base) as usize;
        text.get(..at).is_some_and(|before| before.trim_end().ends_with("var"))
    };
    // A `var` of the function, or a parameter it may change, is lent with
    // `mut`; `self` makes the new function a method of the same type.
    let mut parameters = Vec::new();
    let mut arguments = Vec::new();
    let mut receiver = None;
    for &(_, def) in &inputs {
        let declared = &index.defs[def];
        let ty = declared.ty.as_ref()?;
        if declared.name == "self" {
            let method = enclosing.filter(|d| d.kind == DefKind::Method)?;
            let header =
                header(&text[(method.decl_span.start - base) as usize..(method.decl_span.end - base) as usize]);
            let (before, params) = header.split_once('(')?;
            let owner = before.trim().rsplit_once(' ')?.1.rsplit_once('.')?.0.to_string();
            let mode = params.split([',', ')']).next()?.trim().to_string();
            receiver = Some((owner, mode));
            continue;
        }
        let loaned = ty.strip_prefix("var ").or_else(|| ty.strip_prefix("mut "));
        match (var(declared), loaned) {
            (false, None) => {
                parameters.push(format!("{}: {ty}", declared.name));
                arguments.push(declared.name.clone());
            }
            (_, inner) => {
                parameters.push(format!("{}: mut {}", declared.name, inner.unwrap_or(ty)));
                arguments.push(format!("mut {}", declared.name));
            }
        }
    }
    let mut results = Vec::new();
    for &def in &outputs {
        let declared = &index.defs[def];
        results.push((declared.name.clone(), declared.ty.clone()?, var(declared)));
    }
    if results.len() > 1 && results.iter().any(|(_, _, var)| *var) {
        return None;
    }
    // The error it propagates is the enclosing function's.
    let error = match (fails, enclosing) {
        (false, _) => None,
        (true, Some(def)) => {
            let header = header(&text[(def.decl_span.start - base) as usize..(def.decl_span.end - base) as usize]);
            let after = header.rsplit_once(')')?.1;
            Some(after.split_once('!')?.1.trim().trim_end_matches('{').trim().to_string()).filter(|e| !e.is_empty())
        }
        (true, None) => return None,
    };
    if fails && error.is_none() {
        return None;
    }
    let name = unused_name("extracted", text);
    let (binding, result, returned) = match results.as_slice() {
        [] => (String::new(), None, None),
        [(name, ty, var)] => {
            (format!("{} {name} = ", if *var { "var" } else { "let" }), Some(ty.clone()), Some(name.clone()))
        }
        many => {
            let names: Vec<&str> = many.iter().map(|(n, ..)| n.as_str()).collect();
            let types: Vec<&str> = many.iter().map(|(_, t, _)| t.as_str()).collect();
            let tuple = format!("({})", names.join(", "));
            (format!("let {tuple} = "), Some(format!("({})", types.join(", "))), Some(tuple))
        }
    };
    let signature = match (&result, &error) {
        (None, None) => String::new(),
        (Some(result), None) => format!(": {result}"),
        (None, Some(error)) => format!(": !{error}"),
        (Some(result), Some(error)) => format!(": {result} ! {error}"),
    };
    let body: String = lines
        .lines()
        .map(|line| match line.trim().is_empty() {
            true => "\n".to_string(),
            false => format!("    {}\n", &line[indent.len()..]),
        })
        .collect();
    let returned = returned.map(|value| format!("    {value}\n")).unwrap_or_default();
    let (declared, callee) = match &receiver {
        Some((owner, mode)) => {
            parameters.insert(0, mode.clone());
            (format!("{owner}.{name}"), format!("self.{name}"))
        }
        None => (name.clone(), name.clone()),
    };
    let function = format!("\n\nfn {declared}({}){signature} {{\n{body}{returned}}}", parameters.join(", "));
    let question = if fails { "?" } else { "" };
    let call = format!("{indent}{binding}{callee}({}){question}", arguments.join(", "));
    let after = Span::new(scope.end, scope.end);
    Some(vec![(block, call), (after, function)])
}

/// Whether a binding can change: `var`, or a `mut` or `var` parameter.
fn changeable(analyzed: &Analyzed, def: &crate::index::Def) -> bool {
    let file = analyzed.analysis.sources.file_of(def.name_span.start);
    let (start, end) = ((def.name_span.start - file.base) as usize, (def.name_span.end - file.base) as usize);
    let (Some(before), Some(after)) = (file.text.get(..start), file.text.get(end..)) else { return false };
    let before = before.trim_end();
    let after = after.trim_start();
    before.ends_with(" var")
        || before.ends_with("(var")
        || before == "var"
        || before.ends_with("\tvar")
        || before.ends_with("\nvar")
        || after.starts_with(": var ")
        || after.starts_with(": mut ")
}

/// The name of a declaration of an analysis.
fn index_name(analyzed: &Analyzed, def: usize) -> String {
    analyzed.analysis.index.as_ref().map(|i| i.defs[def].name.clone()).unwrap_or_default()
}

/// The arguments that call a function, each the value a new one starts
/// from, whether the call fails, and whether it gives a value; `none` for a
/// generic function or a parameter with no such value.
fn call_of(header: &str) -> Option<(Vec<String>, bool, bool)> {
    let open = header.find('(')?;
    if header[..open].contains('[') {
        return None;
    }
    let mut depth = 0;
    let mut close = None;
    for (i, c) in header[open..].char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let mut arguments = Vec::new();
    let mut part = String::new();
    let mut depth = 0;
    let mut parameters = Vec::new();
    for c in header[open + 1..close].chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parameters.push(std::mem::take(&mut part));
                continue;
            }
            _ => {}
        }
        part.push(c);
    }
    parameters.push(part);
    for parameter in parameters.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        // One with a default is left to it.
        if parameter.contains(" = ") {
            continue;
        }
        let ty = parameter.split_once(':')?.1.trim();
        arguments.push(empty_value(ty)?.to_string());
    }
    let result = header[close + 1..].trim().trim_start_matches(':').trim();
    let fails = result.contains('!');
    let gives = !result.is_empty() && !result.starts_with('!');
    Some((arguments, fails, gives))
}

/// The value a new value of a type written so starts from.
fn empty_value(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "Int" | "Int8" | "Int16" | "Int32" | "Int64" | "Int128" | "UInt" | "UInt8" | "UInt16" | "UInt32" | "UInt64" => {
            "0"
        }
        "Float" | "Float32" | "Float64" => "0.0",
        "Bool" => "false",
        "String" => "\"\"",
        _ if ty.ends_with('?') => "none",
        _ if ty.starts_with("Array[") => "[]",
        _ if ty.starts_with("Map[") => "[:]",
        _ => return None,
    })
}

/// The binding a `let` or `var` written with a type declares, and the
/// type, which what follows its `=` gives.
fn expected_type(line: &str) -> Option<(String, String)> {
    let line = line.trim_start();
    let rest = line.strip_prefix("let ").or_else(|| line.strip_prefix("var "))?;
    let (declared, value) = rest.split_once(" = ")?;
    if !value.trim().chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let (name, ty) = declared.split_once(':')?;
    Some((name.trim().to_string(), ty.trim().to_string()))
}

/// What a function's header says it gives: `Int` for `fn f(): Int`.
fn returns(header: &str) -> &str {
    let after = header.rsplit_once(')').map_or("", |(_, after)| after);
    after.trim().trim_start_matches(':').trim()
}

/// How a name matches what is searched, lower is closer: 0 the same name,
/// 1 its start, 2 a part of it, 3 its letters in order, as `hRq` for
/// `handleRequest`; `none` when it does not. Case does not count.
fn matched(name: &str, query: &str) -> Option<u8> {
    let (name, query) = (name.to_lowercase(), query.to_lowercase());
    if name == query {
        return Some(0);
    }
    if name.starts_with(&query) {
        return Some(1);
    }
    if name.contains(&query) {
        return Some(2);
    }
    let mut rest = name.chars();
    query.chars().all(|wanted| rest.any(|c| c == wanted)).then_some(3)
}
