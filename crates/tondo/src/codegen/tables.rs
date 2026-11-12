//! The tables a program keeps of itself: its trace, dumps, debug information and tests.

use super::*;

impl Codegen<'_> {
    /// The stack trace table (the runtime's `trace::Table`): each function's
    /// code and size, and in one blob its name, its calls, its lines and
    /// the calls inlined into it, then the names they refer to: their
    /// files, the types the calls assign and the functions inlined. A function's lines are its file and, in as many bytes as
    /// they take, where each run of code of one line starts and its line,
    /// each as the difference from the run before, in LEB128, the line's
    /// zigzag; line `0` is code of no line.
    pub(super) fn trace_table(&mut self, id: DataId) {
        let pad = |blob: &mut Vec<u8>| blob.resize(blob.len().next_multiple_of(4), 0);
        let mut blob = Vec::new();
        // The texts the table names, files and types, each once, by its
        // number in the order first named.
        let mut files: Vec<String> = Vec::new();
        let mut numbers: HashMap<String, u32> = HashMap::new();
        let mut name = |files: &mut Vec<String>, text: String| {
            *numbers.entry(text).or_insert_with_key(|text| {
                files.push(text.clone());
                files.len() as u32 - 1
            })
        };
        let mut entries = vec![0u8; self.traced.len() * 16];
        for (index, function) in self.traced.iter().enumerate() {
            let meta = blob.len() as u32;
            entries[index * 16 + 8..index * 16 + 12].copy_from_slice(&function.size.to_le_bytes());
            entries[index * 16 + 12..index * 16 + 16].copy_from_slice(&meta.to_le_bytes());
            blob.extend_from_slice(&(function.name.len() as u32).to_le_bytes());
            blob.extend_from_slice(&(function.calls.len() as u32).to_le_bytes());
            blob.extend_from_slice(function.name.as_bytes());
            pad(&mut blob);
            for &(offset, site) in &function.calls {
                let (source, ty, scope) = function.sites[site as usize - 1];
                let (file, line, column) = self.sources.position(source);
                let file = name(&mut files, file);
                let ty = match ty {
                    Some(ty) => name(&mut files, self.types.display(ty)),
                    None => u32::MAX,
                };
                for value in [offset, line as u32, column as u32, file, ty, scope.unwrap_or(u32::MAX)] {
                    blob.extend_from_slice(&value.to_le_bytes());
                }
            }
            // Each run of code of one line, and of one inlined call: its
            // line is in the inlined function's file. Code of no statement,
            // as the moves and jumps between two, is the one before's.
            let mut file = None;
            let placed: Vec<(u32, Option<(i64, u32)>)> = function
                .lines
                .iter()
                .map(|&(start, site)| {
                    let placed = (site != 0).then(|| {
                        let (source, _, scope) = function.sites[site as usize - 1];
                        let (path, line, _) = self.sources.position(source);
                        if scope.is_none() {
                            file.get_or_insert(path);
                        }
                        (line as i64, scope.map_or(0, |scope| scope + 1))
                    });
                    (start, placed)
                })
                .collect();
            // Code before the first statement is the first statement's;
            // a debugger's builds keep it of none, which it does not stop in.
            let mut known = placed.iter().find_map(|&(_, placed)| placed).unwrap_or((0, 0));
            let mut runs: Vec<(u32, i64, u32)> = Vec::new();
            for (start, placed) in placed {
                let (line, scope) = placed.unwrap_or(if self.debug.is_some() { (0, 0) } else { known });
                known = (line, scope);
                if runs.last().is_none_or(|&(_, last, inlined)| (last, inlined) != (line, scope)) {
                    runs.push((start, line, scope));
                }
            }
            let mut encoded = Vec::new();
            let (mut offset, mut line) = (0u32, 0i64);
            for (start, now, scope) in runs {
                leb128(&mut encoded, (start - offset) as u64);
                leb128(&mut encoded, ((now - line) << 1 ^ (now - line) >> 63) as u64);
                leb128(&mut encoded, scope as u64);
                (offset, line) = (start, now);
            }
            let file = file.map_or(u32::MAX, |path| name(&mut files, path));
            blob.extend_from_slice(&file.to_le_bytes());
            blob.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
            blob.extend_from_slice(&encoded);
            pad(&mut blob);
            // Each inlined call: the function inlined, where it was called,
            // the inlined call that holds it, and the inlined function's file.
            for inlined in &function.inlined {
                let (path, line, column) = self.sources.position(inlined.call.start);
                let (called, path) = (name(&mut files, inlined.name.clone()), name(&mut files, path));
                let own = name(&mut files, self.sources.position(inlined.span.start).0);
                let parent = inlined.parent.unwrap_or(u32::MAX);
                for value in [called, line as u32, column as u32, path, parent, own] {
                    blob.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        let files_at = blob.len() as u64;
        blob.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for file in &files {
            blob.extend_from_slice(&(file.len() as u32).to_le_bytes());
            blob.extend_from_slice(file.as_bytes());
            pad(&mut blob);
        }
        let define = |module: &mut ObjectModule, name: &str, bytes: Vec<u8>| {
            let id = module.declare_data(name, Linkage::Local, false, false).expect("trace data");
            let mut data = DataDescription::new();
            data.define(bytes.into_boxed_slice());
            data.set_align(8);
            module.define_data(id, &data).expect("trace data definition");
            id
        };
        let blob = define(&mut self.module, "tondo_trace_blob", blob);
        let functions = self.module.declare_data("tondo_trace_functions", Linkage::Local, false, false).unwrap();
        let mut data = DataDescription::new();
        data.define(entries.into_boxed_slice());
        data.set_align(8);
        for (index, function) in self.traced.iter().enumerate() {
            let code = self.module.declare_func_in_data(function.id, &mut data);
            data.write_function_addr(index as u32 * 16, code);
        }
        self.module.define_data(functions, &data).expect("trace functions");
        // Each `try_call`'s handler: the function, its return address and
        // the handler, in the order of the functions and their code.
        let mut handlers = Vec::new();
        for (index, function) in self.traced.iter().enumerate() {
            for &(ret, at) in &function.handlers {
                for value in [index as u32, ret, at] {
                    handlers.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        let handled = handlers.len() as u64 / 12;
        let handlers = define(&mut self.module, "tondo_trace_handlers", handlers);
        let mut header = vec![0u8; 64];
        header[..16].copy_from_slice(crate::cores::MAGIC);
        header[24..32].copy_from_slice(&(self.traced.len() as u64).to_le_bytes());
        header[40..48].copy_from_slice(&files_at.to_le_bytes());
        header[56..64].copy_from_slice(&handled.to_le_bytes());
        let mut data = DataDescription::new();
        data.define(header.into_boxed_slice());
        data.set_align(8);
        let functions = self.module.declare_data_in_data(functions, &mut data);
        let blob = self.module.declare_data_in_data(blob, &mut data);
        let handlers = self.module.declare_data_in_data(handlers, &mut data);
        data.write_data_addr(16, functions, 0);
        data.write_data_addr(32, blob, 0);
        data.write_data_addr(48, handlers, 0);
        self.module.define_data(id, &data).expect("trace table");
    }

    /// A debug build's table (the runtime's `debugger::Table`): a text of
    /// lines, `F` and a function's name, `L` a binding's name and type for
    /// each it shows, and `S` a probe's file, line and function; and the
    /// glue that shows each binding, in the order of the `L` lines.
    pub(super) fn dump_table(&mut self, id: DataId) {
        let (mut text, mut glues) = (crate::cores::BINDINGS.to_string(), Vec::new());
        let (mut shapes, mut known) = (Vec::new(), HashMap::new());
        let bound = self.traced.iter().filter_map(|function| self.dump_bindings.get(&function.body));
        let shaped: Vec<Ty> = bound.flatten().map(|binding| binding.1).collect();
        for ty in shaped {
            self.shape(ty, &mut shapes, &mut known);
        }
        for (index, function) in self.traced.iter().enumerate() {
            text.push_str(&format!("F\t{index}\n"));
            for (register, at) in &function.saved {
                text.push_str(&format!("C\t{register}\t{at}\n"));
            }
            for &(at, length, site) in &function.latent {
                let Some(&(start, ..)) = function.sites.get((site as usize).wrapping_sub(1)) else { continue };
                let (file, line, _) = self.sources.position(start);
                text.push_str(&format!("P\t{at}\t{length}\t{file}\t{line}\n"));
            }
            let Some(bindings) = self.dump_bindings.get(&function.body) else { continue };
            for (name, ty, glue, place, keep, scope, from) in bindings {
                glues.push(*glue);
                let shaped = ty;
                let ty = self.types.display(*ty);
                let keep = keep.map_or(-1, i64::from);
                let from = from.map_or(0, |from| self.sources.position(from.start).1);
                let shape = known[shaped];
                let keep = format!("{keep}\t{}\t{from}\t{shape}", scope.map_or(-1, i64::from));
                match *place {
                    DumpPlace::Slot(slot) => {
                        let Some((_, at)) = function.slots.iter().find(|(s, _)| *s == slot) else {
                            glues.pop();
                            continue;
                        };
                        text.push_str(&format!("B\t{name}\t{ty}\ts\t{at}\t{keep}\n"));
                    }
                    DumpPlace::Value(label) | DumpPlace::Pointer(label) => {
                        let pointer = u8::from(matches!(place, DumpPlace::Pointer(_)));
                        text.push_str(&format!("B\t{name}\t{ty}\tv\t{pointer}\t{keep}\n"));
                        for &(_, start, end, located) in function.labelled.iter().filter(|l| l.0 == label) {
                            match located {
                                Located::Frame(at) => text.push_str(&format!("R\t{start}\t{end}\tf\t{at}\n")),
                                Located::Register(number, true) => {
                                    text.push_str(&format!("R\t{start}\t{end}\tr\t{number}\n"));
                                }
                                Located::Register(number, false) => {
                                    text.push_str(&format!("R\t{start}\t{end}\tx\t{number}\n"));
                                }
                            }
                        }
                    }
                }
            }
        }
        for shape in shapes {
            text.push_str(&format!("Y\t{shape}\n"));
        }
        let text = self.string(&text);
        let array = self.module.declare_data("tondo_dump_glue", Linkage::Local, false, false).expect("dump glue");
        let mut data = DataDescription::new();
        data.define(vec![0u8; glues.len().max(1) * 8].into_boxed_slice());
        data.set_align(8);
        for (index, &glue) in glues.iter().enumerate() {
            let code = self.module.declare_func_in_data(glue, &mut data);
            data.write_function_addr(index as u32 * 8, code);
        }
        self.module.define_data(array, &data).expect("dump glue definition");
        let mut header = vec![0u8; 24];
        header[16..24].copy_from_slice(&(glues.len() as u64).to_le_bytes());
        let mut data = DataDescription::new();
        data.define(header.into_boxed_slice());
        data.set_align(8);
        let text = self.module.declare_data_in_data(text, &mut data);
        let array = self.module.declare_data_in_data(array, &mut data);
        data.write_data_addr(0, text, 0);
        data.write_data_addr(8, array, 0);
        self.module.define_data(id, &data).expect("dump table");
    }

    pub(super) fn debug_table(&mut self, id: DataId) {
        let mut text = String::new();
        let mut glues = Vec::new();
        let (mut layouts, mut known) = (Vec::new(), HashMap::new());
        for (index, (name, lends, bindings)) in self.debug_functions.clone().into_iter().enumerate() {
            text.push_str(&format!("F\t{name}\t{}\n", u8::from(lends)));
            if let Some((alive, addresses)) = self.debug_slots.get(&(index as u32)) {
                text.push_str(&format!("O\t{addresses}\t{alive}\n"));
            }
            for (binding, ty, glue, raw) in bindings {
                let layout = self.write_layout(ty, &mut layouts, &mut known);
                let viewed = u8::from(raw.is_some());
                text.push_str(&format!("L\t{binding}\t{}\t{layout}\t{viewed}\n", self.types.display(ty)));
                glues.push(glue);
                glues.extend(raw);
            }
        }
        for layout in layouts {
            text.push_str(&format!("D\t{layout}\n"));
        }
        if self.types.any_secret() {
            text.push_str("H\n");
        }
        for &(start, function, goes_on) in &self.debug_sites {
            let (file, line, _) = self.sources.position(start);
            text.push_str(&format!("S\t{file}\t{line}\t{function}\t{}\n", u8::from(goes_on)));
        }
        let text = self.string(&text);
        let array = self.module.declare_data("tondo_debug_glue", Linkage::Local, false, false).expect("debug glue");
        let mut data = DataDescription::new();
        data.define(vec![0u8; glues.len().max(1) * 8].into_boxed_slice());
        data.set_align(8);
        for (index, &glue) in glues.iter().enumerate() {
            let code = self.module.declare_func_in_data(glue, &mut data);
            data.write_function_addr(index as u32 * 8, code);
        }
        self.module.define_data(array, &data).expect("debug glue definition");
        let mut header = vec![0u8; 24];
        header[16..24].copy_from_slice(&(glues.len() as u64).to_le_bytes());
        let mut data = DataDescription::new();
        data.define(header.into_boxed_slice());
        data.set_align(8);
        let text = self.module.declare_data_in_data(text, &mut data);
        let array = self.module.declare_data_in_data(array, &mut data);
        data.write_data_addr(0, text, 0);
        data.write_data_addr(8, array, 0);
        self.module.define_data(id, &data).expect("debug table");
    }

    /// How the debugger writes into a value of `ty` (toolchain 7.6), as the
    /// number of its line among `layouts`: a number, a truth, a character
    /// or a text by its type's name; a record's or a tuple's fields by
    /// their offsets, and whether each is behind a pointer; a plain enum by
    /// the size of its tag and its variants; an array by the size of its
    /// elements; a `std.sync` cell, which a watch follows into every task;
    /// and `-` for what it does not write into.
    pub(super) fn write_layout(&mut self, ty: Ty, layouts: &mut Vec<String>, known: &mut HashMap<Ty, usize>) -> usize {
        if let Some(&id) = known.get(&ty) {
            return id;
        }
        let id = layouts.len();
        layouts.push("-".into());
        known.insert(ty, id);
        let fields = |cg: &mut Self, names: Vec<String>, layouts: &mut Vec<String>, known: &mut HashMap<Ty, usize>| {
            let mut out = String::from("r");
            for (index, name) in names.into_iter().enumerate() {
                let slot = cg.slot(ty, Proj::Field(index as u32));
                let inner = cg.write_layout(slot.ty, layouts, known);
                let boxed = if slot.boxed { "*" } else { "" };
                out.push_str(&format!(" {name}={}:{boxed}{inner}", slot.offset));
            }
            out
        };
        layouts[id] = match self.types.kind(ty).clone() {
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Byte | TyKind::String => {
                format!("s {}", self.types.display(ty))
            }
            TyKind::Tuple(items) => fields(self, (0..items.len()).map(|i| i.to_string()).collect(), layouts, known),
            TyKind::Array(element) => {
                let inner = self.write_layout(element, layouts, known);
                format!("a {} {inner}", self.layout(element).size)
            }
            TyKind::Handle(Handle::Mutex | Handle::RwLock | Handle::Atomic, _) => "h".into(),
            TyKind::Adt(adt) => match self.types.adt(adt).kind.clone() {
                AdtKind::Record(declared) => {
                    fields(self, declared.into_iter().map(|f| f.name).collect(), layouts, known)
                }
                AdtKind::Newtype(inner) => {
                    let inner = self.write_layout(inner, layouts, known);
                    layouts[inner].clone()
                }
                AdtKind::Enum(variants)
                    if variants.iter().all(|v| matches!(v.payload, crate::types::Payload::Unit)) =>
                {
                    let layout = self.layout(ty);
                    let size = match layout.shape {
                        Shape::Enum { tag, .. } => tag.bytes(),
                        _ => layout.size,
                    };
                    let names: Vec<String> = variants.into_iter().map(|v| v.name).collect();
                    format!("e {size} {}", names.join(" "))
                }
                AdtKind::Enum(_) => "-".into(),
            },
            _ => "-".into(),
        };
        id
    }

    /// How what reads a core shows a value, as `Show` does (`cores::shown`):
    /// its number among `shapes`, each a line of `s` and a scalar's type, `x`
    /// for a secret, `u` for nothing, `a` and the size and shape of an
    /// array's elements, `n` and an address's shape for what `none` is zero
    /// of, `m` the shapes of a map's keys and values, or a set's elements, `t`
    /// a tuple's and `r` a record's parts, and `v` the size of a tag and each
    /// variant. A part is `offset:shape`, `*` before the shape when
    /// it is behind a pointer; `-` is what only code can show.
    pub(super) fn shape(&mut self, ty: Ty, shapes: &mut Vec<String>, known: &mut HashMap<Ty, usize>) -> usize {
        if let Some(&id) = known.get(&ty) {
            return id;
        }
        let id = shapes.len();
        shapes.push("-".into());
        known.insert(ty, id);
        let part = |cg: &mut Self, slot: Slot, shapes: &mut Vec<String>, known: &mut HashMap<Ty, usize>| {
            let inner = cg.shape(slot.ty, shapes, known);
            format!("{}:{}{inner}", slot.offset, if slot.boxed { "*" } else { "" })
        };
        let repr = self.types.repr(ty);
        let kind = self.types.kind(repr).clone();
        let written = match kind {
            _ if self.types.secret(ty) => "x".to_string(),
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Byte | TyKind::String => {
                format!("s {}", self.types.display(repr))
            }
            TyKind::Unit => "u".into(),
            TyKind::Tuple(items) => {
                let mut out = String::from("t");
                for index in 0..items.len() {
                    let slot = self.slot(ty, Proj::Field(index as u32));
                    out.push_str(&format!(" {}", part(self, slot, shapes, known)));
                }
                out
            }
            TyKind::Array(element) | TyKind::Deque(element) => {
                let inner = self.shape(element, shapes, known);
                format!("a {} {inner}", self.layout(element).size)
            }
            TyKind::Map(key, entry) => {
                let (key, entry) = (self.shape(key, shapes, known), self.shape(entry, shapes, known));
                format!("m {key} {entry}")
            }
            TyKind::Set(element) => format!("m {}", self.shape(element, shapes, known)),
            TyKind::Adt(adt) if matches!(self.types.adt(adt).kind, AdtKind::Record(_)) => {
                let name = self.types.adt(adt).name.replace(' ', "");
                let AdtKind::Record(fields) = self.types.adt(adt).kind.clone() else { unreachable!("a record") };
                let mut out = format!("r {name}");
                for (index, field) in fields.iter().enumerate() {
                    let slot = self.slot(ty, Proj::Field(index as u32));
                    let written = if field.secret { "x".to_string() } else { part(self, slot, shapes, known) };
                    out.push_str(&format!(" {}={written}", field.name));
                }
                out
            }
            TyKind::Option(_) if matches!(self.layout(ty).shape, Shape::Niche) => {
                let slot = self.slot(ty, Proj::Variant(crate::types::SOME, 0));
                format!("n {}", self.shape(slot.ty, shapes, known))
            }
            TyKind::Option(_) | TyKind::Result(..) | TyKind::Adt(_) => match self.layout(ty).shape {
                Shape::Enum { tag, .. } => {
                    let names: Vec<String> = match self.types.kind(repr).clone() {
                        TyKind::Option(_) => vec!["none".into(), "some".into()],
                        TyKind::Result(..) => vec!["ok".into(), "err".into()],
                        TyKind::Adt(adt) => match self.types.adt(adt).kind.clone() {
                            AdtKind::Enum(variants) => {
                                let name = self.types.adt(adt).name.clone();
                                variants.iter().map(|v| format!("{name}.{}", v.name)).collect()
                            }
                            _ => Vec::new(),
                        },
                        _ => Vec::new(),
                    };
                    let secret = |cg: &Self, variant: usize, index: usize| match cg.types.kind(repr) {
                        TyKind::Adt(adt) => match &cg.types.adt(*adt).kind {
                            AdtKind::Enum(variants) => match &variants[variant].payload {
                                crate::types::Payload::Record(fields) => fields[index].secret,
                                _ => false,
                            },
                            _ => false,
                        },
                        _ => false,
                    };
                    let variants = self.types.variants(ty).unwrap_or_default();
                    let mut out = format!("v {}", tag.bytes());
                    for (variant, components) in variants.iter().enumerate() {
                        let name = names.get(variant).map_or("?", String::as_str).replace(' ', "");
                        let mut parts = Vec::new();
                        for index in 0..components.len() {
                            parts.push(match secret(self, variant, index) {
                                true => "x".to_string(),
                                false => {
                                    let slot = self.slot(ty, Proj::Variant(variant as u32, index as u32));
                                    part(self, slot, shapes, known)
                                }
                            });
                        }
                        out.push_str(&format!(" {name}={}", parts.join(",")));
                    }
                    out
                }
                _ => "-".into(),
            },
            _ => "-".into(),
        };
        shapes[id] = written;
        id
    }

    pub(super) fn test_table(&mut self, tests: &[crate::hir::TestNodeInfo]) -> DataId {
        let names: Vec<DataId> = tests.iter().map(|t| self.string(&t.id)).collect();
        let id = self.module.declare_data("tondo_tests", Linkage::Local, false, false).expect("test table");
        let mut bytes = vec![0u8; tests.len() * 24];
        for (index, test) in tests.iter().enumerate() {
            bytes[index * 24..index * 24 + 8].copy_from_slice(&(test.parent as u64).to_le_bytes());
            bytes[index * 24 + 8..index * 24 + 16].copy_from_slice(&(test.suite as u64).to_le_bytes());
        }
        let mut data = DataDescription::new();
        data.define(bytes.into_boxed_slice());
        data.set_align(8);
        for (index, name) in names.into_iter().enumerate() {
            let name = self.module.declare_data_in_data(name, &mut data);
            data.write_data_addr(index as u32 * 24 + 16, name, 0);
        }
        self.module.define_data(id, &data).expect("test table definition");
        id
    }
}
