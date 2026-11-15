//! What an interface of C gives its module (language 16.16): for a trait
//! `Name` with `@ffi.Interface`, the type `NameObject`, an object that C
//! made, whose first word points to the table of its functions, which
//! implements the trait, and those it requires, by calling the table's
//! entries with the object first. It is written as Tondo source, as
//! `plugins.rs` writes a contract's; the checker holds the trait to what an
//! interface may be, and this writes nothing for one that is not.

use crate::ast;
use crate::diag::Diagnostics;
use crate::modules::ModuleSource;
use crate::parser;
use crate::source::SourceMap;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// An interface as its file writes it.
struct Interface {
    module: usize,
    file: usize,
    name: String,
    methods: Vec<Method>,
    /// The module and the name of the interface it requires.
    base: Option<(String, String)>,
    /// Its identity, as `@ffi.Interface(id: "...")` writes it.
    id: Option<String>,
}

/// A method of an interface: its name, its parameters, and its result.
struct Method {
    name: String,
    params: Vec<(String, ast::TypeExpr)>,
    result: Option<ast::TypeExpr>,
}

/// The modules each alias of a file names.
fn imported(file: &ast::Module) -> HashMap<String, String> {
    file.imports
        .iter()
        .map(|i| {
            let alias = i.alias.as_ref().unwrap_or(i.path.last().unwrap()).name.clone();
            (alias, i.path.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join("."))
        })
        .collect()
}

/// The interfaces of every module, each given its object.
pub fn derive(modules: &mut [ModuleSource], sources: &mut SourceMap, diags: &mut Diagnostics) {
    let found = interfaces(modules);
    if found.is_empty() {
        return;
    }
    let by_name: HashMap<(String, String), usize> =
        found.iter().enumerate().map(|(at, i)| ((modules[i.module].path.join("."), i.name.clone()), at)).collect();
    // What each module declares as a type, which a signature of another
    // module names through it.
    let declared: Vec<HashSet<String>> = modules
        .iter()
        .map(|m| {
            m.files
                .iter()
                .flat_map(|(file, _)| &file.items)
                .filter_map(|item| match item {
                    ast::Item::Type(t) => Some(t.name.name.clone()),
                    ast::Item::Alias(a) => Some(a.name.name.clone()),
                    ast::Item::Enum(e) => Some(e.name.name.clone()),
                    ast::Item::Trait(t) => Some(t.name.name.clone()),
                    _ => None,
                })
                .collect()
        })
        .collect();
    // An interface's chain, the interface that requires no other first;
    // none when one is missing or the chain goes round, which the checker
    // says.
    let chain_of = |at: usize| -> Option<Vec<usize>> {
        let mut chain = vec![at];
        let mut seen = HashSet::from([at]);
        while let Some(base) = &found[*chain.last().unwrap()].base {
            let &next = by_name.get(base)?;
            if !seen.insert(next) {
                return None;
            }
            chain.push(next);
        }
        chain.reverse();
        Some(chain)
    };
    let mut texts: HashMap<(usize, usize), Writer> = HashMap::new();
    for (at, interface) in found.iter().enumerate() {
        let Some(chain) = chain_of(at) else { continue };
        let chain: Vec<&Interface> = chain.iter().map(|&i| &found[i]).collect();
        let (module, file) = (interface.module, interface.file);
        let writer = texts.entry((module, file)).or_insert_with(|| Writer::new(&modules[module], file));
        writer.object(interface, &chain, modules, &declared, sources);
    }
    // The Tondo types that implement them, each with the tables C calls:
    // one for each interface no other of the type's requires, with those
    // it requires first.
    for implemented in implementations(modules, &by_name) {
        let required: HashSet<usize> = implemented
            .interfaces
            .iter()
            .filter_map(|&at| chain_of(at))
            .flat_map(|chain| chain.into_iter().rev().skip(1))
            .collect();
        let chains: Option<Vec<Vec<&Interface>>> = implemented
            .interfaces
            .iter()
            .filter(|at| !required.contains(at))
            .map(|&at| chain_of(at).map(|chain| chain.iter().map(|&i| &found[i]).collect()))
            .collect();
        let Some(chains) = chains else { continue };
        let (module, file) = (implemented.module, implemented.file);
        let writer = texts.entry((module, file)).or_insert_with(|| Writer::new(&modules[module], file));
        writer.implemented(&implemented.target, &chains, modules, &declared, sources);
    }
    for ((module, file), writer) in texts {
        let module_name = modules[module].path.join(".");
        let target = &mut modules[module].files[file].0;
        let span = target.imports.first().map_or(crate::source::Span::new(0, 0), |i| i.span);
        let ident = |name: &str| ast::Ident { name: name.into(), span };
        for (path, alias) in &writer.added {
            target.imports.push(ast::Import {
                path: path.split('.').map(ident).collect(),
                alias: Some(ident(alias)),
                span,
                line: span,
            });
        }
        let name = format!("{module_name}.interfaces.derived.to");
        let id = sources.add(Path::new(&name), writer.text);
        sources.set_identity(id, format!("<derived>/{name}"), module_name.clone());
        let generated = parser::parse_file(sources.file(id), diags);
        target.items.extend(generated.items);
    }
}

/// A Tondo type that implements interfaces of C: where the first of its
/// implementations is, the type as that file names it, and the interfaces.
struct Implemented {
    module: usize,
    file: usize,
    target: String,
    interfaces: Vec<usize>,
}

/// The types of the program that implement interfaces of C, by the
/// `impl`s of them, but generic ones, which C cannot be given.
fn implementations(modules: &[ModuleSource], by_name: &HashMap<(String, String), usize>) -> Vec<Implemented> {
    let mut found: Vec<Implemented> = Vec::new();
    for (module, source) in modules.iter().enumerate() {
        let own = source.path.join(".");
        for (at, (file, _)) in source.files.iter().enumerate() {
            let aliases = imported(file);
            for item in &file.items {
                let ast::Item::Impl(decl) = item else { continue };
                let ast::TypeKind::Path { segments, args } = &decl.trait_ref.kind else { continue };
                let named = match segments.as_slice() {
                    [name] => (own.clone(), name.name.clone()),
                    [alias, name] => match aliases.get(&alias.name) {
                        Some(path) => (path.clone(), name.name.clone()),
                        None => continue,
                    },
                    _ => continue,
                };
                let Some(&interface) = by_name.get(&named) else { continue };
                let ast::TypeKind::Path { segments: target, args: target_args } = &decl.target.kind else { continue };
                if !args.is_empty() || !target_args.is_empty() || !decl.generics.is_empty() {
                    continue;
                }
                let target: Vec<&str> = target.iter().map(|s| s.name.as_str()).collect();
                let target = target.join(".");
                // One type of one module, by how its first `impl` names it.
                match found.iter_mut().find(|t| t.module == module && t.target == target) {
                    Some(known) if !known.interfaces.contains(&interface) => known.interfaces.push(interface),
                    Some(_) => {}
                    None => found.push(Implemented { module, file: at, target, interfaces: vec![interface] }),
                }
            }
        }
    }
    found
}

/// The traits with `@ffi.Interface` before them, as written, in the shape
/// an interface has; the checker says what is wrong with the others.
fn interfaces(modules: &[ModuleSource]) -> Vec<Interface> {
    let mut found = Vec::new();
    for (module, source) in modules.iter().enumerate() {
        let own = source.path.join(".") == "std.ffi";
        for (at, (file, _)) in source.files.iter().enumerate() {
            let ffi = crate::derive::alias_of(file, "ffi");
            if ffi.is_none() && !own {
                continue;
            }
            let aliases = imported(file);
            for item in &file.items {
                let ast::Item::Trait(decl) = item else { continue };
                let marked = decl.uses.iter().find(|u| match u.path.as_slice() {
                    [alias, name] => Some(&alias.name) == ffi.as_ref() && name.name == "Interface",
                    [name] => own && name.name == "Interface",
                    _ => false,
                });
                let Some(marked) = marked else { continue };
                if !decl.generics.is_empty() || decl.requires.len() > 1 {
                    continue;
                }
                let base = match decl.requires.first().map(|r| &r.kind) {
                    None => None,
                    Some(ast::TypeKind::Path { segments, args }) if args.is_empty() => match segments.as_slice() {
                        [name] => Some((source.path.join("."), name.name.clone())),
                        [alias, name] => match aliases.get(&alias.name) {
                            Some(path) => Some((path.clone(), name.name.clone())),
                            None => continue,
                        },
                        _ => continue,
                    },
                    Some(_) => continue,
                };
                let Some(methods) = decl.methods.iter().map(method).collect::<Option<Vec<Method>>>() else { continue };
                let id =
                    marked.args.iter().find(|a| a.name.as_ref().is_some_and(|n| n.name == "id")).and_then(|a| match &a
                        .value
                        .kind
                    {
                        ast::ExprKind::Str(segments) => match segments.as_slice() {
                            [ast::StrSegment::Text(text)] => Some(text.clone()),
                            _ => None,
                        },
                        _ => None,
                    });
                let name = decl.name.name.rsplit('.').next().unwrap_or(&decl.name.name).to_string();
                found.push(Interface { module, file: at, name, methods, base, id });
            }
        }
    }
    found
}

/// A method in the shape an interface's has: `self` and parameters by
/// value, without type parameters.
fn method(f: &ast::FnDecl) -> Option<Method> {
    let [receiver, params @ ..] = f.params.as_slice() else { return None };
    if receiver.ty.is_some() || receiver.mode != ast::Mode::Ref || !f.generics.is_empty() {
        return None;
    }
    let mut named = Vec::new();
    for param in params {
        if param.mode != ast::Mode::Value || param.variadic {
            return None;
        }
        named.push((param.name.name.clone(), param.ty.clone()?));
    }
    Some(Method { name: f.name.name.clone(), params: named, result: f.result.clone() })
}

/// The 16 bytes of a GUID written `8-4-4-4-12`, in braces or not, as
/// `ffi.Guid` lays them: three numbers, then eight bytes.
pub fn guid(text: &str) -> Option<(u32, u16, u16, [u8; 8])> {
    let inner = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')).unwrap_or(text);
    let parts: Vec<&str> = inner.split('-').collect();
    let sizes: Vec<usize> = parts.iter().map(|p| p.len()).collect();
    if sizes != [8, 4, 4, 4, 12] || !inner.chars().all(|c| c == '-' || c.is_ascii_hexdigit()) {
        return None;
    }
    let tail = format!("{}{}", parts[3], parts[4]);
    let mut data4 = [0u8; 8];
    for (at, byte) in data4.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&tail[at * 2..at * 2 + 2], 16).ok()?;
    }
    Some((
        u32::from_str_radix(parts[0], 16).ok()?,
        u16::from_str_radix(parts[1], 16).ok()?,
        u16::from_str_radix(parts[2], 16).ok()?,
        data4,
    ))
}

/// The derived text of a file, and the modules it imports for it.
struct Writer {
    text: String,
    /// The file's module and file, and the modules its aliases name.
    module: String,
    file: usize,
    aliases: HashMap<String, String>,
    /// The imports it adds, a module and its alias.
    added: Vec<(String, String)>,
}

impl Writer {
    fn new(module: &ModuleSource, file: usize) -> Writer {
        let mut aliases: HashMap<String, String> = HashMap::new();
        for (alias, path) in imported(&module.files[file].0) {
            aliases.entry(path).or_insert(alias);
        }
        Writer { text: String::new(), module: module.path.join("."), file, aliases, added: Vec::new() }
    }

    /// The alias this file names `module` by, imported for it if needed.
    fn alias(&mut self, module: &str) -> String {
        if let Some(alias) = self.aliases.get(module) {
            return alias.clone();
        }
        let alias = format!("__interface{}", self.added.len());
        self.added.push((module.to_string(), alias.clone()));
        self.aliases.insert(module.to_string(), alias.clone());
        alias
    }

    /// A type that `from`'s file writes, as this file writes it.
    fn written(&mut self, ty: &ast::TypeExpr, from: &Origin, sources: &SourceMap) -> String {
        if (from.module.as_str(), from.file) == (self.module.as_str(), self.file) {
            return sources.slice(ty.span).to_string();
        }
        match &ty.kind {
            ast::TypeKind::Path { segments, args } => {
                let mut path: Vec<String> = segments.iter().map(|s| s.name.clone()).collect();
                if let [name] = path.as_slice()
                    && from.declared.contains(name)
                    && from.module != self.module
                {
                    path.insert(0, self.alias(&from.module));
                } else if path.len() > 1
                    && let Some(module) = from.aliases.get(&path[0])
                {
                    path[0] = self.alias(module);
                }
                let args: Vec<String> = args.iter().map(|a| self.written(a, from, sources)).collect();
                match args.is_empty() {
                    true => path.join("."),
                    false => format!("{}[{}]", path.join("."), args.join(", ")),
                }
            }
            ast::TypeKind::Optional(inner) => format!("{}?", self.written(inner, from, sources)),
            ast::TypeKind::Result { ok, err } => {
                let err = self.written(err, from, sources);
                match ok {
                    Some(ok) => format!("{} ! {err}", self.written(ok, from, sources)),
                    None => format!("!{err}"),
                }
            }
            ast::TypeKind::Tuple(items) => {
                let items: Vec<String> = items.iter().map(|i| self.written(i, from, sources)).collect();
                format!("({})", items.join(", "))
            }
            ast::TypeKind::Fn { params, result, is_unsafe, is_extern, .. } => {
                let params: Vec<String> = params.iter().map(|p| self.written(p, from, sources)).collect();
                let result = result.as_ref().map(|r| format!(": {}", self.written(r, from, sources)));
                let unsafe_ = if *is_unsafe { "unsafe " } else { "" };
                let extern_ = if *is_extern { "extern " } else { "" };
                format!("{unsafe_}{extern_}fn({}){}", params.join(", "), result.unwrap_or_default())
            }
            _ => sources.slice(ty.span).to_string(),
        }
    }

    /// The object of `interface`, and its implementations of the traits of
    /// its chain, which starts with the one that requires no other.
    fn object(
        &mut self,
        interface: &Interface,
        chain: &[&Interface],
        modules: &[ModuleSource],
        declared: &[HashSet<String>],
        sources: &SourceMap,
    ) {
        let name = &interface.name;
        // `std.ffi`'s names, through its alias, or as they are in itself.
        let ffi = match self.module == "std.ffi" {
            true => String::new(),
            false => format!("{}.", self.alias("std.ffi")),
        };
        let object = format!("{name}Object");
        let mut out = format!(
            "/// An object of C that is a `{name}`, as C gives it: its first word points\n\
             /// to the table of a `{name}`'s functions, which its methods call.\n\
             pub type {object} = {{\n    priv object: Pointer[{ffi}Void]\n}}\n\n\
             /// The `{name}` of an object of C, which nothing checks is one.\n\
             pub unsafe fn {object}.of(pointer: Pointer[{ffi}Void]): {object} {{\n    {object} {{ object: pointer }}\n}}\n\n\
             /// The object, for a function of C that takes it.\n\
             pub fn {object}.pointer(self): Pointer[{ffi}Void] {{\n    self.object\n}}\n\n"
        );
        if let Some((data1, data2, data3, data4)) = interface.id.as_deref().and_then(guid) {
            let bytes: Vec<String> = data4.iter().map(|b| format!("Byte({b}u8)")).collect();
            out.push_str(&format!(
                "/// The identity of `{name}`, which `queryInterface` asks for.\n\
                 pub const {object}.Id: {ffi}Guid = {ffi}Guid {{ data1: {data1}, data2: {data2}, data3: {data3}, data4: [{}] }}\n\n",
                bytes.join(", ")
            ));
        }
        // The interface it requires, by a method named as it is.
        if let [.., base, _] = chain {
            let qualified = self.named(base, modules);
            let method = lower_first(&base.name);
            out.push_str(&format!(
                "/// The same object as a `{}`.\npub fn {object}.{method}(self): {qualified}Object {{\n    \
                 unsafe {{ {qualified}Object.of(self.object) }}\n}}\n\n",
                base.name
            ));
        }
        let mut index = 0;
        for &member in chain {
            let origin = Origin {
                module: modules[member.module].path.join("."),
                file: member.file,
                declared: &declared[member.module],
                aliases: imported(&modules[member.module].files[member.file].0),
            };
            let trait_name = self.named(member, modules);
            out.push_str(&format!("impl {trait_name} for {object} {{\n"));
            for method in &member.methods {
                out.push_str(&self.call(method, index, &origin, &ffi, sources));
                index += 1;
            }
            out.push_str("}\n\n");
        }
        self.text.push_str(&out);
    }

    /// What `ffi.object` makes of a Tondo type `target` that implements
    /// interfaces of C, one chain each: a block with a word for each chain's
    /// table, in which each entry finds the value from the object it is
    /// given, and what it holds, the value and a count. For a chain that
    /// requires `Unknown`, its three are written here, over that count.
    fn implemented(
        &mut self,
        target: &str,
        chains: &[Vec<&Interface>],
        modules: &[ModuleSource],
        declared: &[HashSet<String>],
        sources: &SourceMap,
    ) {
        let ffi = match self.module == "std.ffi" {
            true => String::new(),
            false => format!("{}.", self.alias("std.ffi")),
        };
        let sync = self.alias("std.sync");
        let own = format!("__{}", target.replace('.', "_"));
        let (held, block) = (format!("{own}ForC"), format!("{own}Block"));
        let unknown = |interface: &Interface, modules: &[ModuleSource]| {
            interface.name == "Unknown" && modules[interface.module].path.join(".") == "std.ffi"
        };
        let counted = chains.iter().position(|chain| chain.iter().any(|i| unknown(i, modules)));
        let mut out = format!(
            "/// What `ffi.object` keeps of a `{target}` for C: the value, and how many hold it.\n\
             type {held} = {{\n    value: {target}\n    count: {sync}.Atomic[Int]\n}}\n\n"
        );
        let mut fields = Vec::new();
        let mut entries = String::new();
        let mut tables = Vec::new();
        for (slot, chain) in chains.iter().enumerate() {
            let table = format!("{own}Table{slot}");
            let mut columns = Vec::new();
            let mut made = Vec::new();
            for &member in chain {
                let origin = Origin {
                    module: modules[member.module].path.join("."),
                    file: member.file,
                    declared: &declared[member.module],
                    aliases: imported(&modules[member.module].files[member.file].0),
                };
                for method in &member.methods {
                    let entry = format!("{own}Entry{slot}_{}", made.len());
                    let (signature, text) = self.entry(EntryOf {
                        method,
                        entry: &entry,
                        slot,
                        origin: &origin,
                        counting: unknown(member, modules),
                        held: &held,
                        block: &block,
                        chains,
                        ffi: &ffi,
                        modules,
                        sources,
                    });
                    columns.push(format!("    {}: {signature}", method.name));
                    made.push(format!("{}: {entry}", method.name));
                    entries.push_str(&text);
                }
            }
            out.push_str(&format!(
                "/// The table of `{}` of an object of C made of a `{target}`.\nextern type {table} = {{\n{}\n}}\n\n",
                chain.last().map_or("", |i| i.name.as_str()),
                columns.join("\n")
            ));
            fields.push(format!("    table{slot}: Pointer[{table}]"));
            tables.push((table, made));
        }
        out.push_str(&format!(
            "/// An object of C made of a `{target}`: a word for each of its tables, then what it holds.\n\
             extern type {block} = {{\n{}\n    held: Pointer[{ffi}Void]\n}}\n\n",
            fields.join("\n")
        ));
        out.push_str(&entries);
        let mut making = String::new();
        for (slot, (table, made)) in tables.iter().enumerate() {
            making.push_str(&format!(
                "        let __table{slot} = {ffi}allocate[{table}]()\n        unsafe {{ __table{slot}.write({table} {{ {} }}) }}\n",
                made.join(", ")
            ));
        }
        let given: Vec<String> = (0..tables.len()).map(|slot| format!("table{slot}: __table{slot}")).collect();
        out.push_str(&format!(
            "impl {ffi}Implemented for {target} {{\n    fn objectForC(self): Pointer[{ffi}Void] {{\n{making}        \
             let __held = {ffi}context({held} {{ value: self, count: {sync}.Atomic(1) }})\n        \
             let __block = {ffi}allocate[{block}]()\n        \
             unsafe {{\n            __block.write({block} {{ {}, held: __held }})\n            __block.cast[{ffi}Void]()\n        }}\n    }}\n}}\n\n",
            given.join(", ")
        ));
        // What the trait asks of the value itself, which C does not call.
        if counted.is_some() {
            out.push_str(&format!(
                "impl {ffi}Unknown for {target} {{\n    \
                 fn queryInterface(self, id: Pointer[{ffi}Guid], found: Pointer[Pointer[{ffi}Void]]): {ffi}HResult {{\n        \
                 {ffi}HResult.NotImplemented\n    }}\n    fn addRef(self): UInt32 {{\n        1u32\n    }}\n    \
                 fn release(self): UInt32 {{\n        1u32\n    }}\n}}\n\n"
            ));
        }
        self.text.push_str(&out);
    }

    /// The function of C that entry `slot` of a table calls for `method`:
    /// its type, as the table's column, and its text.
    fn entry(&mut self, of: EntryOf) -> (String, String) {
        let EntryOf { method, entry, slot, origin, counting, held, block, chains, ffi, modules, sources } = of;
        let params: Vec<(String, String)> =
            method.params.iter().map(|(name, ty)| (name.clone(), self.written(ty, origin, sources))).collect();
        let mut declared: Vec<String> = vec![format!("__object: Pointer[{ffi}Void]")];
        declared.extend(params.iter().map(|(name, ty)| format!("{name}: {ty}")));
        let mut types: Vec<String> = vec![format!("Pointer[{ffi}Void]")];
        types.extend(params.iter().map(|(_, ty)| ty.clone()));
        let args: Vec<&str> = params.iter().map(|(name, _)| name.as_str()).collect();
        let call = format!("__held.value.{}({})", method.name, args.join(", "));
        let (result, body) = match (counting, method.name.as_str(), method.result.as_ref().map(|r| &r.kind)) {
            (true, "addRef", _) => ("UInt32".to_string(), "UInt32(__held.count.add(1)) ?? 0u32\n".to_string()),
            (true, "release", _) => {
                let frees: String =
                    (0..chains.len()).map(|at| format!("            {ffi}free(__block.read().table{at})\n")).collect();
                let body = format!(
                    "let __left = __held.count.add(-1)\n        if __left == 0 {{\n{frees}            \
                     {ffi}freeContext[{held}](__block.read().held)\n            {ffi}free(__block)\n        }}\n        \
                     UInt32(__left) ?? 0u32\n"
                );
                ("UInt32".to_string(), body)
            }
            (true, "queryInterface", _) => {
                let (id, object) = (&params[0].0, &params[1].0);
                let mut found = String::new();
                for (at, chain) in chains.iter().enumerate() {
                    for member in chain {
                        if member.id.as_deref().and_then(guid).is_none() {
                            continue;
                        }
                        let named = self.named(member, modules);
                        found.push_str(&format!(
                            "        if __slot < 0 and __asked == {named}Object.Id {{\n            __slot = {at}\n        }}\n"
                        ));
                    }
                }
                let body = format!(
                    "let __asked = {id}.read()\n        var __slot = -1\n{found}        if __slot < 0 {{\n            \
                     {object}.cast[UInt64]().write(0u64)\n            return {ffi}HResult.NoInterface\n        }}\n        \
                     {object}.write(__block.cast[UInt64]().offset(__slot).cast[{ffi}Void]())\n        \
                     _ = __held.count.add(1)\n        {ffi}HResult.Ok\n"
                );
                (format!("{ffi}HResult"), body)
            }
            (_, _, Some(ast::TypeKind::Result { ok: Some(ok), .. })) => {
                let value = self.written(ok, origin, sources);
                declared.push(format!("__out: Pointer[{value}]"));
                types.push(format!("Pointer[{value}]"));
                let body = format!(
                    "match {call} {{\n            ok(__value) => {{\n                __out.write(__value)\n                \
                     {ffi}HResult.Ok\n            }}\n            err(__failed) => __failed\n        }}\n"
                );
                (format!("{ffi}HResult"), body)
            }
            (_, _, Some(ast::TypeKind::Result { ok: None, .. })) => {
                let body = format!(
                    "match {call} {{\n            ok(_) => {ffi}HResult.Ok\n            err(__failed) => __failed\n        }}\n"
                );
                (format!("{ffi}HResult"), body)
            }
            (_, _, Some(_)) => (self.written(method.result.as_ref().unwrap(), origin, sources), format!("{call}\n")),
            (_, _, None) => (String::new(), format!("{call}\n")),
        };
        let given = if result.is_empty() { String::new() } else { format!(": {result}") };
        let signature = format!("extern fn({}){given}", types.join(", "));
        let text = format!(
            "fn {entry}({}){given} {{\n    unsafe {{\n        \
             let __block = __object.cast[UInt64]().offset(-{slot}).cast[{block}]()\n        \
             let __held = {ffi}fromContext[{held}](__block.read().held)\n        {body}    }}\n}}\n\n",
            declared.join(", ")
        );
        (signature, text)
    }

    /// How this file names an interface: by its name in its own module,
    /// or through the alias of the module that declares it.
    fn named(&mut self, interface: &Interface, modules: &[ModuleSource]) -> String {
        let module = modules[interface.module].path.join(".");
        match module == self.module {
            true => interface.name.clone(),
            false => format!("{}.{}", self.alias(&module), interface.name),
        }
    }

    /// A method that calls the table's entry `index` with the object first.
    /// One that gives `!ffi.HResult` fails when the number is negative; one
    /// that gives `T ! ffi.HResult` passes C a last parameter for its `T`.
    fn call(&mut self, method: &Method, index: usize, origin: &Origin, ffi: &str, sources: &SourceMap) -> String {
        let params: Vec<(String, String)> =
            method.params.iter().map(|(name, ty)| (name.clone(), self.written(ty, origin, sources))).collect();
        let declared: Vec<String> = params.iter().map(|(name, ty)| format!("{name}: {ty}")).collect();
        let mut types: Vec<String> = vec![format!("Pointer[{ffi}Void]")];
        types.extend(params.iter().map(|(_, ty)| ty.clone()));
        let mut args: Vec<String> = vec!["self.object".into()];
        args.extend(params.iter().map(|(name, _)| name.clone()));
        let result = method.result.as_ref().map(|r| self.written(r, origin, sources));
        let (signature, body) = match method.result.as_ref().map(|r| &r.kind) {
            Some(ast::TypeKind::Result { ok, .. }) => {
                let fails = "        if result.isFailure() {\n";
                match ok {
                    Some(ok) => {
                        let value = self.written(ok, origin, sources);
                        types.push(format!("Pointer[{value}]"));
                        args.push("out".into());
                        let body = format!(
                            "        let out = {ffi}allocate[{value}]()\n        let result = call({})\n{fails}            \
                             {ffi}free(out)\n            fail result\n        }}\n        let value = out.read()\n        \
                             {ffi}free(out)\n        value\n",
                            args.join(", ")
                        );
                        (format!("extern fn({}): {ffi}HResult", types.join(", ")), body)
                    }
                    None => {
                        let body = format!(
                            "        let result = call({})\n{fails}            fail result\n        }}\n",
                            args.join(", ")
                        );
                        (format!("extern fn({}): {ffi}HResult", types.join(", ")), body)
                    }
                }
            }
            _ => {
                let given = result.as_ref().map(|r| format!(": {r}")).unwrap_or_default();
                (format!("extern fn({}){given}", types.join(", ")), format!("        call({})\n", args.join(", ")))
            }
        };
        let given = result.map(|r| format!(": {r}")).unwrap_or_default();
        format!(
            "    fn {}(self{}{}){given} {{\n        unsafe {{\n        let table = self.object.cast[UInt64]().read().toPointer[UInt64]()\n        \
             let call = table.offset({index}).read().toFunction[{signature}]()\n{body}        }}\n    }}\n",
            method.name,
            if declared.is_empty() { "" } else { ", " },
            declared.join(", ")
        )
    }
}

/// What an entry of a table is written from.
struct EntryOf<'a> {
    method: &'a Method,
    entry: &'a str,
    slot: usize,
    origin: &'a Origin<'a>,
    /// Whether it is one of `Unknown`'s, which counts.
    counting: bool,
    held: &'a str,
    block: &'a str,
    chains: &'a [Vec<&'a Interface>],
    ffi: &'a str,
    modules: &'a [ModuleSource],
    sources: &'a SourceMap,
}

/// Where a signature is written: its module, what the module declares, and
/// the modules its file's aliases name.
struct Origin<'a> {
    module: String,
    file: usize,
    declared: &'a HashSet<String>,
    aliases: HashMap<String, String>,
}

/// `unknown` for `Unknown`, `profilerInfo` for `ProfilerInfo`.
fn lower_first(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map(|first| first.to_lowercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_guid() {
        let unknown = guid("00000000-0000-0000-C000-000000000046").unwrap();
        assert_eq!(unknown, (0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]));
        assert_eq!(guid("{176FBED1-A55C-4796-98CA-A9DA0EF883E7}").unwrap().0, 0x176f_bed1);
        assert!(guid("176FBED1-A55C-4796-98CA").is_none() && guid("Z76FBED1-A55C-4796-98CA-A9DA0EF883E7").is_none());
    }
}
