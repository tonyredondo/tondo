//! `tondo lsp`: a language server (LSP 3.17) over standard input and
//! output. A project is analyzed as its test build, so test files are too,
//! with the text of the files the editor has open, again after changes.

use crate::driver::{self, Analysis};
use crate::index::{DefKind, Index};
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
}

#[derive(Default)]
struct Server {
    /// Positions count UTF-8 bytes instead of UTF-16 units.
    utf8: bool,
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
    shut: bool,
}

impl Server {
    /// Handles a message; false once the editor says `exit`.
    fn handle(&mut self, message: Value) -> bool {
        let method = message["method"].as_str().unwrap_or_default().to_string();
        let params = &message["params"];
        let id = message.get("id").cloned();
        let result = match method.as_str() {
            "initialize" => Some(Ok(self.initialize(params))),
            "shutdown" => {
                self.shut = true;
                Some(Ok(Value::Null))
            }
            "exit" => return false,
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
            "textDocument/semanticTokens/full" => {
                Some(Ok(self.semantic_tokens(params).unwrap_or(json!({ "data": [] }))))
            }
            _ if id.is_some() => Some(Err((-32601, format!("`{method}` is not supported")))),
            _ => None,
        };
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

    fn initialize(&mut self, params: &Value) -> Value {
        let encodings = &params["capabilities"]["general"]["positionEncodings"];
        self.utf8 = encodings.as_array().is_some_and(|e| e.iter().any(|e| e == "utf-8"));
        json!({
            "capabilities": {
                "positionEncoding": if self.utf8 { "utf-8" } else { "utf-16" },
                "textDocumentSync": { "openClose": true, "change": 1 },
                "hoverProvider": true,
                "definitionProvider": true,
                "referencesProvider": true,
                "renameProvider": { "prepareProvider": true },
                "documentSymbolProvider": true,
                "documentFormattingProvider": true,
                "completionProvider": { "triggerCharacters": ["."] },
                "codeActionProvider": { "codeActionKinds": ["quickfix", "source.fixAll"] },
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
        if self.dirty.remove(&root) || !self.analyses.contains_key(&root) {
            self.analyze(&root);
        }
        Some(())
    }

    fn analyze(&mut self, root: &Path) {
        let open = self.open.iter().map(|(path, document)| (path.clone(), document.text.clone())).collect();
        let Some(analyzed) = self.analyzed(root, open) else { return };
        self.publish(root, &analyzed);
        if let Some(previous) = self.analyses.insert(root.to_path_buf(), analyzed) {
            let earlier = self.earlier.entry(root.to_path_buf()).or_default();
            earlier.insert(0, previous);
            earlier.truncate(3);
        }
    }

    /// An analysis of a root with the given text of open files.
    fn analyzed(&mut self, root: &Path, open: HashMap<PathBuf, String>) -> Option<Analyzed> {
        let analysis = driver::analyze_for_editor(root, root.is_dir(), open).ok()?;
        let std = self.std_dir();
        let paths = (0..analysis.sources.len())
            .map(|index| {
                let file = analysis.sources.file(index);
                match file.logical.strip_prefix("<std>/") {
                    Some(name) => std.as_ref().map(|dir| dir.join(name)),
                    None if file.path.is_absolute() => file.path.canonicalize().ok(),
                    None => None,
                }
            })
            .collect();
        Some(Analyzed { analysis, paths })
    }

    /// Sends the diagnostics of each file of a root, and clears those of
    /// files that have none now.
    fn publish(&mut self, root: &Path, analyzed: &Analyzed) {
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
            by_file.entry(path).or_default().push(json!({
                "range": range,
                "severity": severity,
                "code": diagnostic.code.unwrap_or("E0000"),
                "source": "tondo",
                "message": diagnostic.message,
            }));
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
        self.open
            .get(path)
            .map(|document| document.uri.clone())
            .unwrap_or_else(|| format!("file://{}", encode(&path.to_string_lossy())))
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
        json!({ "start": position(&file.text, start, self.utf8), "end": position(&file.text, end, self.utf8) })
    }

    /// The analysis, the file and the offset a request points at, once
    /// `prepare` made the analysis.
    fn at(&self, params: &Value) -> Option<(&Analyzed, usize, u32)> {
        let path = uri_path(params["textDocument"]["uri"].as_str()?)?;
        let analyzed = self.analyses.get(&root_of(&path).0)?;
        let index = analyzed.paths.iter().position(|p| p.as_deref() == Some(path.as_path()))?;
        let file = analyzed.analysis.sources.file(index);
        let offset = offset(&file.text, &params["position"], self.utf8)?;
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
        let utf8 = self.utf8;
        let (analyzed, def, span) = self.def_at(params)?;
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
        let file = sources.file_of(span.start);
        let range = {
            let start = (span.start - file.base) as usize;
            let end = (span.end - file.base) as usize;
            json!({ "start": position(&file.text, start, utf8), "end": position(&file.text, end, utf8) })
        };
        Some(json!({ "contents": { "kind": "markdown", "value": value }, "range": range }))
    }

    fn definition(&mut self, params: &Value) -> Option<Value> {
        self.prepare(params)?;
        if let Some((analyzed, _, target, _)) = self.imported(params) {
            let base = analyzed.analysis.sources.file(target?).base;
            return self.location(analyzed, Span::new(base, base));
        }
        let (analyzed, def, _) = self.def_at(params)?;
        let def = &analyzed.analysis.index.as_ref()?.defs[def];
        let span = if def.kind == DefKind::Module { def.decl_span } else { def.name_span };
        self.location(analyzed, span)
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
        let root = root_of(&path).0;
        let (latest, file, offset) = self.at(params)?;
        let source = latest.analysis.sources.file(file);
        let text = source.text.clone();
        let cursor = (offset - source.base) as usize;
        let start = text[..cursor].rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
        if !text[..start].ends_with('.') {
            return Some(json!({ "isIncomplete": false, "items": self.names(latest, offset) }));
        }
        let end = start - 1;
        // A module alias the program uses: its public declarations.
        let alias_start = text[..end].rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
        let alias = &text[alias_start..end];
        let analyses = std::iter::once(latest).chain(self.earlier.get(&root).into_iter().flatten());
        for analyzed in analyses {
            if let Some(items) = self.module_members(analyzed, alias) {
                return Some(json!({ "isIncomplete": false, "items": items }));
            }
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
        let items = self.members(&analyzed, base + end as u32).unwrap_or_default();
        Some(json!({ "isIncomplete": false, "items": items }))
    }

    /// The public declarations of the module the program knows as `alias`.
    fn module_members(&self, analyzed: &Analyzed, alias: &str) -> Option<Vec<Value>> {
        let index = analyzed.analysis.index.as_ref()?;
        let sources = &analyzed.analysis.sources;
        let module = index.defs.iter().find(|d| d.kind == DefKind::Module && d.name == alias)?;
        let module = sources.file_of(module.decl_span.start).module.clone();
        let items = index
            .defs
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                matches!(d.kind, DefKind::Function | DefKind::Type | DefKind::Enum | DefKind::Trait | DefKind::Const)
                    && sources.file_of(d.decl_span.start).module == module
                    && sources.slice(d.decl_span).starts_with("pub ")
            })
            .map(|(id, d)| self.item(analyzed, &d.name, d.kind, Some(id)))
            .collect();
        Some(items)
    }

    /// The members of the type of the innermost expression that ends at
    /// `end`, before a dot.
    fn members(&self, analyzed: &Analyzed, end: u32) -> Option<Vec<Value>> {
        let index = analyzed.analysis.index.as_ref()?;
        let (_, shape) =
            index.types.iter().filter(|(span, _)| span.end == end).min_by_key(|(span, _)| span.end - span.start)?;
        let shape = &index.shapes[*shape];
        let items = shape
            .members
            .iter()
            .map(|member| {
                let kind = if member.method { DefKind::Method } else { DefKind::Field };
                let mut item = self.item(analyzed, &member.name, kind, member.def);
                if let (None, Some(ty)) = (member.def, &member.ty) {
                    item["detail"] = json!(ty);
                }
                if let Some(owner) = &member.builtin
                    && let Some((header, docs)) = crate::api::builtin_doc(owner, &member.name)
                {
                    item["detail"] = json!(header);
                    item["documentation"] = json!({ "kind": "markdown", "value": docs });
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
                items.push(self.item(analyzed, &def.name, def.kind, Some(id)));
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
                items.push(self.item(analyzed, &found.name, DefKind::Module, Some(def)));
            }
        }
        for keyword in KEYWORDS {
            items.push(json!({ "label": keyword, "kind": 14 }));
        }
        items
    }

    /// A completion item, with the declaration's header and documentation.
    fn item(&self, analyzed: &Analyzed, name: &str, kind: DefKind, def: Option<usize>) -> Value {
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
        for token in &lexed {
            let (start, end) = (token.span.start - base, token.span.end - base);
            match &token.tok {
                crate::lexer::Tok::Kw(_) => add(start, end, 11),
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
                    let units = |s: &str| if self.utf8 { s.len() } else { s.encode_utf16().count() };
                    let column = units(&text[line_start..from]);
                    let length = units(&text[from..line_end]);
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
        for diagnostic in &analyzed.analysis.diags.list {
            let Some(span) = diagnostic.span.filter(|&span| inside(span)) else { continue };
            for fix in &diagnostic.fixes {
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
        let end = position(&text, text.len(), self.utf8);
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
const KEYWORDS: [&str; 24] = [
    "let", "var", "fn", "pub", "type", "enum", "trait", "impl", "const", "import", "if", "else", "for", "in", "match",
    "return", "break", "continue", "fail", "spawn", "defer", "select", "none", "some",
];

/// The root a file is analyzed under: the project around it, or the file
/// itself; and whether it is a project.
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

/// An LSP position: the line, and the column in UTF-8 bytes or UTF-16 units.
fn position(text: &str, offset: usize, utf8: bool) -> Value {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = text[..line_start].matches('\n').count();
    let column = &text[line_start..offset];
    let character = if utf8 { column.len() } else { column.encode_utf16().count() };
    json!({ "line": line, "character": character })
}

/// The byte offset of an LSP position.
fn offset(text: &str, position: &Value, utf8: bool) -> Option<usize> {
    let line = position["line"].as_u64()? as usize;
    let character = position["character"].as_u64()? as usize;
    let start = match line {
        0 => 0,
        _ => text.match_indices('\n').nth(line - 1)?.0 + 1,
    };
    let rest = &text[start..];
    let rest = &rest[..rest.find('\n').unwrap_or(rest.len())];
    if utf8 {
        return Some(start + character.min(rest.len()));
    }
    let mut units = 0;
    for (index, c) in rest.char_indices() {
        if units >= character {
            return Some(start + index);
        }
        units += c.len_utf16();
    }
    Some(start + rest.len())
}

fn uri_path(uri: &str) -> Option<PathBuf> {
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
