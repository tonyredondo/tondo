//! Routes of `std.http` (standard library 14.24.2). A route of `http.App`
//! with a literal path, `app.get("/users/:id", handler)`, becomes
//! `app.route(...)` with its OpenAPI operation and the schemas of its types.
//! A handler with typed parameters, `(id: Int, input: NewUser) { ... }`,
//! gets an adapter that reads each from the request — the path, the query
//! or the JSON body — and turns its result into a reply.

use super::*;
use std::collections::BTreeMap;

const VERBS: [&str; 5] = ["get", "post", "put", "patch", "delete"];

/// The text of a string literal without interpolation.
fn literal(e: &ast::Expr) -> Option<String> {
    match &e.kind {
        ast::ExprKind::Str(segments) => match segments.as_slice() {
            [] => Some(String::new()),
            [ast::StrSegment::Text(text)] => Some(text.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn json(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Builds the adapter's syntax.
struct Syntax {
    span: Span,
}

impl Syntax {
    fn expr(&self, kind: ast::ExprKind) -> ast::Expr {
        ast::Expr { kind, span: self.span }
    }

    fn ident(&self, name: &str) -> ast::Ident {
        ast::Ident { name: name.into(), span: self.span }
    }

    fn path(&self, names: &[&str]) -> ast::Expr {
        self.expr(ast::ExprKind::Path(names.iter().map(|n| self.ident(n)).collect()))
    }

    fn string(&self, text: &str) -> ast::Expr {
        self.expr(ast::ExprKind::Str(vec![ast::StrSegment::Text(text.into())]))
    }

    fn arg(&self, value: ast::Expr) -> ast::Arg {
        ast::Arg { name: None, value, mode: ast::Mode::Value, spread: false, span: self.span }
    }

    fn named(&self, name: &str, value: ast::Expr) -> ast::Arg {
        ast::Arg { name: Some(self.ident(name)), value, mode: ast::Mode::Value, spread: false, span: self.span }
    }

    fn call(&self, callee: ast::Expr, args: Vec<ast::Expr>) -> ast::Expr {
        let args = args.into_iter().map(|a| self.arg(a)).collect();
        self.expr(ast::ExprKind::Call { callee: Box::new(callee), args })
    }

    fn method(&self, receiver: ast::Expr, name: &str, args: Vec<ast::Expr>) -> ast::Expr {
        let callee = self.expr(ast::ExprKind::Field(Box::new(receiver), self.ident(name)));
        self.call(callee, args)
    }

    fn try_(&self, value: ast::Expr) -> ast::Expr {
        self.expr(ast::ExprKind::Try(Box::new(value)))
    }

    fn closure(&self, params: &[&str], body: ast::Block) -> ast::Expr {
        let params = params
            .iter()
            .map(|name| ast::ClosureParam { name: self.ident(name), ty: None, mode: ast::Mode::Value, variadic: false })
            .collect();
        self.expr(ast::ExprKind::Closure(Box::new(ast::Closure { is_unsafe: false, params, result: None, body })))
    }

    fn block(&self, stmts: Vec<ast::Stmt>, tail: ast::Expr) -> ast::Block {
        ast::Block { stmts, tail: Some(Box::new(tail)), span: self.span }
    }
}

/// A route's OpenAPI operation as it is built.
#[derive(Default)]
struct Operation {
    parameters: Vec<String>,
    body: Option<String>,
    responses: Vec<(String, String)>,
    schemas: BTreeMap<String, String>,
}

impl Operation {
    fn text(&self) -> String {
        let mut parts = Vec::new();
        if !self.parameters.is_empty() {
            parts.push(format!("\"parameters\":[{}]", self.parameters.join(",")));
        }
        if let Some(body) = &self.body {
            parts.push(format!("\"requestBody\":{body}"));
        }
        let responses: Vec<String> = self.responses.iter().map(|(code, r)| format!("{}:{r}", json(code))).collect();
        parts.push(format!("\"responses\":{{{}}}", responses.join(",")));
        format!("{{{}}}", parts.join(","))
    }

    fn parameter(&mut self, name: &str, place: &str, required: bool, schema: String) {
        self.parameters.push(format!(
            "{{\"name\":{},\"in\":\"{place}\",\"required\":{required},\"schema\":{schema}}}",
            json(name)
        ));
    }

    fn response(&mut self, code: &str, description: &str, content: Option<(&str, String)>) {
        let content = match content {
            Some((kind, schema)) => format!(",\"content\":{{{}:{{\"schema\":{schema}}}}}", json(kind)),
            None => String::new(),
        };
        self.responses.push((code.into(), format!("{{\"description\":{}{content}}}", json(description))));
    }
}

impl Checker<'_> {
    fn http_type(&self, name: &str) -> Option<Ty> {
        match self.type_names.get(&format!("http.{name}")) {
            Some(TypeName::Nominal(ty)) => Some(*ty),
            _ => None,
        }
    }

    /// Whether a method call is a route of `http.App` with a literal path.
    pub(super) fn is_http_route(&self, ty: Ty, method: &ast::Ident, args: &[ast::Arg]) -> bool {
        self.http_type("App") == Some(ty)
            && VERBS.contains(&method.name.as_str())
            && args.len() == 2
            && args.iter().all(|a| a.name.is_none() && !a.spread && a.mode == ast::Mode::Value)
            && literal(&args[0].value).is_some()
    }

    /// The names and types a handler takes when it is typed: a closure whose
    /// parameters all have types, or a named function, other than one that
    /// takes just the request.
    fn typed_handler(&mut self, handler: &ast::Expr, request: Ty) -> Option<Vec<ast::Ident>> {
        match &handler.kind {
            ast::ExprKind::Closure(closure) => {
                let mut types = Vec::new();
                for param in &closure.params {
                    types.push(self.resolve_type(param.ty.as_ref()?));
                }
                (types != [request]).then(|| closure.params.iter().map(|p| p.name.clone()).collect())
            }
            ast::ExprKind::Path(segments) if segments.len() == 1 => {
                let Some(&ModuleName::Fn(id)) = self.names.get(&segments[0].name) else { return None };
                let signature = &self.signatures[id.0 as usize];
                if signature.params == [request] || signature.receiver.is_some() {
                    return None;
                }
                let span = segments[0].span;
                Some(signature.names.iter().map(|n| ast::Ident { name: n.clone(), span }).collect())
            }
            _ => None,
        }
    }

    /// `app.get(path, handler)` as `app.route(...)` with its operation, and
    /// with an adapter for a typed handler.
    pub(super) fn http_route(
        &mut self,
        cx: &mut FnCx,
        receiver: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let path = literal(&args[0].value).expect("a literal path");
        let verb = method.name.to_uppercase();
        let handler = &args[1].value;
        let s = Syntax { span: handler.span };
        let segments: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let route_params: Vec<&str> =
            segments.iter().filter_map(|p| p.strip_prefix(':').or_else(|| p.strip_prefix('*'))).collect();
        // A parameter takes a whole segment and is named like a binding.
        let named = |name: &str| {
            name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        };
        if let Some(wrong) = segments
            .iter()
            .find(|p| p.strip_prefix(':').or_else(|| p.strip_prefix('*')).is_some_and(|name| !named(name)))
        {
            let message = format!(
                "the segment `{wrong}` of the route `{path}`: a parameter takes a whole segment and is named like a binding, as `:file`; split the parts of one in the handler"
            );
            self.diags.error("E1102", args[0].value.span, message);
            return Self::error_expr(span);
        }
        if let Some(rest) = segments.iter().rev().skip(1).find(|p| p.starts_with('*')) {
            let message =
                format!("`{rest}` takes the rest of the path, so it is the last segment of the route `{path}`");
            self.diags.error("E1102", args[0].value.span, message);
            return Self::error_expr(span);
        }
        let (Some(request), Some(reply), Some(problem)) =
            (self.http_type("Request"), self.http_type("Reply"), self.http_type("Problem"))
        else {
            return Self::error_expr(span);
        };
        let mut doc = Operation::default();
        let mut prelude = None;
        let adapter = match self.typed_handler(handler, request) {
            None => {
                for name in &route_params {
                    doc.parameter(name, "path", true, "{\"type\":\"string\"}".into());
                }
                doc.response("default", "The reply", None);
                handler.clone()
            }
            Some(typed) => {
                let value = self.expr(cx, handler, None);
                let Some(callable) = self.callable(value.ty) else {
                    self.diags.error("E1102", handler.span, "a route's handler is a function");
                    return Self::error_expr(span);
                };
                let local_name = format!("·handler{}", cx.locals.len());
                let local = self.declare(cx, &s.ident(&local_name), value.ty, false);
                prelude = Some(hir::Stmt::Let(local, value));
                let mut extracted = Vec::new();
                let mut body: Option<String> = None;
                let mut fails = false;
                // A parameter it cannot read has its error; the call would
                // only add one more.
                let mut unread = false;
                for (name, &ty) in typed.iter().zip(&callable.params) {
                    let request_value = || s.path(&["·request"]);
                    let n = name.name.as_str();
                    let is_int = matches!(self.types.kind(ty), TyKind::Int(IntTy::I64));
                    let optional = match *self.types.kind(ty) {
                        TyKind::Option(inner) => Some(inner),
                        _ => None,
                    };
                    let scalar = |this: &Self, ty: Ty| {
                        ty == Ty::STRING || matches!(this.types.kind(ty), TyKind::Int(IntTy::I64))
                    };
                    let read = if ty == request {
                        request_value()
                    } else if route_params.contains(&n) {
                        fails |= is_int;
                        let schema = self.schema(ty, &mut doc.schemas);
                        doc.parameter(n, "path", true, schema);
                        match (ty == Ty::STRING, is_int) {
                            (true, _) => s.method(request_value(), "param", vec![s.string(n)]),
                            (_, true) => s.try_(s.method(request_value(), "intParam", vec![s.string(n)])),
                            _ => {
                                let message = format!(
                                    "the path parameter `{n}` is a `String` or an `Int`, not {}; take `http.Request` to read it",
                                    self.display(ty)
                                );
                                self.diags.error("E1102", name.span, message);
                                unread = true;
                                continue;
                            }
                        }
                    } else if scalar(self, ty) || optional.is_some_and(|inner| scalar(self, inner)) {
                        let inner = optional.unwrap_or(ty);
                        let schema = self.schema(inner, &mut doc.schemas);
                        doc.parameter(n, "query", optional.is_none(), schema);
                        fails |= optional.is_none() || inner != Ty::STRING;
                        let reader = match (optional.is_some(), inner == Ty::STRING) {
                            (true, true) => "query",
                            (true, false) => "intQuery",
                            (false, true) => "requiredQuery",
                            (false, false) => "requiredIntQuery",
                        };
                        let call = s.method(request_value(), reader, vec![s.string(n)]);
                        if reader == "query" { call } else { s.try_(call) }
                    } else {
                        if matches!(verb.as_str(), "GET" | "DELETE") {
                            let message = format!(
                                "a {verb} request has no body, which `{n}` would be read from: take it from the path or the query, or take `http.Request`"
                            );
                            self.diags.error("E1102", name.span, message);
                            unread = true;
                            continue;
                        }
                        if let Some(first) = &body {
                            let message = format!(
                                "a handler reads one body, as `{first}`; `{n}` would be read from it too: put both in one type"
                            );
                            self.diags.error("E1102", name.span, message);
                            unread = true;
                            continue;
                        }
                        body = Some(n.to_string());
                        fails = true;
                        let schema = self.schema(ty, &mut doc.schemas);
                        doc.body = Some(format!(
                            "{{\"required\":true,\"content\":{{\"application/json\":{{\"schema\":{schema}}}}}}}"
                        ));
                        s.try_(s.method(request_value(), "json", Vec::new()))
                    };
                    extracted.push(read);
                }
                if unread {
                    return Self::error_expr(span);
                }
                // The call, with its failure as a problem.
                let mut value = s.call(s.path(&[local_name.as_str()]), extracted);
                let mut result = callable.result;
                if let TyKind::Result(ok, error) = *self.types.kind(result) {
                    result = ok;
                    if error != problem {
                        let from = s.call(s.path(&["http.Problem", "from"]), vec![s.path(&["·error"])]);
                        let mapper = s.closure(&["·error"], s.block(Vec::new(), from));
                        value = s.method(value, "mapErr", vec![mapper]);
                    }
                    value = s.try_(value);
                    doc.response("default", "A problem (RFC 9457)", Some(("application/problem+json", "{}".into())));
                }
                if fails {
                    doc.response("400", "Bad Request", Some(("application/problem+json", "{}".into())));
                }
                let reply_of = |this: &mut Self, value: ast::Expr, ty: Ty, doc: &mut Operation| -> ast::Expr {
                    if ty == reply {
                        doc.response("200", "The reply", None);
                        value
                    } else if ty == Ty::STRING {
                        doc.response("200", "OK", Some(("text/plain", "{\"type\":\"string\"}".into())));
                        s.call(s.path(&["http.Reply", "text"]), vec![value])
                    } else {
                        let schema = this.schema(ty, &mut doc.schemas);
                        doc.response("200", "OK", Some(("application/json", schema)));
                        s.call(s.path(&["http.Reply", "json"]), vec![value])
                    }
                };
                let tail = match *self.types.kind(result) {
                    TyKind::Unit => {
                        doc.response("204", "No Content", None);
                        let discard = ast::Stmt { kind: ast::StmtKind::Discard(value), span: s.span };
                        let empty = s.call(s.path(&["http.Reply", "empty"]), Vec::new());
                        s.expr(ast::ExprKind::Block(s.block(vec![discard], empty)))
                    }
                    TyKind::Option(inner) => {
                        let converted = reply_of(self, s.path(&["·value"]), inner, &mut doc);
                        doc.response("404", "Not Found", Some(("application/problem+json", "{}".into())));
                        let mapper = s.closure(&["·value"], s.block(Vec::new(), converted));
                        let found = s.method(value, "map", vec![mapper]);
                        let missing = s.call(s.path(&["http.Reply", "notFound"]), Vec::new());
                        s.method(found, "unwrapOr", vec![missing])
                    }
                    _ => reply_of(self, value, result, &mut doc),
                };
                s.closure(&["·request"], s.block(Vec::new(), tail))
            }
        };
        let schemas = doc.schemas.iter().map(|(name, schema)| (s.string(name), s.string(schema))).collect();
        let route_args = vec![
            s.arg(s.string(&verb)),
            s.arg(s.string(&path)),
            s.arg(adapter),
            s.named("operation", s.string(&doc.text())),
            s.named("schemas", s.expr(ast::ExprKind::Map(schemas))),
        ];
        let key = self.owner_key(receiver.ty);
        let Some(&id) = self.methods.get(&(key, "route".to_string())) else { return Self::error_expr(span) };
        let signature = self.signatures[id.0 as usize].clone();
        let route = s.ident("route");
        let call =
            self.dot_call(cx, Callee::Fn(id), signature, HashMap::new(), receiver, &route, &route_args, expected, span);
        match prelude {
            None => call,
            Some(stmt) => {
                let ty = call.ty;
                Self::mk(
                    hir::ExprKind::Block(hir::Block { stmts: vec![stmt], tail: Some(Box::new(call)), ty }),
                    ty,
                    span,
                )
            }
        }
    }

    /// The JSON Schema of a type as `std.json` writes it: records as objects,
    /// enums without payloads as strings and others as `{"Variant": ...}`,
    /// newtypes as what they wrap. Records and enums go into `schemas` and
    /// are referred to by name.
    fn schema(&self, ty: Ty, schemas: &mut BTreeMap<String, String>) -> String {
        match self.types.kind(ty).clone() {
            TyKind::Int(_) | TyKind::Byte => "{\"type\":\"integer\"}".into(),
            TyKind::Float(_) => "{\"type\":\"number\"}".into(),
            TyKind::Bool => "{\"type\":\"boolean\"}".into(),
            TyKind::String | TyKind::Char => "{\"type\":\"string\"}".into(),
            TyKind::Unit => "{\"type\":\"null\"}".into(),
            TyKind::Option(inner) => format!("{{\"anyOf\":[{},{{\"type\":\"null\"}}]}}", self.schema(inner, schemas)),
            TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) => {
                format!("{{\"type\":\"array\",\"items\":{}}}", self.schema(element, schemas))
            }
            TyKind::Map(_, value) => {
                format!("{{\"type\":\"object\",\"additionalProperties\":{}}}", self.schema(value, schemas))
            }
            TyKind::Tuple(parts) => {
                let items: Vec<String> = parts.iter().map(|&p| self.schema(p, schemas)).collect();
                format!("{{\"type\":\"array\",\"prefixItems\":[{}]}}", items.join(","))
            }
            TyKind::Adt(id) => {
                let kind = self.types.adt(id).kind.clone();
                if let AdtKind::Newtype(inner) = kind {
                    return self.schema(inner, schemas);
                }
                let shown = self.types.display(ty);
                let name: String = shown
                    .rsplit('.')
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '_' })
                    .collect::<String>()
                    .trim_end_matches('_')
                    .to_string();
                if !schemas.contains_key(&name) {
                    // A recursive type refers to itself while it is written.
                    schemas.insert(name.clone(), "{}".into());
                    let object = |this: &Self, fields: &[Field], schemas: &mut BTreeMap<String, String>| {
                        let properties: Vec<String> = fields
                            .iter()
                            .map(|f| format!("{}:{}", json(&f.name), this.schema(f.ty, schemas)))
                            .collect();
                        let required: Vec<String> = fields
                            .iter()
                            .filter(|f| !matches!(this.types.kind(f.ty), TyKind::Option(_)))
                            .map(|f| json(&f.name))
                            .collect();
                        format!(
                            "{{\"type\":\"object\",\"properties\":{{{}}},\"required\":[{}]}}",
                            properties.join(","),
                            required.join(",")
                        )
                    };
                    let body = match kind {
                        AdtKind::Record(fields) => object(self, &fields, schemas),
                        AdtKind::Enum(variants) if variants.iter().all(|v| matches!(v.payload, Payload::Unit)) => {
                            let names: Vec<String> = variants.iter().map(|v| json(&v.name)).collect();
                            format!("{{\"type\":\"string\",\"enum\":[{}]}}", names.join(","))
                        }
                        AdtKind::Enum(variants) => {
                            let cases: Vec<String> = variants
                                .iter()
                                .map(|v| {
                                    let payload = match &v.payload {
                                        Payload::Unit => return format!("{{\"const\":{}}}", json(&v.name)),
                                        Payload::Tuple(types) if types.len() == 1 => self.schema(types[0], schemas),
                                        Payload::Tuple(types) => {
                                            let items: Vec<String> =
                                                types.iter().map(|&t| self.schema(t, schemas)).collect();
                                            format!("{{\"type\":\"array\",\"prefixItems\":[{}]}}", items.join(","))
                                        }
                                        Payload::Record(fields) => object(self, fields, schemas),
                                    };
                                    format!(
                                        "{{\"type\":\"object\",\"properties\":{{{}:{payload}}},\"required\":[{}]}}",
                                        json(&v.name),
                                        json(&v.name)
                                    )
                                })
                                .collect();
                            format!("{{\"oneOf\":[{}]}}", cases.join(","))
                        }
                        AdtKind::Newtype(_) => unreachable!("handled above"),
                    };
                    schemas.insert(name.clone(), body);
                }
                format!("{{\"$ref\":\"#/components/schemas/{name}\"}}")
            }
            _ => "{}".into(),
        }
    }
}
