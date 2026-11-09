//! Live builds: the slots calls go through, and the tables a reload reads.

use super::*;

/// A live build's slots (toolchain 7.10): the table every call of the
/// program's functions goes through, the slot of each function, and its
/// identity; and what the image registers with the runtime.
pub(super) struct LiveCodegen {
    pub(super) slots: DataId,
    pub(super) info: DataId,
    pub(super) slot_of: Vec<u32>,
    pub(super) identities: Vec<String>,
    /// Each compiled function's code, hashed, by its function.
    pub(super) code: HashMap<FuncId, u64>,
    /// For a patch: the types whose identity the running program holds,
    /// and the number its debugger's sites start from; and the code the
    /// program runs, hashed, by identity, which a patch leaves where it is
    /// unless it changes.
    pub(super) patch: Option<(Vec<String>, u32)>,
    pub(super) known: HashMap<String, u64>,
    /// A patch's conversions, and the release glue of what each converts
    /// from and to; and of those that convert what values with identity
    /// hold, the identity of the old type, and the new one's size and
    /// alignment, release and visit glue.
    pub(super) conversions: Vec<(FuncId, Option<FuncId>, Option<FuncId>)>,
    pub(super) objects: Vec<LiveObject>,
    /// The word of each function's first site, the function, and the
    /// site's number in the build.
    pub(super) sites: Vec<(DataId, usize, usize)>,
    /// The site of each point that has a probe, by function and point.
    pub(super) point_sites: HashMap<(usize, u32), u32>,
    /// What each point's state is in its function, as `point_layout` lays
    /// it out, by function and point.
    pub(super) layouts: HashMap<(usize, u32), StateLayout>,
}

impl LiveCodegen {
    pub(super) fn new(
        module: &mut ObjectModule,
        build: &mut crate::live::Build,
        bodies: &[Body],
        types: &Types,
    ) -> Result<LiveCodegen, String> {
        if build.capacity == 0 {
            build.capacity = crate::live::capacity(bodies.len());
        }
        let mut next = build.slots.values().max().map_or(0, |&slot| slot + 1);
        let mut seen: HashMap<String, u32> = HashMap::new();
        let (mut slot_of, mut identities) = (Vec::new(), Vec::new());
        // A closure is its function's n-th of so many: an edit that adds
        // or removes one makes them all others.
        let mut closures: HashMap<&str, (u32, u32)> = HashMap::new();
        for body in bodies.iter().filter(|b| b.name.starts_with("closure in ")) {
            closures.entry(&body.name).or_default().1 += 1;
        }
        for body in bodies {
            let params: Vec<String> = body
                .params
                .iter()
                .zip(&body.loans)
                .map(|(param, &loan)| {
                    let ty = types.display(body.locals[param.0 as usize]);
                    if loan { format!("mut {ty}") } else { ty }
                })
                .collect();
            let name = match closures.get_mut(body.name.as_str()) {
                Some((ordinal, total)) => {
                    *ordinal += 1;
                    format!("{} {ordinal}/{total}", body.name)
                }
                None => body.name.clone(),
            };
            let mut signature: Vec<Ty> = body.params.iter().map(|p| body.locals[p.0 as usize]).collect();
            signature.push(body.result);
            let digest = crate::live::shape_digest(types, &signature);
            let base = format!("{name}({}): {}", params.join(", "), types.display(body.result));
            let base = if digest.is_empty() { base } else { format!("{base} @{digest}") };
            build.holds.entry(body.name.clone()).or_default().extend(crate::live::held(types, &body.locals, false));
            // Closures share a name: the first of a function keeps it.
            let count = seen.entry(base.clone()).or_insert(0);
            let identity = if *count == 0 { base } else { format!("{base} #{count}") };
            *count += 1;
            let slot = *build.slots.entry(identity.clone()).or_insert_with(|| {
                next += 1;
                next - 1
            });
            if slot >= build.capacity {
                return Err("this live build has no room for more functions: start it again".into());
            }
            slot_of.push(slot);
            identities.push(identity);
        }
        // A patch uses the running program's slots, and says how to set them.
        let (linkage, info) = match build.patch.is_some() {
            true => (Linkage::Import, "tondo_patch"),
            false => (Linkage::Local, "tondo_live"),
        };
        let slots = module.declare_data("tondo_live_slots", linkage, true, false).map_err(|e| e.to_string())?;
        let info = module.declare_data(info, Linkage::Local, false, false).map_err(|e| e.to_string())?;
        let patch = build.patch.as_ref().map(|base| (base.data.clone(), base.sites));
        let known = build.patch.as_ref().map(|base| base.code.clone()).unwrap_or_default();
        Ok(LiveCodegen {
            slots,
            info,
            slot_of,
            identities,
            code: HashMap::new(),
            patch,
            known,
            conversions: Vec::new(),
            objects: Vec::new(),
            sites: Vec::new(),
            point_sites: HashMap::new(),
            layouts: HashMap::new(),
        })
    }
}

/// What a live build's code hashes see of the names it refers to.
pub(super) struct Labels {
    pub(super) functions: HashMap<FuncId, String>,
    pub(super) data: HashMap<DataId, String>,
}

/// A hash of a function's code and of what it refers to.
pub(super) fn code_hash(code: &Compiled, labels: &Labels) -> u64 {
    use cranelift_module::ModuleRelocTarget;
    let mut parts: Vec<Vec<u8>> = vec![code.bytes.clone()];
    for reloc in &code.relocs {
        let target = match &reloc.name {
            ModuleRelocTarget::User { namespace: 0, index } => {
                labels.functions.get(&FuncId::from_u32(*index)).cloned().unwrap_or_default()
            }
            ModuleRelocTarget::User { index, .. } => {
                labels.data.get(&DataId::from_u32(*index)).cloned().unwrap_or_default()
            }
            other => format!("{other:?}"),
        };
        parts.push(format!("{} {:?} {} {target}", reloc.offset, reloc.kind, reloc.addend).into_bytes());
    }
    let parts: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    u64::from_str_radix(&crate::driver::fingerprint(&parts), 16).unwrap_or(0)
}

impl Codegen<'_> {
    /// What a live build's manifest keeps of each function's points, for a
    /// patch to move frames from one version's to another's: a hash of each
    /// of the function's lines, which says what lines stay the same, and of
    /// each point its line among them, what it is, its site and what its
    /// state holds, with each local's name across versions.
    pub(super) fn live_points(&mut self, build: &mut crate::live::Build, bodies: &[Body]) {
        let live = self.live.take().expect("a live build");
        let mut functions = serde_json::Map::new();
        for (index, body) in bodies.iter().enumerate() {
            let placed: Vec<(u32, &crate::mir::Point)> = (0u32..)
                .zip(&body.points)
                .filter(|(number, point)| point.span != Span::default() && live.layouts.contains_key(&(index, *number)))
                .collect();
            let Some(&(_, first)) = placed.first() else { continue };
            let lines: Vec<usize> = placed.iter().map(|(_, p)| self.sources.position(p.span.start).1).collect();
            let (top, bottom) = (*lines.iter().min().expect("a point"), *lines.iter().max().expect("a point"));
            let text = &self.sources.file_of(first.span.start).text;
            let hashes: Vec<u64> =
                text.lines().skip(top - 1).take(bottom - top + 1).map(crate::live::line_hash).collect();
            // A closure's text, which tells which closure it is when the
            // number of its function's changes.
            let shown: Vec<&str> = text.lines().skip(top - 1).take(bottom - top + 1).map(str::trim).collect();
            let shown = body.name.starts_with("closure in ").then(|| shown.join("\n"));
            let mut points = Vec::new();
            for (&(number, point), line) in placed.iter().zip(&lines) {
                let (items, size) = &live.layouts[&(index, number)];
                let mut items: Vec<_> = items.iter().collect();
                items.sort_by_key(|(local, ..)| local.0);
                // Bindings of one name live at once are told apart by order.
                let mut named: HashMap<&str, u32> = HashMap::new();
                let state: Vec<serde_json::Value> = items
                    .into_iter()
                    .filter_map(|(local, kept, offset, bytes)| {
                        let key = match &body.origins[local.0 as usize] {
                            Origin::Named(name) => {
                                let rank = named.entry(name).or_insert(0);
                                *rank += 1;
                                format!("{name}/{}", *rank - 1)
                            }
                            Origin::Made(path, made) => format!("#{}/{made}", crate::live::path_text(path)),
                            Origin::Param(_) => return None,
                        };
                        let ty = body.locals[local.0 as usize];
                        Some(serde_json::json!({
                            "key": key,
                            "type": crate::live::describe(self.types, ty, &mut build.structures),
                            "lent": matches!(kept, StateKept::Pointer(_)),
                            "owned": self.types.needs_drop(ty),
                            "offset": offset,
                            "size": bytes,
                        }))
                    })
                    .collect();
                points.push(serde_json::json!({
                    "point": number,
                    "line": line - top,
                    "kind": point.kind,
                    "path": crate::live::path_text(&point.path),
                    "defers": point.defers,
                    "binds": point.binds,
                    "reads": point.reads,
                    "changes": point.changes,
                    "loops": point.loops,
                    "entry_of": point.entry_of,
                    "runs": point.runs,
                    "site": live.point_sites.get(&(index, number)),
                    "size": size,
                    "state": state,
                }));
            }
            let file = self.sources.position(first.span.start).0;
            // How its frames get their parameters, which a frame that moves
            // to a function of another identity passes converted; and its
            // result, which the old caller takes.
            let params: Vec<serde_json::Value> = body
                .params
                .iter()
                .zip(&body.loans)
                .map(|(param, &loan)| {
                    let ty = body.locals[param.0 as usize];
                    let class = match self.class(ty) {
                        Class::Zero => "zero",
                        Class::Scalar(_) => "scalar",
                        Class::Memory => "memory",
                    };
                    let key = crate::live::describe(self.types, ty, &mut build.structures);
                    let size = self.layout(ty).size;
                    serde_json::json!({ "type": key, "loan": loan, "class": class, "size": size })
                })
                .collect();
            let result = crate::live::describe(self.types, body.result, &mut build.structures);
            // A closure's frame works on its captures, through its first
            // parameter; one that changes them or gives them away works on
            // the closure's own, which a copy cannot stand for.
            let (mutates, consumes) = body
                .params
                .first()
                .and_then(|env| self.types.closure_calls(body.locals[env.0 as usize]))
                .unwrap_or_default();
            let returns = match self.class(body.result) {
                Class::Zero => "zero",
                Class::Scalar(_) => "scalar",
                Class::Memory => "memory",
            };
            // Each call a frame may be in: what the rest of its statement
            // reaches through values with identity, and for a wait, whether
            // its statement can start again.
            let mut calls: Vec<serde_json::Value> = crate::live::touches(self.types, body)
                .into_iter()
                .map(|(span, touches)| {
                    let (_, line, column) = self.sources.position(span.start);
                    let again = body.pauses.iter().find(|(at, _)| *at == span).map(|&(_, again)| again);
                    serde_json::json!({ "line": line, "column": column, "touches": touches, "again": again })
                })
                .collect();
            calls.dedup();
            let entry = serde_json::json!({
                "calls": calls,
                "file": file,
                "top": top,
                "body": [body.name, body.span.start],
                "lines": hashes,
                "points": points,
                "params": params,
                "result": result,
                "mutates": mutates,
                "consumes": consumes,
                "returns": returns,
                "result_size": self.layout(body.result).size,
                "text": shown,
            });
            functions.insert(live.identities[index].clone(), entry);
        }
        build.points = functions;
        self.live = Some(live);
    }

    /// A live build's tables (toolchain 7.10): the slots, each pointing at
    /// its function; the runtime's functions, the functions of C the
    /// program calls and the identities of its types, which a patch reaches
    /// by their place; and what the image registers with the runtime.
    pub(super) fn live_tables(&mut self, build: &mut crate::live::Build, trace: DataId) -> Result<(), String> {
        let live = self.live.take().expect("a live build");
        build.code = live
            .identities
            .iter()
            .enumerate()
            .filter_map(|(index, identity)| live.code.get(&self.functions[index]).map(|&h| (identity.clone(), h)))
            .collect();
        let base = live.patch.as_ref().map_or(0, |(_, sites)| *sites as u64);
        for &(id, _, first) in &live.sites {
            let mut word = DataDescription::new();
            word.define((base + first as u64).to_le_bytes().to_vec().into_boxed_slice());
            word.set_align(8);
            self.module.define_data(id, &word).map_err(|e| e.to_string())?;
        }
        if let Some((known, sites)) = &live.patch {
            return self.patch_table(build, &live, known.len(), *sites, trace);
        }
        let mut slots = DataDescription::new();
        slots.define(vec![0u8; build.capacity as usize * 8].into_boxed_slice());
        slots.set_align(8);
        for (index, &slot) in live.slot_of.iter().enumerate() {
            let function = self.module.declare_func_in_data(self.functions[index], &mut slots);
            slots.write_function_addr(slot * 8, function);
        }
        self.module.define_data(live.slots, &slots).map_err(|e| e.to_string())?;
        let runtime = self.address_table("tondo_live_runtime", &build.runtime)?;
        // What Cranelift calls itself, besides what the program calls.
        let mut c: Vec<String> = self.c_functions.keys().cloned().collect();
        c.extend(["memcpy", "memmove", "memset", "memcmp"].map(String::from));
        c.sort();
        c.dedup();
        build.c = c;
        let c = self.address_table("tondo_live_c", &build.c)?;
        let mut identities: Vec<(String, DataId)> =
            self.identities.iter().map(|(&ty, &id)| (self.identity_key(ty), id)).collect();
        identities.sort();
        build.data = identities.iter().map(|(name, _)| name.clone()).collect();
        // With room for the types patches add.
        let room = identities.len() + 1024;
        let data =
            self.module.declare_data("tondo_live_data", Linkage::Local, true, false).map_err(|e| e.to_string())?;
        let mut table = DataDescription::new();
        table.define(vec![0u8; room * 8].into_boxed_slice());
        table.set_align(8);
        for (index, (_, id)) in identities.iter().enumerate() {
            let identity = self.module.declare_data_in_data(*id, &mut table);
            table.write_data_addr(index as u32 * 8, identity, 0);
        }
        self.module.define_data(data, &table).map_err(|e| e.to_string())?;
        build.sites = self.debug_sites.len() as u32;
        let id = self.string(&build.id);
        let used = live.slot_of.iter().max().map_or(0, |&slot| slot as u64 + 1);
        let words: [(u64, Option<DataId>); 12] = [
            (0, Some(live.slots)),
            (build.capacity as u64, None),
            (used.max(build.slots.values().max().map_or(0, |&s| s as u64 + 1)), None),
            (0, Some(runtime)),
            (build.runtime.len() as u64, None),
            (0, Some(c)),
            (build.c.len() as u64, None),
            (0, Some(data)),
            (build.data.len() as u64, None),
            (room as u64, None),
            (0, Some(id)),
            (build.sites as u64, None),
        ];
        let mut info = DataDescription::new();
        info.define(words.iter().flat_map(|(value, _)| value.to_le_bytes()).collect::<Vec<u8>>().into_boxed_slice());
        info.set_align(8);
        for (index, (_, target)) in words.iter().enumerate() {
            if let Some(target) = target {
                let target = self.module.declare_data_in_data(*target, &mut info);
                info.write_data_addr(index as u32 * 8, target, 0);
            }
        }
        self.module.define_data(live.info, &info).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// A patch's table, which the runtime reads once it maps the patch:
    /// its tables of stack traces and of the debugger's probes, the site
    /// its probes number from, then the slots it sets and the identities
    /// of the types it adds.
    pub(super) fn patch_table(
        &mut self,
        build: &mut crate::live::Build,
        live: &LiveCodegen,
        known: usize,
        sites: u32,
        trace: DataId,
    ) -> Result<(), String> {
        let mut added: Vec<(String, DataId)> = self
            .identities
            .iter()
            .filter(|(_, id)| self.module.declarations().get_data_decl(**id).linkage != Linkage::Import)
            .map(|(&ty, &id)| (self.identity_key(ty), id))
            .collect();
        added.sort();
        // The functions whose code changes or is new: the others go on
        // running the code the program has, whose frames a later patch
        // moves as it knows their points.
        let set: Vec<usize> = (0..live.slot_of.len())
            .filter(|&index| live.known.get(&live.identities[index]) != live.code.get(&self.functions[index]))
            .collect();
        let start = 40 + set.len() * 16;
        // Then the conversions: their count, and each one's function and
        // the release glue of what it converts from and to.
        let conversions = start + added.len() * 16;
        // Then what values with identity hold that converts: the count, and
        // each one's conversion, old identity, new size and alignment, and
        // release and visit glue.
        let objects = conversions + 8 + live.conversions.len() * 24;
        let mut bytes = vec![0u8; objects + 8 + live.objects.len() * 56];
        bytes[conversions..conversions + 8].copy_from_slice(&(live.conversions.len() as u64).to_le_bytes());
        bytes[objects..objects + 8].copy_from_slice(&(live.objects.len() as u64).to_le_bytes());
        for (index, &(conversion, _, _, size, align, _, _)) in live.objects.iter().enumerate() {
            let at = objects + 8 + index * 56;
            bytes[at..at + 8].copy_from_slice(&(conversion as u64).to_le_bytes());
            bytes[at + 24..at + 32].copy_from_slice(&(size as u64).to_le_bytes());
            bytes[at + 32..at + 40].copy_from_slice(&(align as u64).to_le_bytes());
        }
        bytes[16..24].copy_from_slice(&(sites as u64).to_le_bytes());
        bytes[24..32].copy_from_slice(&(set.len() as u64).to_le_bytes());
        bytes[32..40].copy_from_slice(&(added.len() as u64).to_le_bytes());
        for (at, &index) in set.iter().enumerate() {
            bytes[40 + at * 16..48 + at * 16].copy_from_slice(&(live.slot_of[index] as u64).to_le_bytes());
        }
        for index in 0..added.len() {
            let at = start + index * 16;
            bytes[at..at + 8].copy_from_slice(&((known + index) as u64).to_le_bytes());
        }
        let mut table = DataDescription::new();
        table.define(bytes.into_boxed_slice());
        table.set_align(8);
        let trace = self.module.declare_data_in_data(trace, &mut table);
        table.write_data_addr(0, trace, 0);
        if let Some(debug) = self.debug {
            let debug = self.module.declare_data_in_data(debug, &mut table);
            table.write_data_addr(8, debug, 0);
        }
        for (at, &index) in set.iter().enumerate() {
            let function = self.module.declare_func_in_data(self.functions[index], &mut table);
            table.write_function_addr((48 + at * 16) as u32, function);
        }
        for (index, (name, id)) in added.iter().enumerate() {
            let identity = self.module.declare_data_in_data(*id, &mut table);
            table.write_data_addr((start + index * 16 + 8) as u32, identity, 0);
            build.data.push(name.clone());
        }
        for (index, &(convert, from, to)) in live.conversions.iter().enumerate() {
            let at = conversions + 8 + index * 24;
            for (offset, function) in [(0, Some(convert)), (8, from), (16, to)] {
                if let Some(function) = function {
                    let function = self.module.declare_func_in_data(function, &mut table);
                    table.write_function_addr((at + offset) as u32, function);
                }
            }
        }
        for (index, &(_, from, to, _, _, release, visit)) in live.objects.iter().enumerate() {
            let at = objects + 8 + index * 56;
            for (offset, identity) in [(8, from), (16, to)] {
                let identity = self.module.declare_data_in_data(identity, &mut table);
                table.write_data_addr((at + offset) as u32, identity, 0);
            }
            for (offset, function) in [(40, release), (48, visit)] {
                if let Some(function) = function {
                    let function = self.module.declare_func_in_data(function, &mut table);
                    table.write_function_addr((at + offset) as u32, function);
                }
            }
        }
        self.module.define_data(live.info, &table).map_err(|e| e.to_string())?;
        build.sites = sites + self.debug_sites.len() as u32;
        Ok(())
    }

    /// A table of the addresses of functions by name: those the program
    /// declares, or others as imports.
    pub(super) fn address_table(&mut self, name: &str, functions: &[String]) -> Result<DataId, String> {
        use cranelift_module::FuncOrDataId;
        let id = self.module.declare_data(name, Linkage::Local, false, false).map_err(|e| e.to_string())?;
        let mut table = DataDescription::new();
        table.define(vec![0u8; functions.len().max(1) * 8].into_boxed_slice());
        table.set_align(8);
        for (index, function) in functions.iter().enumerate() {
            let offset = index as u32 * 8;
            match self.module.get_name(function) {
                Some(FuncOrDataId::Data(data)) => {
                    let data = self.module.declare_data_in_data(data, &mut table);
                    table.write_data_addr(offset, data, 0);
                }
                found => {
                    let func = match found {
                        Some(FuncOrDataId::Func(func)) => func,
                        _ => {
                            let signature = self.module.make_signature();
                            self.module
                                .declare_function(function, Linkage::Import, &signature)
                                .map_err(|e| e.to_string())?
                        }
                    };
                    let func = self.module.declare_func_in_data(func, &mut table);
                    table.write_function_addr(offset, func);
                }
            }
        }
        self.module.define_data(id, &table).map_err(|e| e.to_string())?;
        Ok(id)
    }
}
