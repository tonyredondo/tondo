//! The prelude, imports, module names, types, generics, kinds and defaults.

use super::*;

impl<'a> Checker<'a> {
    pub(super) fn declare_prelude(&mut self) {
        // `trait Display { fn display(self): String }` (19.6).
        let self_param = self.types.declare_param("Self".into());
        let signature = Signature {
            params: vec![self_param],
            names: vec!["self".into()],
            receiver: Some(ast::Mode::Ref),
            modes: vec![ast::Mode::Ref],
            variadic: false,
            result: Ty::STRING,
            generics: vec![self_param],
            name: "Display.display".into(),
            defaults: Vec::new(),
            is_unsafe: false,
            foreign: false,
        };
        let display = TraitMethod { name: "display".into(), signature, default: None, span: Span::default() };
        self.traits.push(TraitDef {
            name: "Display".into(),
            self_param,
            params: Vec::new(),
            methods: vec![display],
            requires: Vec::new(),
        });
        self.trait_names.insert("Display".into(), DISPLAY);
        self.bounds.insert(self_param, vec![(DISPLAY, Vec::new())]);
        // `trait Iterator[T] { fn next(mut self): T? }` (10.17).
        let self_param = self.types.declare_param("Self".into());
        let element = self.types.declare_param("T".into());
        let signature = Signature {
            params: vec![self_param],
            names: vec!["self".into()],
            receiver: Some(ast::Mode::Mut),
            modes: vec![ast::Mode::Mut],
            variadic: false,
            result: self.types.intern(TyKind::Option(element)),
            generics: vec![self_param, element],
            name: "Iterator.next".into(),
            defaults: Vec::new(),
            is_unsafe: false,
            foreign: false,
        };
        let next = TraitMethod { name: "next".into(), signature, default: None, span: Span::default() };
        self.traits.push(TraitDef {
            name: "Iterator".into(),
            self_param,
            params: vec![element],
            methods: vec![next],
            requires: Vec::new(),
        });
        self.trait_names.insert("Iterator".into(), ITERATOR);
        self.bounds.insert(self_param, vec![(ITERATOR, vec![element])]);
        // The cursors of maps and sets: a collection and a position.
        for (kind, set) in [
            (CursorKind::Entries, false),
            (CursorKind::Keys, false),
            (CursorKind::Values, false),
            (CursorKind::Values, true),
        ] {
            let key = self.types.declare_param("K".into());
            self.types.param_mut(key).key = true;
            let value = self.types.declare_param("V".into());
            let (params, collection) = if set {
                (vec![key], self.types.intern(TyKind::Set(key)))
            } else {
                (vec![key, value], self.map_type(key, value))
            };
            let name = match (kind, set) {
                (_, true) => "SetValues",
                (CursorKind::Entries, _) => "MapEntries",
                (CursorKind::Keys, _) => "MapKeys",
                (CursorKind::Values, _) => "MapValues",
            };
            let template = self.types.declare_template(name.into(), params.clone());
            // Private to every module: a cursor's representation is no part
            // of what a program means (language 8.13).
            let fields = vec![
                Field { name: "collection".into(), ty: collection, private: true, secret: false },
                Field { name: "position".into(), ty: Ty::INT, private: true, secret: false },
            ];
            self.owners.insert(
                name.into(),
                Owner { module: u32::MAX, public: true, private_fields: vec![true, true], unreflected: Vec::new() },
            );
            self.types.define_template(template, AdtKind::Record(fields));
            self.types.declare_cursor(template, kind);
            let element = match (kind, set) {
                (CursorKind::Entries, false) => self.types.intern(TyKind::Tuple(vec![key, value])),
                (CursorKind::Values, false) => value,
                _ => key,
            };
            let target = self.types.instantiate(template, params.clone());
            self.impls.push(ImplDef {
                intrinsic: true,
                span: Span::default(),
                trait_id: ITERATOR,
                trait_args: vec![element],
                target,
                generics: params,
                methods: vec![None],
            });
            self.cursor_templates.insert((kind, set), template);
        }
        let (id, ty) = self.types.declare_adt("NumericConversionError".into());
        let variants = ["OutOfRange", "NotFinite", "NotIntegral"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.type_names.insert("NumericConversionError".into(), TypeName::Nominal(ty));
        self.conversion_error = ty;
        let (id, ty) = self.types.declare_adt("collections.CollectionError".into());
        let variants = ["InvalidCapacity", "InvalidIndex", "InvalidStep", "ResourceLimit"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.types.shown.insert(ty, Shown::Words);
        self.collection_error = ty;
        let (id, ty) = self.types.declare_adt("text.TextError".into());
        let variants = ["InvalidIndex", "InvalidBoundary", "ResourceLimit", "InvalidData"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.types.shown.insert(ty, Shown::Words);
        self.text_error = ty;
        let (id, ty) = self.types.declare_adt("io.IoError".into());
        let variants = ["Closed", "InvalidData", "Host"]
            .map(|name| Variant { name: name.into(), payload: Payload::Unit })
            .to_vec();
        self.types.define_adt(id, AdtKind::Enum(variants));
        self.types.shown.insert(ty, Shown::Words);
        self.io_error = ty;
        for handle in [Handle::Writer, Handle::Reader, Handle::Buffer] {
            self.types.handle(handle, Ty::UNIT);
        }
        for ty in [Ty::STRING, self.types.intern(TyKind::Array(Ty::BYTE))] {
            self.types.intern(TyKind::Option(ty));
        }
        // What `std.ffi` exposes through `std.internal.memory`: C's `void`,
        // which only a pointer refers to, and C strings.
        let (id, void) = self.types.declare_adt("ffi.Void".into());
        self.types.foreign.insert(id, crate::types::CLayout { opaque: true, ..Default::default() });
        self.void = void;
        let (id, c_string) = self.types.declare_adt("ffi.CString".into());
        let bytes = self.types.intern(TyKind::Pointer(Ty::BYTE));
        self.types.define_adt(id, AdtKind::Newtype(bytes));
        self.c_string = c_string;
        for ty in [c_string, bytes, self.types.intern(TyKind::Pointer(void))] {
            self.types.intern(TyKind::Option(ty));
        }
    }

    /// The type a numeric constructor such as `Int8(value)` names (18.6).
    pub(super) fn numeric_type(name: &str) -> Option<Ty> {
        if let Some(int) = IntTy::from_name(name) {
            return Some(Ty::int(int));
        }
        Some(match name {
            "Float" | "Float64" => Ty::F64,
            "Float32" => Ty::F32,
            "Byte" => Ty::BYTE,
            _ => return None,
        })
    }

    /// Whether converting `from` to `to` can fail, or `None` when the table
    /// of 18.6 has no such conversion.
    pub(super) fn conversion_checked(&self, from: Ty, to: Ty) -> Option<bool> {
        self.types.numeric_conversion(from, to)
    }

    pub(super) fn import(&mut self, import: &ast::Import) {
        let path: Vec<&str> = import.path.iter().map(|i| i.name.as_str()).collect();
        let module = match path.as_slice() {
            ["std", "console"] => StdModule::Console,
            ["std", "collections"] => StdModule::Collections,
            ["std", "text"] => StdModule::Text,
            ["std", "time"] => StdModule::Time,
            ["std", "sync"] => StdModule::Sync,
            ["std", "simd"] => StdModule::Simd,
            ["std", "fs"] => StdModule::Fs,
            ["std", "path"] => StdModule::Path,
            ["std", "io"] => StdModule::Io,
            ["std", "env"] => StdModule::Env,
            ["std", "process"] => StdModule::Process,
            ["std", "internal", "json"] => StdModule::InternalJson,
            ["std", "internal", "memory"] => StdModule::InternalMemory,
            ["std", "testing"] => StdModule::Testing,
            ["std", "internal", "regex"] => StdModule::InternalRegex,
            ["std", "internal", "reflect"] => StdModule::InternalReflect,
            ["std", "internal", "text"] => StdModule::InternalText,
            ["std", "internal", "math"] => StdModule::InternalMath,
            ["std", "internal", "cpu"] => StdModule::InternalCpu,
            ["std", "internal", "library"] => StdModule::InternalLibrary,
            ["std", "internal", "meta"] => StdModule::InternalMeta,
            ["std", "internal", "codec"] => StdModule::InternalCodec,
            ["std", "internal", "http"] => StdModule::InternalHttp,
            ["std", "internal", "toml"] => StdModule::InternalToml,
            ["std", "internal", "yaml"] => StdModule::InternalYaml,
            ["std", "internal", "msgpack"] => StdModule::InternalMsgpack,
            ["std", "internal", "protobuf"] => StdModule::InternalProtobuf,
            ["std", "internal", "grpc"] => StdModule::InternalGrpc,
            ["std", "internal", "cbor"] => StdModule::InternalCbor,
            ["std", "internal", "net"] => StdModule::InternalNet,
            ["std", "internal", "fuzz"] => StdModule::InternalFuzz,
            ["std", "internal", "model"] => StdModule::InternalModel,
            ["std", "internal", "panic"] => StdModule::InternalPanic,
            ["std", "internal", "time"] => StdModule::InternalTime,
            ["std", "internal", "fs"] => StdModule::InternalFs,
            ["std", "internal", "env"] => StdModule::InternalEnv,
            ["std", "internal", "process"] => StdModule::InternalProcess,
            ["std", "internal", "sql"] => StdModule::InternalSql,
            ["std", "internal", "image"] => StdModule::InternalImage,
            ["std", "internal", "context"] => StdModule::InternalContext,
            ["std", "internal", "channel"] => StdModule::InternalChannel,
            ["std", rest @ ..] if let Some((targets, suffix, key)) = crate::stdlib::elsewhere(&path) => {
                let message = format!(
                    "`std.{}` is of {targets} only: import it from a file of {targets}, as `name_{suffix}.to`, or limit the package with `{key} = [\"{suffix}\"]` in tondo.toml",
                    rest.join(".")
                );
                self.diags.error("E1008", import.span, message);
                return;
            }
            ["std", rest @ ..] => {
                let name = rest.join(".");
                let modules = crate::api::standard_modules();
                let names = modules.iter().filter_map(|m| m.strip_prefix("std."));
                let message = match closest(&name, names) {
                    Some(near) => format!("there is no module `std.{name}`; did you mean `std.{near}`?"),
                    None => format!("there is no module `std.{name}`; `tondo api --list` names the standard modules"),
                };
                self.diags.error("E1008", import.span, message);
                let near = closest(&name, modules.iter().filter_map(|m| m.strip_prefix("std.")));
                if let (Some(near), Some(first), Some(last)) = (near, import.path.first(), import.path.last()) {
                    let edit = (first.span.to(last.span), format!("std.{near}"));
                    self.diags.fix(format!("import `std.{near}`"), false, vec![edit]);
                }
                return;
            }
            _ => {
                let message = format!("module `{}` is not available", path.join("."));
                self.diags.error("E1008", import.span, message);
                return;
            }
        };
        if matches!(
            module,
            StdModule::Fs
                | StdModule::Process
                | StdModule::Env
                | StdModule::Time
                | StdModule::Sync
                | StdModule::Testing
                | StdModule::InternalMemory
        ) && self.meta_denied(import.span, &format!("`{}`", path.join(".")))
        {
            return;
        }
        let name = import.alias.as_ref().unwrap_or(import.path.last().unwrap());
        self.declare_module_name(name, ModuleName::Module(module));
        if module == StdModule::Sync && self.compare_exchange.is_none() {
            let param = self.types.declare_param("T".into());
            self.types.param_mut(param).copy = true;
            let template = self.types.declare_template(format!("{}.CompareExchange", name.name), vec![param]);
            let variant = |name: &str| Variant { name: name.into(), payload: Payload::Tuple(vec![param]) };
            self.types.define_template(template, AdtKind::Enum(vec![variant("Exchanged"), variant("Mismatch")]));
            let generic = self.types.instantiate(template, vec![param]);
            self.compare_exchange = Some((template, generic));
            for handle in [Handle::Semaphore, Handle::Permit, Handle::Barrier] {
                let ty = self.types.handle(handle, Ty::UNIT);
                self.types.intern(TyKind::Option(ty));
            }
        }
        if module == StdModule::Fs && self.fs.is_none() {
            let unit = |name: &str| Variant { name: name.into(), payload: Payload::Unit };
            let declare_enum = |this: &mut Self, ty: &str, variants: &[&str]| {
                let (id, ty) = this.types.declare_adt(format!("{}.{ty}", name.name));
                this.types.define_adt(id, AdtKind::Enum(variants.iter().map(|v| unit(v)).collect()));
                ty
            };
            let errors = [
                "NotFound",
                "PermissionDenied",
                "AlreadyExists",
                "NotADirectory",
                "IsADirectory",
                "InvalidData",
                "InvalidName",
                "Host",
            ];
            // `Host` holds the system's number of the error.
            let (id, error) = self.types.declare_adt(format!("{}.FsError", name.name));
            let variants = errors
                .iter()
                .map(|&v| match v {
                    "Host" => Variant { name: v.into(), payload: Payload::Tuple(vec![Ty::INT]) },
                    _ => unit(v),
                })
                .collect();
            self.types.define_adt(id, AdtKind::Enum(variants));
            self.types.shown.insert(error, Shown::Words);
            let kind = declare_enum(self, "FileKind", &["File", "Directory", "Symlink", "Other"]);
            let mode = declare_enum(self, "OpenMode", &["Read", "Write", "Append", "CreateNew", "ReadWrite"]);
            let file = self.types.handle(Handle::File, Ty::UNIT);
            for ty in [Ty::STRING, self.types.intern(TyKind::Array(Ty::BYTE))] {
                self.types.intern(TyKind::Option(ty));
            }
            self.fs = Some(FsTypes { error, kind, mode, file });
        }
        if module == StdModule::InternalJson {
            self.types.handle(Handle::JsonReader, Ty::UNIT);
            self.types.handle(Handle::JsonWriter, Ty::UNIT);
        }
        if module == StdModule::InternalRegex {
            self.types.handle(Handle::Regex, Ty::UNIT);
            self.types.intern(TyKind::Array(Ty::INT));
        }
        if module == StdModule::InternalReflect {
            self.types.handle(Handle::ReflectBox, Ty::UNIT);
        }
        if matches!(module, StdModule::InternalText | StdModule::InternalCodec | StdModule::InternalHttp) {
            self.types.intern(TyKind::Array(Ty::STRING));
        }
        if matches!(
            module,
            StdModule::InternalHttp
                | StdModule::InternalCodec
                | StdModule::InternalToml
                | StdModule::InternalYaml
                | StdModule::InternalMsgpack
                | StdModule::InternalProtobuf
                | StdModule::InternalGrpc
                | StdModule::InternalCbor
                | StdModule::InternalNet
                | StdModule::InternalFs
                | StdModule::InternalSql
                | StdModule::InternalImage
                | StdModule::InternalLibrary
        ) {
            self.types.handle(Handle::Native, Ty::UNIT);
        }
        if module == StdModule::InternalFs {
            self.types.handle(Handle::Owned, Ty::UNIT);
        }
        if matches!(module, StdModule::InternalFs | StdModule::InternalTime | StdModule::InternalImage) {
            self.types.intern(TyKind::Array(Ty::INT));
            self.types.intern(TyKind::Array(Ty::STRING));
        }
        if module == StdModule::Process && self.process.is_none() {
            let prefix = &name.name;
            let bytes = self.types.intern(TyKind::Array(Ty::BYTE));
            let writer = self.types.handle(Handle::Writer, Ty::UNIT);
            let reader = self.types.handle(Handle::Reader, Ty::UNIT);
            let variant = |name: &str, payload: Option<Ty>| Variant {
                name: name.into(),
                payload: payload.map_or(Payload::Unit, |ty| Payload::Tuple(vec![ty])),
            };
            let s = Ty::STRING;
            let (id, input) = self.types.declare_adt(format!("{prefix}.Input"));
            let variants = vec![
                variant("Inherit", None),
                variant("Null", None),
                variant("Text", Some(s)),
                variant("Bytes", Some(bytes)),
                variant("File", Some(s)),
                variant("Reader", Some(reader)),
                variant("Pipe", None),
            ];
            self.types.define_adt(id, AdtKind::Enum(variants));
            let (id, output) = self.types.declare_adt(format!("{prefix}.Output"));
            let variants = vec![
                variant("Inherit", None),
                variant("Null", None),
                variant("Capture", None),
                variant("File", Some(s)),
                variant("Append", Some(s)),
                variant("Writer", Some(writer)),
                variant("Stdout", None),
                variant("Pipe", None),
            ];
            self.types.define_adt(id, AdtKind::Enum(variants));
            let (id, status) = self.types.declare_adt(format!("{prefix}.ExitStatus"));
            let fields = vec![
                Field { name: "code".into(), ty: Ty::INT, private: false, secret: false },
                Field { name: "success".into(), ty: Ty::BOOL, private: false, secret: false },
            ];
            self.types.define_adt(id, AdtKind::Record(fields));
            let statuses = self.types.intern(TyKind::Array(status));
            let (id, result) = self.types.declare_adt(format!("{prefix}.ProcessOutput"));
            let fields = vec![
                Field { name: "stdout".into(), ty: bytes, private: false, secret: false },
                Field { name: "stderr".into(), ty: bytes, private: false, secret: false },
                Field { name: "combined".into(), ty: bytes, private: false, secret: false },
                Field { name: "statuses".into(), ty: statuses, private: false, secret: false },
            ];
            self.types.define_adt(id, AdtKind::Record(fields));
            let (id, error) = self.types.declare_adt(format!("{prefix}.ProcessError"));
            let variants = vec![
                variant("NotFound", None),
                variant("PermissionDenied", None),
                variant("InvalidData", None),
                Variant { name: "Failed".into(), payload: Payload::Tuple(vec![status, result]) },
                variant("Host", None),
                Variant { name: "InvalidCommand".into(), payload: Payload::Tuple(vec![Ty::STRING]) },
            ];
            self.types.define_adt(id, AdtKind::Enum(variants));
            self.types.shown.insert(error, Shown::ProcessError);
            let (id, signal) = self.types.declare_adt(format!("{prefix}.Signal"));
            let variants =
                ["Interrupt", "Terminate", "Hangup", "User1", "User2", "Quit", "Kill"].map(|name| variant(name, None));
            self.types.define_adt(id, AdtKind::Enum(variants.to_vec()));
            let process = self.types.handle(Handle::Process, Ty::UNIT);
            for handle in [Handle::Command, Handle::Pipeline] {
                self.types.handle(handle, Ty::UNIT);
            }
            self.process = Some(ProcessTypes { input, output, status, result, error, process, signal });
        }
        if module == StdModule::Env && self.env_error.is_none() {
            let (id, ty) = self.types.declare_adt(format!("{}.EnvError", name.name));
            let variants = ["NotFound", "NotADirectory", "PermissionDenied", "Host"]
                .map(|name| Variant { name: name.into(), payload: Payload::Unit })
                .to_vec();
            self.types.define_adt(id, AdtKind::Enum(variants));
            self.types.shown.insert(ty, Shown::Words);
            self.env_error = Some(ty);
            self.types.intern(TyKind::Map(Ty::STRING, Ty::STRING));
            self.types.intern(TyKind::Array(Ty::STRING));
        }
        if module == StdModule::Time && self.duration.is_none() {
            let (id, ty) = self.types.declare_adt(format!("{}.Duration", name.name));
            self.types.define_adt(id, AdtKind::Newtype(Ty::INT));
            self.types.shown.insert(ty, Shown::Duration);
            self.duration = Some(ty);
            let (id, ty) = self.types.declare_adt(format!("{}.Instant", name.name));
            self.types.define_adt(id, AdtKind::Newtype(Ty::INT));
            self.instant = Some(ty);
            let (id, ty) = self.types.declare_adt(format!("{}.DateTime", name.name));
            let fields = ["year", "month", "day", "hour", "minute", "second", "nanosecond"]
                .map(|name| Field { name: name.into(), ty: Ty::INT, private: false, secret: false })
                .to_vec();
            self.types.define_adt(id, AdtKind::Record(fields));
            self.types.shown.insert(ty, Shown::DateTime);
            self.datetime = Some(ty);
            let (id, ty) = self.types.declare_adt(format!("{}.TimeError", name.name));
            let variants = ["InvalidDateTime", "InvalidDuration", "UnknownZone"]
                .map(|name| Variant { name: name.into(), payload: Payload::Unit })
                .to_vec();
            self.types.define_adt(id, AdtKind::Enum(variants));
            self.types.shown.insert(ty, Shown::Words);
            self.time_error = Some(ty);
        }
    }

    /// A type of an imported standard module, such as `text.TextError`.
    pub(super) fn module_type(&mut self, module: &str, name: &str) -> Option<Ty> {
        if let Some(&ModuleName::Module(hosted)) = self.names.get(module)
            && let Some(ty) = Self::companions(hosted).iter().find_map(|companion| {
                match self.type_names.get(&format!("{companion}.{name}")) {
                    Some(&TypeName::Nominal(ty)) if self.owner(ty).is_some_and(|owner| owner.public) => Some(ty),
                    _ => None,
                }
            })
        {
            return Some(ty);
        }
        match (self.names.get(module), name) {
            (Some(ModuleName::Module(StdModule::Env)), "EnvError") => self.env_error,
            (Some(ModuleName::Module(StdModule::InternalJson)), "NativeReader") => {
                Some(self.types.intern(TyKind::Handle(Handle::JsonReader, Ty::UNIT)))
            }
            (Some(ModuleName::Module(StdModule::InternalJson)), "NativeWriter") => {
                Some(self.types.intern(TyKind::Handle(Handle::JsonWriter, Ty::UNIT)))
            }
            (Some(ModuleName::Module(StdModule::InternalRegex)), "NativeRegex") => {
                Some(self.types.intern(TyKind::Handle(Handle::Regex, Ty::UNIT)))
            }
            (
                Some(ModuleName::Module(
                    StdModule::InternalHttp
                    | StdModule::InternalCodec
                    | StdModule::InternalToml
                    | StdModule::InternalYaml
                    | StdModule::InternalMsgpack
                    | StdModule::InternalProtobuf
                    | StdModule::InternalGrpc
                    | StdModule::InternalCbor
                    | StdModule::InternalNet
                    | StdModule::InternalFs
                    | StdModule::InternalSql
                    | StdModule::InternalImage
                    | StdModule::InternalLibrary
                    | StdModule::InternalText
                    | StdModule::InternalContext,
                )),
                "Native",
            ) => Some(self.types.intern(TyKind::Handle(Handle::Native, Ty::UNIT))),
            (Some(ModuleName::Module(StdModule::InternalFs)), "Owned") => {
                Some(self.types.intern(TyKind::Handle(Handle::Owned, Ty::UNIT)))
            }
            (Some(ModuleName::Module(StdModule::InternalReflect)), "Box") => {
                Some(self.types.intern(TyKind::Handle(Handle::ReflectBox, Ty::UNIT)))
            }
            (Some(ModuleName::Module(StdModule::InternalMemory)), "CString") => Some(self.c_string),
            (Some(ModuleName::Module(StdModule::InternalMemory)), "Void") => Some(self.void),
            (Some(ModuleName::Module(StdModule::Process)), name) => {
                let types = self.process?;
                match name {
                    "Input" => Some(types.input),
                    "Output" => Some(types.output),
                    "ExitStatus" => Some(types.status),
                    "ProcessOutput" => Some(types.result),
                    "ProcessError" => Some(types.error),
                    "Process" => Some(types.process),
                    "Signal" => Some(types.signal),
                    _ => None,
                }
            }
            (Some(ModuleName::Module(StdModule::Io)), name) => match name {
                "IoError" => Some(self.io_error),
                "Writer" => Some(self.types.intern(TyKind::Handle(Handle::Writer, Ty::UNIT))),
                "Reader" => Some(self.types.intern(TyKind::Handle(Handle::Reader, Ty::UNIT))),
                "Buffer" => Some(self.types.intern(TyKind::Handle(Handle::Buffer, Ty::UNIT))),
                _ => None,
            },
            (Some(ModuleName::Module(StdModule::Collections)), "CollectionError") => Some(self.collection_error),
            (Some(ModuleName::Module(StdModule::Text)), "TextError") => Some(self.text_error),
            (Some(ModuleName::Module(StdModule::Time)), "Duration") => self.duration,
            (Some(ModuleName::Module(StdModule::Time)), "TimeError") => self.time_error,
            (Some(ModuleName::Module(StdModule::Time)), "Instant") => self.instant,
            (Some(ModuleName::Module(StdModule::Time)), "DateTime") => self.datetime,
            (Some(ModuleName::Module(StdModule::Sync)), "CompareExchange") => self.compare_exchange.map(|(_, g)| g),
            (Some(ModuleName::Module(StdModule::Fs)), name) => {
                let fs = self.fs?;
                match name {
                    "FsError" => Some(fs.error),
                    "FileKind" => Some(fs.kind),
                    "OpenMode" => Some(fs.mode),
                    "File" => Some(fs.file),
                    _ => None,
                }
            }
            (Some(ModuleName::Module(StdModule::Sync)), name) => {
                let handle = match name {
                    "Semaphore" => Handle::Semaphore,
                    "Permit" => Handle::Permit,
                    "Barrier" => Handle::Barrier,
                    _ => return None,
                };
                Some(self.types.intern(TyKind::Handle(handle, Ty::UNIT)))
            }
            _ => None,
        }
    }

    pub(super) fn declare_module_name(&mut self, name: &ast::Ident, value: ModuleName) {
        if matches!(value, ModuleName::Fn(_)) && CONTEXTUAL_CONSTRUCTORS.contains(&name.name.as_str()) {
            let message = format!(
                "a function cannot be named `{}`: `{}(...)` always constructs a `Result` or `Option`",
                name.name, name.name
            );
            self.diags.error("E1005", name.span, message);
        } else if INTRINSICS.contains(&name.name.as_str()) {
            let message = format!("`{}` is a reserved intrinsic name", name.name);
            self.diags.error("E1005", name.span, message);
        } else if self.names.insert(name.name.clone(), value).is_some() {
            let message = format!("`{}` is declared more than once", name.name);
            self.diags.error("E1002", name.span, message);
            self.first_declared(name);
        } else {
            self.declared_at.insert(name.name.clone(), name.span);
        }
    }

    /// Where the binding `name` visible in `cx`, or in what encloses it, is
    /// declared.
    pub(super) fn binding_span(&self, cx: &FnCx, name: &str) -> Option<Span> {
        match self.lookup(cx, name) {
            Some(local) => Some(cx.locals[local.0 as usize].span),
            None => self.binding_span(cx.parent.as_deref()?, name),
        }
    }

    /// Points the error reported last to where `name` was first declared.
    pub(super) fn first_declared(&mut self, name: &ast::Ident) {
        if let Some(&first) = self.declared_at.get(&name.name) {
            self.diags.related(first, format!("`{}` is first declared here", name.name));
        }
    }

    pub(super) fn declare_type_name(&mut self, name: &ast::Ident, value: TypeName) {
        if PRELUDE_TYPES.contains(&name.name.as_str()) {
            let message = format!(
                "`{}` is a type of the prelude, which every module sees, and cannot be redeclared: give this type another name",
                name.name
            );
            self.diags.error("E1005", name.span, message);
            // The rest of the program checks against it, rather than
            // failing at each use.
            self.misnamed.insert(name.name.clone());
            self.type_names.insert(name.name.clone(), value);
        } else if self.type_names.insert(name.name.clone(), value).is_some() {
            let message = format!("type `{}` is declared more than once", name.name);
            self.diags.error("E1002", name.span, message);
            self.first_declared(name);
        } else if self.trait_names.contains_key(&name.name) && self.declared_at.contains_key(&name.name) {
            // A trait declared by this name, not one of the prelude's,
            // which every module sees by its own.
            let message = format!("`{}` is already declared", name.name);
            self.diags.error("E1002", name.span, message);
            self.first_declared(name);
        } else {
            self.declared_at.insert(name.name.clone(), name.span);
        }
    }

    /// Declares every nominal type and alias, then resolves their definitions,
    /// so declarations may refer to each other in any order (7.11).
    /// The name a type shows: its own for an intrinsic type the prelude
    /// declares, such as `Range`.
    /// The module `module` is, as the types and the code generator number
    /// it from its path.
    pub(super) fn declaring_module(&mut self, module: u32) -> crate::types::ModuleId {
        let path = self.module_paths.get(module as usize).map(|path| path.join(".")).unwrap_or_default();
        self.types.module(&path)
    }

    pub(super) fn shown_type_name(&self, name: &str, module: u32) -> String {
        match name.strip_prefix("prelude.") {
            Some(bare) if PRELUDE_TYPES.contains(&bare) && Some(module) == self.prelude => bare.to_string(),
            // `Panic`, which the prelude names from `std.panics`, and
            // `time.TimedOut`, which `std.time` writes in Tondo.
            _ if self.module_paths.get(module as usize).is_some_and(|path| path == &["std", "panics"]) => {
                name.strip_prefix("panics.").unwrap_or(name).to_string()
            }
            _ if let Some(path) = self.module_paths.get(module as usize)
                && let [std, companion] = path.as_slice()
                && std == "std"
                && let Some(completed) = crate::stdlib::completed(companion) =>
            {
                format!("{completed}.{}", name.strip_prefix(&format!("{companion}.")).unwrap_or(name))
            }
            _ => name.to_string(),
        }
    }

    pub(super) fn declare_types(&mut self, items: &'a [ast::Item]) {
        let mut declared = Vec::new();
        let mut templates = Vec::new();
        for item in items {
            self.index_type(item);
            let owner = match item {
                ast::Item::Type(decl) => Some((
                    decl.module,
                    decl.public,
                    match &decl.body {
                        ast::TypeBody::Record(fields) => fields.iter().map(|f| f.private).collect(),
                        ast::TypeBody::Newtype(_) | ast::TypeBody::Opaque => Vec::new(),
                    },
                    decl.unreflected.clone(),
                )),
                ast::Item::Enum(decl) => Some((decl.module, decl.public, Vec::new(), decl.unreflected.clone())),
                _ => None,
            };
            if let Some((module, public, private_fields, unreflected)) = owner {
                let name = match item {
                    ast::Item::Type(decl) => &decl.name.name,
                    ast::Item::Enum(decl) => &decl.name.name,
                    _ => unreachable!(),
                };
                let owner = Owner { module, public, private_fields, unreflected };
                self.owners.insert(self.shown_type_name(name, module), owner);
            }
            let name = match item {
                ast::Item::Type(decl) => &decl.name,
                ast::Item::Enum(decl) => &decl.name,
                ast::Item::Alias(decl) => {
                    let entry = AliasEntry {
                        target: decl.target.clone(),
                        span: decl.name.span,
                        resolved: None,
                        resolving: false,
                    };
                    self.aliases.push(entry);
                    self.declare_type_name(&decl.name, TypeName::Alias(self.aliases.len() - 1));
                    // The prelude's aliases are everywhere, by their own name,
                    // as `Panic`.
                    if let Some(bare) = decl.name.name.strip_prefix("prelude.")
                        && PRELUDE_TYPES.contains(&bare)
                    {
                        self.type_names.insert(bare.to_string(), TypeName::Alias(self.aliases.len() - 1));
                    }
                    continue;
                }
                ast::Item::Fn(_)
                | ast::Item::Trait(_)
                | ast::Item::Impl(_)
                | ast::Item::Const(_)
                | ast::Item::Test(_)
                | ast::Item::Derive(_) => continue,
            };
            let generics = match item {
                ast::Item::Type(decl) => &decl.generics,
                ast::Item::Enum(decl) => &decl.generics,
                _ => unreachable!(),
            };
            if let ast::Item::Type(decl) = item
                && !self.c_type_declaration(decl)
            {
                continue;
            }
            if !generics.is_empty() {
                if let ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Newtype(_), .. }) = item {
                    self.diags.unsupported(name.span, "generic newtypes");
                    continue;
                }
                let module = match item {
                    ast::Item::Type(decl) => decl.module,
                    ast::Item::Enum(decl) => decl.module,
                    _ => unreachable!(),
                };
                let params = self.declare_generics(generics, false, module);
                let template = self.types.declare_template(self.shown_type_name(&name.name, module), params.clone());
                if secret_type(item) {
                    self.types.hide_template(template);
                }
                let declaring = self.declaring_module(module);
                self.types.place_template(template, declaring);
                self.declare_type_name(name, TypeName::Template(template));
                templates.push((item, name, template, generics, params));
                continue;
            }
            let module = match item {
                ast::Item::Type(decl) => decl.module,
                ast::Item::Enum(decl) => decl.module,
                _ => unreachable!(),
            };
            let renamed = |path: &Vec<String>| {
                path == &["std", "panics"]
                    || matches!(path.as_slice(), [std, c] if std == "std" && crate::stdlib::completed(c).is_some())
            };
            // A type the prelude declares for every program, as `ByteOrder`.
            let everywhere = name.name.strip_prefix("prelude.").filter(|bare| PRELUDE_TYPES.contains(bare));
            let shown = match self.module_paths.get(module as usize).is_some_and(renamed) || everywhere.is_some() {
                true => self.shown_type_name(&name.name, module),
                false => name.name.clone(),
            };
            let (id, ty) = self.types.declare_adt(shown);
            if secret_type(item) {
                self.types.hide_adt(id);
            }
            let declaring = self.declaring_module(module);
            self.types.place_adt(id, declaring);
            if let ast::Item::Type(decl) = item
                && decl.context
            {
                self.context_of.insert(ty, self.contexts.len() as u32);
                self.contexts.push((ty, decl));
            }
            if let ast::Item::Type(decl) = item
                && decl.annotation
            {
                self.annotation_decls.insert(ty, decl);
            }
            if let ast::Item::Type(decl) = item
                && has_field_defaults(decl)
            {
                self.defaulted.push((Defaulted::Record(ty), ty, decl, Vec::new()));
            }
            if let ast::Item::Type(decl) = item
                && decl.foreign
            {
                let layout = self.c_layout(decl);
                self.types.foreign.insert(id, layout);
                self.c_decls.insert(id, decl);
            }
            self.declare_type_name(name, TypeName::Nominal(ty));
            if let Some(bare) = everywhere {
                self.type_names.insert(bare.to_string(), TypeName::Nominal(ty));
            }
            if let ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Newtype(_), .. }) = item {
                self.declare_module_name(name, ModuleName::Newtype(ty));
            }
            declared.push((item, name, id));
        }
        for &(item, _, id) in &declared {
            let kind = self.adt_kind(item);
            self.types.define_adt(id, kind);
        }
        for &(item, name, template, generics, ref params) in &templates {
            self.enter_generics(generics, params);
            let kind = self.adt_kind(item);
            self.leave_generics(generics);
            self.types.define_template(template, kind);
            // Productivity is checked on the instance with its own parameters.
            let ty = self.types.instantiate(template, params.clone());
            if let ast::Item::Type(decl) = item
                && has_field_defaults(decl)
            {
                self.defaulted.push((Defaulted::Template(template), ty, decl, params.clone()));
            }
            if let TyKind::Adt(id) = *self.types.kind(ty) {
                declared.push((item, name, id));
            }
        }
        for index in 0..self.aliases.len() {
            self.resolve_alias(index);
        }
        if let Some(name) = self.types.runaway.take() {
            let message = format!("the generic type `{name}` contains itself with ever larger type arguments");
            self.diags.error_without_span("E1107", message);
        }
        self.check_productivity(&declared);
    }

    /// Declares the parameters of a generic declaration with the
    /// capabilities their bounds grant.
    /// Declares type parameters. Those of functions, methods and impls are
    /// `Copy` unless they say `~Copy`; a type's parameters assume nothing
    /// (12.1).
    pub(super) fn declare_generics(&mut self, generics: &[ast::GenericParam], code: bool, module: u32) -> Vec<Ty> {
        let mut params = Vec::new();
        for generic in generics {
            // Outside the root, the root's types are out of sight: other
            // modules' are renamed, and only the prelude's keep their names.
            let name = generic.name.name.as_str();
            let visible = module == 0 || PRELUDE_TYPES.contains(&name);
            if (visible && self.type_named(name).is_some()) || self.type_params.iter().any(|(n, _)| n == name) {
                let message = format!("type parameter `{}` would shadow a visible type", generic.name.name);
                self.diags.error("E1003", generic.name.span, message);
            }
            let param = self.types.declare_param(generic.name.name.clone());
            self.types.param_mut(param).copy = code;
            params.push(param);
        }
        // A bound may mention any parameter of the list.
        self.enter_generics(generics, &params);
        for (generic, &param) in generics.iter().zip(&params) {
            for bound in &generic.bounds {
                self.apply_bound(param, bound);
            }
        }
        self.leave_generics(generics);
        params
    }

    /// Records what a bound lets generic code do with `param` (12.5).
    pub(super) fn apply_bound(&mut self, param: Ty, bound: &ast::TypeExpr) {
        if let ast::TypeKind::NotCopy = bound.kind {
            let def = self.types.param_mut(param);
            if !def.copy {
                let message =
                    "`~Copy` drops the `Copy` that parameters of functions, methods and impls assume; remove it here";
                self.diags.error("E1115", bound.span, message);
            }
            def.copy = false;
            return;
        }
        let ast::TypeKind::Path { segments, args } = &bound.kind else {
            self.diags.error("E1105", bound.span, "a bound names a trait or capability");
            return;
        };
        match (segments.as_slice(), args.is_empty()) {
            ([name], true) if name.name == "Equatable" => self.types.param_mut(param).equatable = true,
            ([name], true) if name.name == "Key" => self.types.param_mut(param).key = true,
            ([name], true) if name.name == "Copy" => self.types.param_mut(param).copy = true,
            ([name], true) if name.name == "Send" => self.types.param_mut(param).send = true,
            ([name], true) if name.name == "Share" => self.types.param_mut(param).share = true,
            ([name], false) if matches!(name.name.as_str(), "Call" | "CallMut" | "CallOnce") => {
                let [signature] = args.as_slice() else {
                    self.diags.error("E1104", bound.span, "a call bound takes one function type: `Call[fn(A): B]`");
                    return;
                };
                let signature = self.resolve_type(signature);
                if !matches!(self.types.kind(signature), TyKind::Fn(..)) {
                    self.diags.error("E1104", bound.span, "a call bound takes a function type: `Call[fn(A): B]`");
                    return;
                }
                let protocol = match name.name.as_str() {
                    "Call" => Protocol::Call,
                    "CallMut" => Protocol::CallMut,
                    _ => Protocol::CallOnce,
                };
                if self.call_bounds.insert(param, (protocol, signature)).is_some() {
                    self.diags.error("E1115", bound.span, "a type parameter has one call bound");
                }
            }
            _ => {
                if let Some((trait_id, args)) = self.trait_ref(bound) {
                    // An integer compares, keys a map and goes between tasks.
                    if Some(trait_id) == self.integer {
                        let def = self.types.param_mut(param);
                        (def.equatable, def.key, def.send, def.share) = (true, true, true, true);
                    }
                    let bounds = self.bounds.entry(param).or_default();
                    if bounds.contains(&(trait_id, args.clone())) {
                        self.diags.error("E1115", bound.span, "this bound is repeated");
                    } else {
                        bounds.push((trait_id, args));
                    }
                }
            }
        }
    }

    /// Brings the parameters of a generic declaration into scope.
    pub(super) fn enter_generics(&mut self, generics: &[ast::GenericParam], params: &[Ty]) {
        for (generic, &param) in generics.iter().zip(params) {
            self.type_params.push((generic.name.name.clone(), param));
        }
    }

    pub(super) fn leave_generics(&mut self, generics: &[ast::GenericParam]) {
        self.type_params.truncate(self.type_params.len() - generics.len());
    }

    pub(super) fn adt_kind(&mut self, item: &ast::Item) -> AdtKind {
        {
            match item {
                ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Newtype(inner), .. }) => {
                    AdtKind::Newtype(self.resolve_type(inner))
                }
                ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Record(fields), foreign: true, .. }) => {
                    AdtKind::Record(self.c_fields(fields))
                }
                ast::Item::Type(ast::TypeDecl {
                    name,
                    body: ast::TypeBody::Record(fields),
                    context,
                    public,
                    annotation,
                    ..
                }) => {
                    // Its module alone sees a private type, and so its fields.
                    if !public && !annotation {
                        for field in fields.iter().filter(|field| field.private) {
                            let message = format!(
                                "`priv` changes nothing in a type that is not `pub`, which only its module sees: remove it from `{}`, or make `{}` `pub`",
                                field.name.name, name.name
                            );
                            self.diags.error("E1506", field.name.span, message);
                        }
                    }
                    self.wires.insert(name.name.clone(), fields.iter().map(wire).collect());
                    AdtKind::Record(self.field_decls(fields, *context))
                }
                ast::Item::Type(ast::TypeDecl { body: ast::TypeBody::Opaque, .. }) => AdtKind::Record(Vec::new()),
                ast::Item::Enum(decl) => {
                    let mut seen = HashMap::new();
                    let mut variants = Vec::new();
                    for variant in &decl.variants {
                        if let Some(first) = seen.insert(variant.name.name.as_str(), variant.name.span) {
                            let message = format!("variant `{}` is declared more than once", variant.name.name);
                            self.diags.error("E1505", variant.name.span, message);
                            self.diags.related(first, format!("`{}` is first declared here", variant.name.name));
                        }
                        let payload = match &variant.payload {
                            ast::VariantPayload::Unit => Payload::Unit,
                            ast::VariantPayload::Tuple(items) => {
                                Payload::Tuple(items.iter().map(|t| self.resolve_type(t)).collect())
                            }
                            ast::VariantPayload::Record(fields) => {
                                let key = format!("{}.{}", decl.name.name, variant.name.name);
                                self.wires.insert(key, fields.iter().map(wire).collect());
                                Payload::Record(self.field_decls(fields, false))
                            }
                        };
                        variants.push(Variant { name: variant.name.name.clone(), payload });
                    }
                    AdtKind::Enum(variants)
                }
                _ => unreachable!(),
            }
        }
    }

    /// The defaults of the fields of records: constants, as a parameter's,
    /// checked where the record is declared, with its type parameters.
    pub(super) fn record_defaults(&mut self) {
        while let Some(&(key, ..)) = self.defaulted.first() {
            self.record_default(key);
        }
    }

    /// The defaults of the fields of a record, when first needed: a default
    /// may be a literal of another record that leaves out fields of its own.
    pub(super) fn record_default(&mut self, key: Defaulted) {
        let Some(index) = self.defaulted.iter().position(|d| d.0 == key) else { return };
        let (_, ty, decl, params) = self.defaulted.remove(index);
        let ast::TypeBody::Record(declared) = &decl.body else { return };
        let module = std::mem::replace(&mut self.module, decl.module);
        self.enter_generics(&decl.generics, &params);
        let fields = self.record_fields(ty, decl.span).unwrap_or_default();
        let mut cx = self.new_cx(Ty::UNIT);
        let mut defaults = Vec::new();
        for (field, (_, field_ty)) in declared.iter().zip(fields) {
            let default = field.default.as_ref().map(|default| {
                let value = self.expr(&mut cx, default, Some(field_ty));
                let mut value = self.coerce(value, field_ty);
                if !self.types.has_error(value.ty) && !constant(&mut value) {
                    let message = "a field's default value is a constant: a literal, `none`, `[]`, or a variant, tuple or record of constants";
                    self.diags.error("E1102", default.span, message);
                }
                value
            });
            defaults.push(default);
        }
        self.field_defaults.insert(key, defaults);
        self.leave_generics(&decl.generics);
        self.module = module;
    }

    /// The field defaults of a record type, its generic ones instantiated.
    pub(super) fn defaults_of(&mut self, ty: Ty) -> Option<Vec<Option<hir::Expr>>> {
        self.record_default(Defaulted::Record(ty));
        if let Some(defaults) = self.field_defaults.get(&Defaulted::Record(ty)) {
            return Some(defaults.clone());
        }
        let (template, args) = self.types.instance_of(ty)?;
        let args = args.to_vec();
        self.record_default(Defaulted::Template(template));
        let mut defaults = self.field_defaults.get(&Defaulted::Template(template))?.clone();
        let params = self.types.template(template).params.clone();
        let map: HashMap<Ty, Ty> = params.into_iter().zip(args).collect();
        for default in defaults.iter_mut().flatten() {
            self.instantiate(default, &map);
        }
        Some(defaults)
    }

    /// The fields of a record or a variant; of a context's record with
    /// `context`, whose fields take `@log` and `@label` (11.16).
    pub(super) fn field_decls(&mut self, fields: &[ast::FieldDecl], context: bool) -> Vec<Field> {
        let mut seen = HashMap::new();
        let mut resolved = Vec::new();
        for field in fields {
            if let Some(first) = seen.insert(field.name.name.as_str(), field.name.span) {
                let message = format!("field `{}` is declared more than once", field.name.name);
                self.diags.error("E1505", field.name.span, message);
                self.diags.related(first, format!("`{}` is first declared here", field.name.name));
            }
            let ty = self.resolve_type(&field.ty);
            self.vector_field(&field.name.name, ty, field.ty.span);
            for annotation in &field.annotations {
                match (annotation.name.name.as_str(), &annotation.argument) {
                    ("secret", None) => {}
                    ("secret", Some(_)) => {
                        self.diags.error("E1102", annotation.span, "`@secret` takes nothing: write `@secret`");
                    }
                    ("log" | "label", None | Some(ast::AnnotationArg::Text(_))) if context => {}
                    (name, _) if context => {
                        let message = format!(
                            "unknown annotation `@{name}`; a field of a context takes `@log`, which puts it in every log event and panic report, or `@label`, which makes it a label of profiles, each with an optional name: `@log(\"request_id\")`"
                        );
                        self.diags.error("E1616", annotation.span, message);
                    }
                    ("name", Some(_)) => {}
                    ("ignore", None) if field.default.is_some() => {}
                    ("ignore" | "omitNone", None)
                        if matches!(self.types.kind(ty), TyKind::Option(_) | TyKind::Error) => {}
                    ("omitNone", None) => {
                        let message = format!(
                            "`@omitNone` leaves out a field that is `none`, and `{}` is not optional: write `{}: {}?`, or leave out `@omitNone`",
                            field.name.name,
                            field.name.name,
                            self.types.display(ty)
                        );
                        self.diags.error("E1102", annotation.span, message);
                    }
                    ("ignore", None) => {
                        let message = format!(
                            "decoding leaves out an ignored field, so it has a default value or is optional: write `{}: {}?`",
                            field.name.name,
                            self.types.display(ty)
                        );
                        self.diags.error("E1102", annotation.span, message);
                    }
                    ("name", None) => {
                        let message = "`@name` takes the name the field has outside: `@name(\"user_id\")`";
                        self.diags.error("E1102", annotation.span, message);
                    }
                    _ => {
                        let message = format!(
                            "unknown annotation `@{}`; a field takes `@name(\"text\")`, `@ignore`, `@omitNone` or `@secret`",
                            annotation.name.name
                        );
                        self.diags.error("E1001", annotation.span, message);
                    }
                }
            }
            let secret = field.annotations.iter().any(|a| a.name.name == "secret");
            resolved.push(Field { name: field.name.name.clone(), ty, private: field.private, secret });
        }
        resolved
    }

    /// Productivity (9.10) as a least fixed point: a nominal type is
    /// productive once some finite value of it can be built from productive
    /// components. A recursive type that never becomes productive is an error.
    pub(super) fn check_productivity(&mut self, declared: &[(&ast::Item, &ast::Ident, AdtId)]) {
        let mut productive: HashSet<AdtId> = HashSet::new();
        loop {
            let before = productive.len();
            for &(_, _, id) in declared {
                if !productive.contains(&id) && self.adt_productive(id, &productive) {
                    productive.insert(id);
                }
            }
            if productive.len() == before {
                break;
            }
        }
        for &(_, name, id) in declared {
            let ty = self.types.intern(TyKind::Adt(id));
            if !productive.contains(&id) && self.types.reaches(ty, ty) {
                let message = format!("type `{}` contains itself and has no finite value", name.name);
                self.diags.error("E1107", name.span, message);
                self.types.define_adt(id, AdtKind::Newtype(Ty::ERROR));
            }
        }
    }

    pub(super) fn adt_productive(&self, id: AdtId, productive: &HashSet<AdtId>) -> bool {
        let all = |types: Vec<Ty>| types.iter().all(|&t| self.ty_productive(t, productive));
        match &self.types.adt(id).kind {
            AdtKind::Newtype(inner) => self.ty_productive(*inner, productive),
            AdtKind::Record(fields) => all(fields.iter().map(|f| f.ty).collect()),
            AdtKind::Enum(variants) => variants.iter().any(|v| all(v.payload.types())),
        }
    }

    pub(super) fn ty_productive(&self, ty: Ty, productive: &HashSet<AdtId>) -> bool {
        match self.types.kind(ty) {
            TyKind::Adt(id) => productive.contains(id),
            TyKind::Tuple(items) => items.iter().all(|&t| self.ty_productive(t, productive)),
            TyKind::Never => false,
            _ => true,
        }
    }

    pub(super) fn resolve_alias(&mut self, index: usize) -> Ty {
        let entry = &self.aliases[index];
        if let Some(ty) = entry.resolved {
            return ty;
        }
        if entry.resolving {
            let span = entry.span;
            self.diags.error("E1106", span, "transparent aliases form a cycle");
            self.aliases[index].resolved = Some(Ty::ERROR);
            return Ty::ERROR;
        }
        self.aliases[index].resolving = true;
        let target = self.aliases[index].target.clone();
        let ty = self.resolve_type(&target);
        self.aliases[index].resolved.get_or_insert(ty);
        self.aliases[index].resolved.unwrap()
    }

    /// The signature of `f` declared in `outer` (an impl or trait scope, or
    /// the module), recording the scope its body is checked in.
    /// Checks the default values of a function's parameters against their
    /// types, once every name of the module is known.
    pub(super) fn defaults(&mut self, f: &ast::FnDecl, id: FnId) {
        if f.params.iter().all(|p| p.default.is_none()) {
            return;
        }
        let module = std::mem::replace(&mut self.module, f.module);
        let mut defaults = Vec::new();
        for (index, param) in f.params.iter().enumerate() {
            let Some(default) = &param.default else {
                defaults.push(None);
                continue;
            };
            if param.mode != ast::Mode::Value || param.variadic {
                let message = if param.variadic {
                    "a variadic parameter has no default value: omitted, it is empty"
                } else {
                    "a loan parameter has no default value: it lends the caller's place"
                };
                self.diags.error("E1102", default.span, message);
                defaults.push(None);
                continue;
            }
            let ty = self.signatures[id.0 as usize].params[index];
            let mut cx = self.new_cx(Ty::UNIT);
            let value = self.expr(&mut cx, default, Some(ty));
            let mut value = self.coerce(value, ty);
            if !constant(&mut value) && !self.types.has_error(value.ty) {
                let message = "a default value is a constant: a literal, `none`, `[]`, or a variant, tuple or record of constants";
                self.diags.error("E1102", default.span, message);
            }
            defaults.push(Some(value));
        }
        self.module = module;
        self.signatures[id.0 as usize].defaults = defaults;
    }

    /// A trait method's parameters have no default values: implementations
    /// could not agree on them.
    pub(super) fn no_defaults(&mut self, f: &ast::FnDecl) {
        if let Some(default) = f.params.iter().find_map(|p| p.default.as_ref()) {
            let message = "a trait method's parameters have no default values; declare an inherent method `fn Type.name(...)` with them";
            self.diags.error("E1102", default.span, message);
        }
    }
}
