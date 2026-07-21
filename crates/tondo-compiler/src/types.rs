use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::package::{Namespace, SymbolIdentity};
use crate::source::{LogicalPath, ModulePath, SourceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(u32);

impl TypeId {
    pub fn index(self) -> u32 {
        self.0
    }
}

impl fmt::Display for TypeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "type#{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InferenceId(u32);

impl InferenceId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScalarType {
    Bool,
    Int,
    Float,
    Byte,
    Char,
    String,
    Unit,
    Never,
    Int8,
    Int16,
    Int32,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
}

impl ScalarType {
    pub const ALL: [Self; 16] = [
        Self::Bool,
        Self::Int,
        Self::Float,
        Self::Byte,
        Self::Char,
        Self::String,
        Self::Unit,
        Self::Never,
        Self::Int8,
        Self::Int16,
        Self::Int32,
        Self::UInt8,
        Self::UInt16,
        Self::UInt32,
        Self::UInt64,
        Self::Float32,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "Bool",
            Self::Int => "Int",
            Self::Float => "Float",
            Self::Byte => "Byte",
            Self::Char => "Char",
            Self::String => "String",
            Self::Unit => "Unit",
            Self::Never => "Never",
            Self::Int8 => "Int8",
            Self::Int16 => "Int16",
            Self::Int32 => "Int32",
            Self::UInt8 => "UInt8",
            Self::UInt16 => "UInt16",
            Self::UInt32 => "UInt32",
            Self::UInt64 => "UInt64",
            Self::Float32 => "Float32",
        }
    }
}

impl fmt::Display for ScalarType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IntrinsicType {
    Array,
    Map,
    Set,
    Range,
    Ref,
    Pointer,
    Join,
    Command,
    Pipeline,
    NumericConversionError,
}

impl IntrinsicType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Array => "Array",
            Self::Map => "Map",
            Self::Set => "Set",
            Self::Range => "Range",
            Self::Ref => "Ref",
            Self::Pointer => "Pointer",
            Self::Join => "Join",
            Self::Command => "Command",
            Self::Pipeline => "Pipeline",
            Self::NumericConversionError => "NumericConversionError",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Self::Map | Self::Join => 2,
            Self::Array | Self::Set | Self::Range | Self::Ref | Self::Pointer => 1,
            Self::Command | Self::Pipeline | Self::NumericConversionError => 0,
        }
    }
}

impl fmt::Display for IntrinsicType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParameterMode {
    Value,
    Ref,
    Mut,
    Var,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assignability {
    Exact,
    UnionInjection,
    UnionWidening,
    OptionLift,
    Diverging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericConversion {
    Identity,
    Total,
    Checked,
}

impl ParameterMode {
    fn prefix(self) -> &'static str {
        match self {
            Self::Value => "",
            Self::Ref => "ref ",
            Self::Mut => "mut ",
            Self::Var => "var ",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionParameter {
    mode: ParameterMode,
    ty: TypeId,
}

impl FunctionParameter {
    pub fn new(mode: ParameterMode, ty: TypeId) -> Self {
        Self { mode, ty }
    }

    pub fn mode(&self) -> ParameterMode {
        self.mode
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionType {
    is_async: bool,
    is_unsafe: bool,
    parameters: Vec<FunctionParameter>,
    variadic: Option<TypeId>,
    outcome: TypeId,
}

impl FunctionType {
    pub fn new(
        is_async: bool,
        is_unsafe: bool,
        parameters: Vec<FunctionParameter>,
        variadic: Option<TypeId>,
        outcome: TypeId,
    ) -> Self {
        Self {
            is_async,
            is_unsafe,
            parameters,
            variadic,
            outcome,
        }
    }

    pub fn is_async(&self) -> bool {
        self.is_async
    }

    pub fn is_unsafe(&self) -> bool {
        self.is_unsafe
    }

    pub fn parameters(&self) -> &[FunctionParameter] {
        &self.parameters
    }

    pub fn variadic(&self) -> Option<TypeId> {
        self.variadic
    }

    pub fn outcome(&self) -> TypeId {
        self.outcome
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GeneratedTypeKind {
    Closure,
    UnsafeClosure,
    AsyncClosure,
    AsyncUnsafeClosure,
}

impl GeneratedTypeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Closure => "closure",
            Self::UnsafeClosure => "unsafe-closure",
            Self::AsyncClosure => "async-closure",
            Self::AsyncUnsafeClosure => "async-unsafe-closure",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GeneratedTypeIdentity {
    kind: GeneratedTypeKind,
    source_id: SourceId,
    module: ModulePath,
    file: LogicalPath,
    start_byte: u32,
}

impl GeneratedTypeIdentity {
    pub fn new(
        kind: GeneratedTypeKind,
        source_id: SourceId,
        module: ModulePath,
        file: LogicalPath,
        start_byte: u32,
    ) -> Self {
        Self {
            kind,
            source_id,
            module,
            file,
            start_byte,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CursorMode {
    Own,
    Ref,
}

impl CursorMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Own => "own",
            Self::Ref => "ref",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TypeKind {
    Error,
    Scalar(ScalarType),
    Nominal {
        identity: SymbolIdentity,
        arguments: Vec<TypeId>,
    },
    Tuple(Vec<TypeId>),
    Function(FunctionType),
    Option(TypeId),
    Result {
        success: TypeId,
        error: TypeId,
    },
    Union(Vec<TypeId>),
    Intrinsic {
        constructor: IntrinsicType,
        arguments: Vec<TypeId>,
    },
    GenericParameter(u32),
    Inference(InferenceId),
    OpaqueResult(SymbolIdentity),
    Generated {
        identity: GeneratedTypeIdentity,
        arguments: Vec<TypeId>,
    },
    Cursor {
        mode: CursorMode,
        collection: TypeId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeError {
    ResourceLimit {
        limit: u32,
    },
    UnknownType(TypeId),
    InvalidNominalNamespace(Namespace),
    InvalidTupleArity(usize),
    InvalidIntrinsicArity {
        constructor: IntrinsicType,
        expected: usize,
        actual: usize,
    },
    MissingGenericArgument {
        position: u32,
        arity: usize,
    },
    UnresolvedInference(InferenceId),
    RecoveryTypeHasNoCanonicalName,
}

impl fmt::Display for TypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResourceLimit { limit } => {
                write!(formatter, "interned type node limit exceeded ({limit})")
            }
            Self::UnknownType(ty) => write!(formatter, "unknown {ty}"),
            Self::InvalidNominalNamespace(namespace) => write!(
                formatter,
                "a nominal type identity cannot use the `{namespace}` namespace"
            ),
            Self::InvalidTupleArity(arity) => {
                write!(
                    formatter,
                    "a tuple type requires at least two items, got {arity}"
                )
            }
            Self::InvalidIntrinsicArity {
                constructor,
                expected,
                actual,
            } => write!(
                formatter,
                "intrinsic `{constructor}` requires {expected} type arguments, got {actual}"
            ),
            Self::MissingGenericArgument { position, arity } => write!(
                formatter,
                "generic parameter ${position} has no argument in a substitution of arity {arity}"
            ),
            Self::UnresolvedInference(inference) => write!(
                formatter,
                "inference variable ${} has no canonical public type",
                inference.index()
            ),
            Self::RecoveryTypeHasNoCanonicalName => {
                formatter.write_str("the internal recovery type has no canonical public name")
            }
        }
    }
}

impl Error for TypeError {}

#[derive(Debug, Clone)]
pub struct TypeInterner {
    limit: u32,
    kinds: Vec<TypeKind>,
    by_kind: BTreeMap<TypeKind, TypeId>,
    next_inference: u32,
}

impl TypeInterner {
    pub fn new(limit: u32) -> Result<Self, TypeError> {
        let mut interner = Self {
            limit,
            kinds: Vec::new(),
            by_kind: BTreeMap::new(),
            next_inference: 0,
        };
        interner.intern_raw(TypeKind::Error)?;
        for scalar in ScalarType::ALL {
            interner.intern_raw(TypeKind::Scalar(scalar))?;
        }
        Ok(interner)
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    pub fn ids(&self) -> impl ExactSizeIterator<Item = TypeId> + '_ {
        (0..self.kinds.len()).map(|index| {
            TypeId(u32::try_from(index).expect("the type interner is limited to u32 entries"))
        })
    }

    pub fn kind(&self, ty: TypeId) -> Result<&TypeKind, TypeError> {
        self.kinds
            .get(ty.0 as usize)
            .ok_or(TypeError::UnknownType(ty))
    }

    pub fn error(&self) -> TypeId {
        self.existing(&TypeKind::Error)
    }

    pub fn scalar(&self, scalar: ScalarType) -> TypeId {
        self.existing(&TypeKind::Scalar(scalar))
    }

    pub fn named_scalar(&self, name: &str) -> Option<TypeId> {
        let scalar = match name {
            "Bool" => ScalarType::Bool,
            "Int" | "Int64" => ScalarType::Int,
            "Float" | "Float64" => ScalarType::Float,
            "Byte" => ScalarType::Byte,
            "Char" => ScalarType::Char,
            "String" => ScalarType::String,
            "Unit" => ScalarType::Unit,
            "Never" => ScalarType::Never,
            "Int8" => ScalarType::Int8,
            "Int16" => ScalarType::Int16,
            "Int32" => ScalarType::Int32,
            "UInt8" => ScalarType::UInt8,
            "UInt16" => ScalarType::UInt16,
            "UInt32" => ScalarType::UInt32,
            "UInt64" => ScalarType::UInt64,
            "Float32" => ScalarType::Float32,
            _ => return None,
        };
        Some(self.scalar(scalar))
    }

    pub fn nominal(
        &mut self,
        identity: SymbolIdentity,
        arguments: Vec<TypeId>,
    ) -> Result<TypeId, TypeError> {
        if identity.namespace() != Namespace::Type {
            return Err(TypeError::InvalidNominalNamespace(identity.namespace()));
        }
        self.validate_children(&arguments)?;
        self.intern_raw(TypeKind::Nominal {
            identity,
            arguments,
        })
    }

    pub fn tuple(&mut self, items: Vec<TypeId>) -> Result<TypeId, TypeError> {
        if items.len() < 2 {
            return Err(TypeError::InvalidTupleArity(items.len()));
        }
        self.validate_children(&items)?;
        self.intern_raw(TypeKind::Tuple(items))
    }

    pub fn function(&mut self, function: FunctionType) -> Result<TypeId, TypeError> {
        for parameter in &function.parameters {
            self.validate_child(parameter.ty)?;
        }
        if let Some(variadic) = function.variadic {
            self.validate_child(variadic)?;
        }
        self.validate_child(function.outcome)?;
        self.intern_raw(TypeKind::Function(function))
    }

    pub fn option(&mut self, item: TypeId) -> Result<TypeId, TypeError> {
        self.validate_child(item)?;
        self.intern_raw(TypeKind::Option(item))
    }

    pub fn result(&mut self, success: TypeId, error: TypeId) -> Result<TypeId, TypeError> {
        self.validate_child(success)?;
        self.validate_child(error)?;
        self.intern_raw(TypeKind::Result { success, error })
    }

    pub fn union(
        &mut self,
        members: impl IntoIterator<Item = TypeId>,
    ) -> Result<TypeId, TypeError> {
        let never = self.scalar(ScalarType::Never);
        let mut pending = members.into_iter().collect::<Vec<_>>();
        let mut normalized = BTreeMap::<String, TypeId>::new();
        while let Some(member) = pending.pop() {
            self.validate_child(member)?;
            if member == never {
                continue;
            }
            if let TypeKind::Union(items) = self.kind(member)? {
                pending.extend(items.iter().copied());
                continue;
            }
            normalized.insert(self.canonical(member)?, member);
        }
        match normalized.len() {
            0 => Ok(never),
            1 => Ok(*normalized
                .values()
                .next()
                .expect("a one-item normalized union has a member")),
            _ => self.intern_raw(TypeKind::Union(normalized.into_values().collect())),
        }
    }

    pub fn intrinsic(
        &mut self,
        constructor: IntrinsicType,
        arguments: Vec<TypeId>,
    ) -> Result<TypeId, TypeError> {
        let expected = constructor.arity();
        if arguments.len() != expected {
            return Err(TypeError::InvalidIntrinsicArity {
                constructor,
                expected,
                actual: arguments.len(),
            });
        }
        self.validate_children(&arguments)?;
        self.intern_raw(TypeKind::Intrinsic {
            constructor,
            arguments,
        })
    }

    pub fn generic_parameter(&mut self, position: u32) -> Result<TypeId, TypeError> {
        self.intern_raw(TypeKind::GenericParameter(position))
    }

    pub fn fresh_inference(&mut self) -> Result<TypeId, TypeError> {
        let inference = InferenceId(self.next_inference);
        let next_inference = self
            .next_inference
            .checked_add(1)
            .ok_or(TypeError::ResourceLimit { limit: self.limit })?;
        let ty = self.intern_raw(TypeKind::Inference(inference))?;
        self.next_inference = next_inference;
        Ok(ty)
    }

    pub fn opaque_result(&mut self, identity: SymbolIdentity) -> Result<TypeId, TypeError> {
        if identity.namespace() != Namespace::Value {
            return Err(TypeError::InvalidNominalNamespace(identity.namespace()));
        }
        self.intern_raw(TypeKind::OpaqueResult(identity))
    }

    pub fn generated(
        &mut self,
        identity: GeneratedTypeIdentity,
        arguments: Vec<TypeId>,
    ) -> Result<TypeId, TypeError> {
        self.validate_children(&arguments)?;
        self.intern_raw(TypeKind::Generated {
            identity,
            arguments,
        })
    }

    pub fn cursor(&mut self, mode: CursorMode, collection: TypeId) -> Result<TypeId, TypeError> {
        self.validate_child(collection)?;
        self.intern_raw(TypeKind::Cursor { mode, collection })
    }

    pub fn canonical(&self, ty: TypeId) -> Result<String, TypeError> {
        self.render_iterative(ty)
    }

    /// Classifies the closed, top-level assignment relation from Tondo 0.1.
    ///
    /// This deliberately does not recurse through generic applications,
    /// options, results, tuples, or functions: every such constructor is
    /// invariant. Contextual `none` has no source `TypeId` and is handled by
    /// expression checking.
    pub fn assignability(
        &self,
        actual: TypeId,
        expected: TypeId,
    ) -> Result<Option<Assignability>, TypeError> {
        self.validate_child(actual)?;
        self.validate_child(expected)?;
        if actual == expected {
            return Ok(Some(Assignability::Exact));
        }
        if actual == self.scalar(ScalarType::Never) {
            return Ok(Some(Assignability::Diverging));
        }

        if let TypeKind::Union(expected_members) = self.kind(expected)? {
            let actual_members = match self.kind(actual)? {
                TypeKind::Union(members) => members.as_slice(),
                _ => std::slice::from_ref(&actual),
            };
            if actual_members
                .iter()
                .all(|member| expected_members.contains(member))
            {
                return Ok(Some(if actual_members.len() == 1 {
                    Assignability::UnionInjection
                } else {
                    Assignability::UnionWidening
                }));
            }
        }

        if let TypeKind::Option(item) = self.kind(expected)?
            && *item == actual
        {
            return Ok(Some(Assignability::OptionLift));
        }
        Ok(None)
    }

    /// `none` is contextual rather than a standalone type, and is accepted
    /// only by a direct option expectation.
    pub fn accepts_none(&self, expected: TypeId) -> Result<bool, TypeError> {
        Ok(matches!(self.kind(expected)?, TypeKind::Option(_)))
    }

    /// Returns whether two canonical type patterns have a first-order
    /// unifier, treating generic parameters as the only variables.
    pub fn first_order_unifiable(&self, left: TypeId, right: TypeId) -> Result<bool, TypeError> {
        self.validate_child(left)?;
        self.validate_child(right)?;
        let mut substitutions = BTreeMap::new();
        self.unify_iterative(left, right, &mut substitutions)
    }

    fn unify_iterative(
        &self,
        left: TypeId,
        right: TypeId,
        substitutions: &mut BTreeMap<u32, TypeId>,
    ) -> Result<bool, TypeError> {
        let mut pending = vec![(left, right)];
        while let Some((left, right)) = pending.pop() {
            let left = self.resolve_generic(left, substitutions)?;
            let right = self.resolve_generic(right, substitutions)?;
            if left == right {
                continue;
            }
            match (self.kind(left)?.clone(), self.kind(right)?.clone()) {
                (TypeKind::GenericParameter(position), _) => {
                    if !self.bind_generic(position, right, substitutions)? {
                        return Ok(false);
                    }
                }
                (_, TypeKind::GenericParameter(position)) => {
                    if !self.bind_generic(position, left, substitutions)? {
                        return Ok(false);
                    }
                }
                (TypeKind::Scalar(left), TypeKind::Scalar(right)) if left == right => {}
                (
                    TypeKind::Nominal {
                        identity: left_identity,
                        arguments: left_arguments,
                    },
                    TypeKind::Nominal {
                        identity: right_identity,
                        arguments: right_arguments,
                    },
                ) if left_identity == right_identity
                    && left_arguments.len() == right_arguments.len() =>
                {
                    pending.extend(left_arguments.into_iter().zip(right_arguments));
                }
                (TypeKind::Tuple(left), TypeKind::Tuple(right))
                | (TypeKind::Union(left), TypeKind::Union(right))
                    if left.len() == right.len() =>
                {
                    pending.extend(left.into_iter().zip(right));
                }
                (TypeKind::Function(left), TypeKind::Function(right))
                    if left.is_async == right.is_async
                        && left.is_unsafe == right.is_unsafe
                        && left.parameters.len() == right.parameters.len()
                        && left.variadic.is_some() == right.variadic.is_some() =>
                {
                    for (left, right) in left.parameters.into_iter().zip(right.parameters) {
                        if left.mode != right.mode {
                            return Ok(false);
                        }
                        pending.push((left.ty, right.ty));
                    }
                    if let (Some(left), Some(right)) = (left.variadic, right.variadic) {
                        pending.push((left, right));
                    }
                    pending.push((left.outcome, right.outcome));
                }
                (TypeKind::Option(left), TypeKind::Option(right)) => {
                    pending.push((left, right));
                }
                (
                    TypeKind::Result {
                        success: left_success,
                        error: left_error,
                    },
                    TypeKind::Result {
                        success: right_success,
                        error: right_error,
                    },
                ) => {
                    pending.push((left_success, right_success));
                    pending.push((left_error, right_error));
                }
                (
                    TypeKind::Intrinsic {
                        constructor: left_constructor,
                        arguments: left_arguments,
                    },
                    TypeKind::Intrinsic {
                        constructor: right_constructor,
                        arguments: right_arguments,
                    },
                ) if left_constructor == right_constructor
                    && left_arguments.len() == right_arguments.len() =>
                {
                    pending.extend(left_arguments.into_iter().zip(right_arguments));
                }
                (TypeKind::OpaqueResult(left), TypeKind::OpaqueResult(right)) if left == right => {}
                (
                    TypeKind::Generated {
                        identity: left_identity,
                        arguments: left_arguments,
                    },
                    TypeKind::Generated {
                        identity: right_identity,
                        arguments: right_arguments,
                    },
                ) if left_identity == right_identity
                    && left_arguments.len() == right_arguments.len() =>
                {
                    pending.extend(left_arguments.into_iter().zip(right_arguments));
                }
                (
                    TypeKind::Cursor {
                        mode: left_mode,
                        collection: left_collection,
                    },
                    TypeKind::Cursor {
                        mode: right_mode,
                        collection: right_collection,
                    },
                ) if left_mode == right_mode => pending.push((left_collection, right_collection)),
                (TypeKind::Inference(left), TypeKind::Inference(right)) if left == right => {}
                (TypeKind::Error, _) | (_, TypeKind::Error) => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    fn resolve_generic(
        &self,
        mut ty: TypeId,
        substitutions: &BTreeMap<u32, TypeId>,
    ) -> Result<TypeId, TypeError> {
        let mut remaining = substitutions.len().saturating_add(1);
        while let TypeKind::GenericParameter(position) = self.kind(ty)? {
            let Some(replacement) = substitutions.get(position).copied() else {
                break;
            };
            ty = replacement;
            remaining = remaining
                .checked_sub(1)
                .expect("occurs checks prevent cyclic generic substitutions");
        }
        Ok(ty)
    }

    fn bind_generic(
        &self,
        position: u32,
        ty: TypeId,
        substitutions: &mut BTreeMap<u32, TypeId>,
    ) -> Result<bool, TypeError> {
        if self.occurs(position, ty, substitutions)? {
            return Ok(false);
        }
        substitutions.insert(position, ty);
        Ok(true)
    }

    fn occurs(
        &self,
        position: u32,
        ty: TypeId,
        substitutions: &BTreeMap<u32, TypeId>,
    ) -> Result<bool, TypeError> {
        let mut pending = vec![ty];
        let mut visited = BTreeMap::<TypeId, ()>::new();
        while let Some(ty) = pending.pop() {
            let ty = self.resolve_generic(ty, substitutions)?;
            if visited.insert(ty, ()).is_some() {
                continue;
            }
            match self.kind(ty)? {
                TypeKind::GenericParameter(other) => {
                    if *other == position {
                        return Ok(true);
                    }
                }
                TypeKind::Nominal { arguments, .. }
                | TypeKind::Tuple(arguments)
                | TypeKind::Union(arguments)
                | TypeKind::Intrinsic { arguments, .. }
                | TypeKind::Generated { arguments, .. } => {
                    pending.extend(arguments.iter().copied());
                }
                TypeKind::Function(function) => {
                    pending.extend(function.parameters.iter().map(|parameter| parameter.ty));
                    pending.extend(function.variadic);
                    pending.push(function.outcome);
                }
                TypeKind::Option(item) => pending.push(*item),
                TypeKind::Result { success, error } => {
                    pending.push(*success);
                    pending.push(*error);
                }
                TypeKind::Cursor { collection, .. } => pending.push(*collection),
                TypeKind::Error
                | TypeKind::Scalar(_)
                | TypeKind::Inference(_)
                | TypeKind::OpaqueResult(_) => {}
            }
        }
        Ok(false)
    }

    fn existing(&self, kind: &TypeKind) -> TypeId {
        *self
            .by_kind
            .get(kind)
            .expect("bootstrap types are interned during construction")
    }

    fn validate_child(&self, ty: TypeId) -> Result<(), TypeError> {
        self.kind(ty).map(|_| ())
    }

    fn validate_children(&self, types: &[TypeId]) -> Result<(), TypeError> {
        for ty in types {
            self.validate_child(*ty)?;
        }
        Ok(())
    }

    fn intern_raw(&mut self, kind: TypeKind) -> Result<TypeId, TypeError> {
        if let Some(existing) = self.by_kind.get(&kind) {
            return Ok(*existing);
        }
        if self.kinds.len() >= self.limit as usize {
            return Err(TypeError::ResourceLimit { limit: self.limit });
        }
        let id = TypeId(
            u32::try_from(self.kinds.len())
                .map_err(|_| TypeError::ResourceLimit { limit: self.limit })?,
        );
        self.kinds.push(kind.clone());
        self.by_kind.insert(kind, id);
        Ok(id)
    }

    fn render_iterative(&self, root: TypeId) -> Result<String, TypeError> {
        self.kind(root)?;
        let mut output = String::new();
        let mut pending = vec![RenderTask::Type(root, Precedence::Union)];
        while let Some(task) = pending.pop() {
            let (ty, minimum) = match task {
                RenderTask::Text(text) => {
                    output.push_str(&text);
                    continue;
                }
                RenderTask::Type(ty, minimum) => (ty, minimum),
            };
            let kind = self.kind(ty)?.clone();
            if precedence(&kind) < minimum {
                output.push('(');
                pending.push(RenderTask::Text(")".into()));
            }
            match kind {
                TypeKind::Error => return Err(TypeError::RecoveryTypeHasNoCanonicalName),
                TypeKind::Scalar(scalar) => output.push_str(scalar.as_str()),
                TypeKind::Nominal {
                    identity,
                    arguments,
                } => {
                    output.push_str(&identity.canonical_name());
                    push_application(&mut output, &mut pending, &arguments);
                }
                TypeKind::Tuple(items) => {
                    output.push('(');
                    pending.push(RenderTask::Text(")".into()));
                    push_render_sequence(&mut pending, &items, Precedence::Union, ", ");
                }
                TypeKind::Function(function) => {
                    output.push_str(match (function.is_async, function.is_unsafe) {
                        (false, false) => "fn(",
                        (true, false) => "async fn(",
                        (false, true) => "unsafe fn(",
                        (true, true) => "async unsafe fn(",
                    });
                    if function.outcome != self.scalar(ScalarType::Unit) {
                        pending.push(RenderTask::Type(function.outcome, Precedence::Union));
                        pending.push(RenderTask::Text(": ".into()));
                    }
                    pending.push(RenderTask::Text(")".into()));
                    let mut items = function
                        .parameters
                        .iter()
                        .map(|parameter| {
                            (
                                parameter.mode.prefix().to_owned(),
                                parameter.ty,
                                if parameter.mode == ParameterMode::Value {
                                    Precedence::Union
                                } else {
                                    Precedence::Optional
                                },
                            )
                        })
                        .collect::<Vec<_>>();
                    if let Some(variadic) = function.variadic {
                        items.push(("...".into(), variadic, Precedence::Union));
                    }
                    push_render_items(&mut pending, &items);
                }
                TypeKind::Option(item) => {
                    pending.push(RenderTask::Text("?".into()));
                    pending.push(RenderTask::Type(item, Precedence::Primary));
                }
                TypeKind::Result { success, error } => {
                    pending.push(RenderTask::Type(error, Precedence::Optional));
                    if success == self.scalar(ScalarType::Unit) {
                        output.push('!');
                    } else {
                        pending.push(RenderTask::Text(" ! ".into()));
                        pending.push(RenderTask::Type(success, Precedence::Optional));
                    }
                }
                TypeKind::Union(members) => {
                    push_render_sequence(&mut pending, &members, Precedence::Union, " | ");
                }
                TypeKind::Intrinsic {
                    constructor,
                    arguments,
                } => {
                    output.push_str(constructor.as_str());
                    push_application(&mut output, &mut pending, &arguments);
                }
                TypeKind::GenericParameter(position) => {
                    output.push('$');
                    output.push_str(&position.to_string());
                }
                TypeKind::Inference(inference) => {
                    return Err(TypeError::UnresolvedInference(inference));
                }
                TypeKind::OpaqueResult(identity) => {
                    output.push_str(&identity.canonical_name());
                    output.push_str("#result");
                }
                TypeKind::Generated {
                    identity,
                    arguments,
                } => {
                    output.push_str("generated[");
                    output.push_str(&json_string(identity.kind.as_str()));
                    output.push(',');
                    output.push_str(&json_string(identity.source_id.as_str()));
                    output.push(',');
                    output.push_str(&json_string(identity.module.as_str()));
                    output.push(',');
                    output.push_str(&json_string(identity.file.as_str()));
                    output.push(',');
                    output.push_str(&identity.start_byte.to_string());
                    output.push(']');
                    push_application(&mut output, &mut pending, &arguments);
                }
                TypeKind::Cursor { mode, collection } => {
                    output.push_str("cursor[");
                    output.push_str(mode.as_str());
                    output.push(',');
                    pending.push(RenderTask::Text("]".into()));
                    pending.push(RenderTask::Type(collection, Precedence::Union));
                }
            }
        }
        Ok(output)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeSubstitution {
    arguments: Vec<TypeId>,
}

impl TypeSubstitution {
    pub fn new(arguments: Vec<TypeId>) -> Self {
        Self { arguments }
    }

    pub fn arguments(&self) -> &[TypeId] {
        &self.arguments
    }

    pub fn apply(&self, interner: &mut TypeInterner, ty: TypeId) -> Result<TypeId, TypeError> {
        interner.kind(ty)?;
        let mut memo = BTreeMap::<TypeId, TypeId>::new();
        let mut pending = vec![(ty, false)];
        while let Some((current, expanded)) = pending.pop() {
            if memo.contains_key(&current) {
                continue;
            }
            let kind = interner.kind(current)?.clone();
            if !expanded {
                match kind {
                    TypeKind::GenericParameter(position) => {
                        let substituted = *self.arguments.get(position as usize).ok_or(
                            TypeError::MissingGenericArgument {
                                position,
                                arity: self.arguments.len(),
                            },
                        )?;
                        interner.kind(substituted)?;
                        memo.insert(current, substituted);
                    }
                    TypeKind::Error
                    | TypeKind::Scalar(_)
                    | TypeKind::Inference(_)
                    | TypeKind::OpaqueResult(_) => {
                        memo.insert(current, current);
                    }
                    _ => {
                        pending.push((current, true));
                        push_type_children(&kind, &mut pending);
                    }
                }
                continue;
            }
            let get = |child: TypeId| {
                memo.get(&child)
                    .copied()
                    .expect("all substitution children are rebuilt before their parent")
            };
            let substituted = match kind {
                TypeKind::GenericParameter(position) => *self
                    .arguments
                    .get(position as usize)
                    .ok_or(TypeError::MissingGenericArgument {
                        position,
                        arity: self.arguments.len(),
                    })?,
                TypeKind::Error | TypeKind::Scalar(_) | TypeKind::Inference(_) => current,
                TypeKind::Nominal {
                    identity,
                    arguments,
                } => interner.nominal(identity, arguments.into_iter().map(get).collect())?,
                TypeKind::Tuple(items) => interner.tuple(items.into_iter().map(get).collect())?,
                TypeKind::Function(function) => interner.function(FunctionType::new(
                    function.is_async,
                    function.is_unsafe,
                    function
                        .parameters
                        .into_iter()
                        .map(|parameter| FunctionParameter::new(parameter.mode, get(parameter.ty)))
                        .collect(),
                    function.variadic.map(get),
                    get(function.outcome),
                ))?,
                TypeKind::Option(item) => interner.option(get(item))?,
                TypeKind::Result { success, error } => interner.result(get(success), get(error))?,
                TypeKind::Union(members) => interner.union(members.into_iter().map(get))?,
                TypeKind::Intrinsic {
                    constructor,
                    arguments,
                } => interner.intrinsic(constructor, arguments.into_iter().map(get).collect())?,
                TypeKind::OpaqueResult(_) => current,
                TypeKind::Generated {
                    identity,
                    arguments,
                } => interner.generated(identity, arguments.into_iter().map(get).collect())?,
                TypeKind::Cursor { mode, collection } => interner.cursor(mode, get(collection))?,
            };
            memo.insert(current, substituted);
        }
        Ok(memo[&ty])
    }
}

impl Default for TypeInterner {
    fn default() -> Self {
        Self::new(4_000_000).expect("the default type budget contains bootstrap scalars")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceError {
    Type(TypeError),
    Mismatch {
        left: TypeId,
        right: TypeId,
    },
    RecursiveSolution {
        inference: InferenceId,
        within: TypeId,
    },
    Unsolved(InferenceId),
}

impl fmt::Display for InferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Type(error) => error.fmt(formatter),
            Self::Mismatch { left, right } => {
                write!(formatter, "cannot equate {left} with {right}")
            }
            Self::RecursiveSolution { inference, within } => write!(
                formatter,
                "inference variable ${} occurs recursively in {within}",
                inference.index()
            ),
            Self::Unsolved(inference) => {
                write!(
                    formatter,
                    "inference variable ${} is unsolved",
                    inference.index()
                )
            }
        }
    }
}

impl Error for InferenceError {}

impl From<TypeError> for InferenceError {
    fn from(error: TypeError) -> Self {
        Self::Type(error)
    }
}

/// Request-local, non-generalizing inference state for one expression or
/// callable body. Generic parameters remain rigid; only explicit
/// [`TypeKind::Inference`] nodes can acquire solutions.
#[derive(Debug, Clone, Default)]
pub struct InferenceContext {
    solutions: BTreeMap<InferenceId, TypeId>,
}

impl InferenceContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh(&mut self, interner: &mut TypeInterner) -> Result<TypeId, InferenceError> {
        Ok(interner.fresh_inference()?)
    }

    pub fn solution(&self, inference: InferenceId) -> Option<TypeId> {
        self.solutions.get(&inference).copied()
    }

    /// Adds an invariant equality constraint. On failure, all solutions added
    /// by this call are rolled back so another contextual interpretation can be
    /// attempted without leaking partial state.
    pub fn equate(
        &mut self,
        interner: &TypeInterner,
        left: TypeId,
        right: TypeId,
    ) -> Result<(), InferenceError> {
        interner.kind(left)?;
        interner.kind(right)?;
        let checkpoint = self.solutions.clone();
        let result = self.equate_inner(interner, left, right);
        if result.is_err() {
            self.solutions = checkpoint;
        }
        result
    }

    fn equate_inner(
        &mut self,
        interner: &TypeInterner,
        left: TypeId,
        right: TypeId,
    ) -> Result<(), InferenceError> {
        let mut pending = vec![(left, right)];
        while let Some((left, right)) = pending.pop() {
            let left = self.resolve_head(interner, left)?;
            let right = self.resolve_head(interner, right)?;
            if left == right {
                continue;
            }
            let left_kind = interner.kind(left)?.clone();
            let right_kind = interner.kind(right)?.clone();
            match (left_kind, right_kind) {
                (TypeKind::Inference(left), TypeKind::Inference(right)) => {
                    if left < right {
                        self.bind(interner, right, left_type(interner, left)?)?;
                    } else {
                        self.bind(interner, left, left_type(interner, right)?)?;
                    }
                }
                (TypeKind::Inference(inference), _) => {
                    self.bind(interner, inference, right)?;
                }
                (_, TypeKind::Inference(inference)) => {
                    self.bind(interner, inference, left)?;
                }
                (TypeKind::Error, _) | (_, TypeKind::Error) => {}
                (TypeKind::Scalar(left), TypeKind::Scalar(right)) if left == right => {}
                (
                    TypeKind::Nominal {
                        identity: left_identity,
                        arguments: left_arguments,
                    },
                    TypeKind::Nominal {
                        identity: right_identity,
                        arguments: right_arguments,
                    },
                ) if left_identity == right_identity
                    && left_arguments.len() == right_arguments.len() =>
                {
                    pending.extend(left_arguments.into_iter().zip(right_arguments));
                }
                (TypeKind::Tuple(left), TypeKind::Tuple(right))
                | (TypeKind::Union(left), TypeKind::Union(right))
                    if left.len() == right.len() =>
                {
                    pending.extend(left.into_iter().zip(right));
                }
                (TypeKind::Function(left), TypeKind::Function(right))
                    if left.is_async == right.is_async
                        && left.is_unsafe == right.is_unsafe
                        && left.parameters.len() == right.parameters.len()
                        && left.variadic.is_some() == right.variadic.is_some() =>
                {
                    for (left, right) in left.parameters.into_iter().zip(right.parameters) {
                        if left.mode != right.mode {
                            return Err(InferenceError::Mismatch {
                                left: left.ty,
                                right: right.ty,
                            });
                        }
                        pending.push((left.ty, right.ty));
                    }
                    if let (Some(left), Some(right)) = (left.variadic, right.variadic) {
                        pending.push((left, right));
                    }
                    pending.push((left.outcome, right.outcome));
                }
                (TypeKind::Option(left), TypeKind::Option(right)) => {
                    pending.push((left, right));
                }
                (
                    TypeKind::Result {
                        success: left_success,
                        error: left_error,
                    },
                    TypeKind::Result {
                        success: right_success,
                        error: right_error,
                    },
                ) => {
                    pending.push((left_success, right_success));
                    pending.push((left_error, right_error));
                }
                (
                    TypeKind::Intrinsic {
                        constructor: left_constructor,
                        arguments: left_arguments,
                    },
                    TypeKind::Intrinsic {
                        constructor: right_constructor,
                        arguments: right_arguments,
                    },
                ) if left_constructor == right_constructor
                    && left_arguments.len() == right_arguments.len() =>
                {
                    pending.extend(left_arguments.into_iter().zip(right_arguments));
                }
                (TypeKind::GenericParameter(left), TypeKind::GenericParameter(right))
                    if left == right => {}
                (TypeKind::OpaqueResult(left), TypeKind::OpaqueResult(right)) if left == right => {}
                (
                    TypeKind::Generated {
                        identity: left_identity,
                        arguments: left_arguments,
                    },
                    TypeKind::Generated {
                        identity: right_identity,
                        arguments: right_arguments,
                    },
                ) if left_identity == right_identity
                    && left_arguments.len() == right_arguments.len() =>
                {
                    pending.extend(left_arguments.into_iter().zip(right_arguments));
                }
                (
                    TypeKind::Cursor {
                        mode: left_mode,
                        collection: left_collection,
                    },
                    TypeKind::Cursor {
                        mode: right_mode,
                        collection: right_collection,
                    },
                ) if left_mode == right_mode => pending.push((left_collection, right_collection)),
                _ => return Err(InferenceError::Mismatch { left, right }),
            }
        }
        Ok(())
    }

    fn bind(
        &mut self,
        interner: &TypeInterner,
        inference: InferenceId,
        ty: TypeId,
    ) -> Result<(), InferenceError> {
        if self.occurs(interner, inference, ty)? {
            return Err(InferenceError::RecursiveSolution {
                inference,
                within: ty,
            });
        }
        self.solutions.insert(inference, ty);
        Ok(())
    }

    fn occurs(
        &self,
        interner: &TypeInterner,
        inference: InferenceId,
        root: TypeId,
    ) -> Result<bool, InferenceError> {
        let mut pending = vec![root];
        let mut visited = BTreeMap::<TypeId, ()>::new();
        while let Some(ty) = pending.pop() {
            if visited.insert(ty, ()).is_some() {
                continue;
            }
            match interner.kind(ty)? {
                TypeKind::Inference(found) => {
                    if *found == inference {
                        return Ok(true);
                    }
                    if let Some(solution) = self.solutions.get(found) {
                        pending.push(*solution);
                    }
                }
                TypeKind::Nominal { arguments, .. }
                | TypeKind::Tuple(arguments)
                | TypeKind::Union(arguments)
                | TypeKind::Intrinsic { arguments, .. }
                | TypeKind::Generated { arguments, .. } => {
                    pending.extend(arguments.iter().copied());
                }
                TypeKind::Function(function) => {
                    pending.extend(function.parameters.iter().map(|parameter| parameter.ty));
                    pending.extend(function.variadic);
                    pending.push(function.outcome);
                }
                TypeKind::Option(item) => pending.push(*item),
                TypeKind::Result { success, error } => {
                    pending.push(*success);
                    pending.push(*error);
                }
                TypeKind::Cursor { collection, .. } => pending.push(*collection),
                TypeKind::Error
                | TypeKind::Scalar(_)
                | TypeKind::GenericParameter(_)
                | TypeKind::OpaqueResult(_) => {}
            }
        }
        Ok(false)
    }

    fn resolve_head(
        &self,
        interner: &TypeInterner,
        mut ty: TypeId,
    ) -> Result<TypeId, InferenceError> {
        let mut remaining = self.solutions.len().saturating_add(1);
        while let TypeKind::Inference(inference) = interner.kind(ty)? {
            let Some(solution) = self.solutions.get(inference).copied() else {
                break;
            };
            ty = solution;
            remaining = remaining
                .checked_sub(1)
                .ok_or(InferenceError::RecursiveSolution {
                    inference: *inference,
                    within: ty,
                })?;
        }
        Ok(ty)
    }

    pub fn resolve(
        &self,
        interner: &mut TypeInterner,
        root: TypeId,
    ) -> Result<TypeId, InferenceError> {
        interner.kind(root)?;
        let mut memo = BTreeMap::<TypeId, TypeId>::new();
        let mut pending = vec![(root, false)];
        while let Some((original, expanded)) = pending.pop() {
            if memo.contains_key(&original) {
                continue;
            }
            let ty = self.resolve_head(interner, original)?;
            if ty != original {
                if expanded {
                    let resolved = *memo
                        .get(&ty)
                        .expect("a resolved inference target is visited first");
                    memo.insert(original, resolved);
                } else {
                    pending.push((original, true));
                    pending.push((ty, false));
                }
                continue;
            }
            let kind = interner.kind(ty)?.clone();
            if !expanded {
                match &kind {
                    TypeKind::Inference(inference) => {
                        return Err(InferenceError::Unsolved(*inference));
                    }
                    TypeKind::Error
                    | TypeKind::Scalar(_)
                    | TypeKind::GenericParameter(_)
                    | TypeKind::OpaqueResult(_) => {
                        memo.insert(original, original);
                    }
                    _ => {
                        pending.push((original, true));
                        push_type_children(&kind, &mut pending);
                    }
                }
                continue;
            }
            let rebuilt = rebuild_resolved_kind(interner, kind, &memo)?;
            memo.insert(original, rebuilt);
        }
        Ok(memo[&root])
    }

    pub fn finish(
        &self,
        interner: &mut TypeInterner,
        roots: impl IntoIterator<Item = TypeId>,
    ) -> Result<Vec<TypeId>, InferenceError> {
        roots
            .into_iter()
            .map(|root| self.resolve(interner, root))
            .collect()
    }
}

fn left_type(interner: &TypeInterner, inference: InferenceId) -> Result<TypeId, TypeError> {
    interner
        .by_kind
        .get(&TypeKind::Inference(inference))
        .copied()
        .ok_or(TypeError::UnresolvedInference(inference))
}

fn push_type_children(kind: &TypeKind, pending: &mut Vec<(TypeId, bool)>) {
    let mut push = |ty| pending.push((ty, false));
    match kind {
        TypeKind::Nominal { arguments, .. }
        | TypeKind::Tuple(arguments)
        | TypeKind::Union(arguments)
        | TypeKind::Intrinsic { arguments, .. }
        | TypeKind::Generated { arguments, .. } => {
            for argument in arguments.iter().rev() {
                push(*argument);
            }
        }
        TypeKind::Function(function) => {
            push(function.outcome);
            if let Some(variadic) = function.variadic {
                push(variadic);
            }
            for parameter in function.parameters.iter().rev() {
                push(parameter.ty);
            }
        }
        TypeKind::Option(item) => push(*item),
        TypeKind::Result { success, error } => {
            push(*error);
            push(*success);
        }
        TypeKind::Cursor { collection, .. } => push(*collection),
        TypeKind::Error
        | TypeKind::Scalar(_)
        | TypeKind::GenericParameter(_)
        | TypeKind::Inference(_)
        | TypeKind::OpaqueResult(_) => {}
    }
}

fn rebuild_resolved_kind(
    interner: &mut TypeInterner,
    kind: TypeKind,
    memo: &BTreeMap<TypeId, TypeId>,
) -> Result<TypeId, InferenceError> {
    let get = |ty: TypeId| {
        memo.get(&ty)
            .copied()
            .expect("all child types are resolved before their parent")
    };
    let ty = match kind {
        TypeKind::Error => interner.error(),
        TypeKind::Scalar(scalar) => interner.scalar(scalar),
        TypeKind::Nominal {
            identity,
            arguments,
        } => interner.nominal(identity, arguments.into_iter().map(get).collect())?,
        TypeKind::Tuple(items) => interner.tuple(items.into_iter().map(get).collect())?,
        TypeKind::Function(function) => interner.function(FunctionType::new(
            function.is_async,
            function.is_unsafe,
            function
                .parameters
                .into_iter()
                .map(|parameter| FunctionParameter::new(parameter.mode, get(parameter.ty)))
                .collect(),
            function.variadic.map(get),
            get(function.outcome),
        ))?,
        TypeKind::Option(item) => interner.option(get(item))?,
        TypeKind::Result { success, error } => interner.result(get(success), get(error))?,
        TypeKind::Union(members) => interner.union(members.into_iter().map(get))?,
        TypeKind::Intrinsic {
            constructor,
            arguments,
        } => interner.intrinsic(constructor, arguments.into_iter().map(get).collect())?,
        TypeKind::GenericParameter(position) => interner.generic_parameter(position)?,
        TypeKind::Inference(inference) => return Err(InferenceError::Unsolved(inference)),
        TypeKind::OpaqueResult(identity) => interner.opaque_result(identity)?,
        TypeKind::Generated {
            identity,
            arguments,
        } => interner.generated(identity, arguments.into_iter().map(get).collect())?,
        TypeKind::Cursor { mode, collection } => interner.cursor(mode, get(collection))?,
    };
    Ok(ty)
}

enum RenderTask {
    Type(TypeId, Precedence),
    Text(String),
}

fn push_application(output: &mut String, pending: &mut Vec<RenderTask>, arguments: &[TypeId]) {
    if arguments.is_empty() {
        return;
    }
    output.push('[');
    pending.push(RenderTask::Text("]".into()));
    push_render_sequence(pending, arguments, Precedence::Union, ", ");
}

fn push_render_sequence(
    pending: &mut Vec<RenderTask>,
    types: &[TypeId],
    minimum: Precedence,
    separator: &str,
) {
    for (index, ty) in types.iter().enumerate().rev() {
        pending.push(RenderTask::Type(*ty, minimum));
        if index > 0 {
            pending.push(RenderTask::Text(separator.into()));
        }
    }
}

fn push_render_items(pending: &mut Vec<RenderTask>, items: &[(String, TypeId, Precedence)]) {
    for (index, (prefix, ty, minimum)) in items.iter().enumerate().rev() {
        pending.push(RenderTask::Type(*ty, *minimum));
        pending.push(RenderTask::Text(prefix.clone()));
        if index > 0 {
            pending.push(RenderTask::Text(", ".into()));
        }
    }
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a UTF-8 string cannot fail")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Precedence {
    Union,
    Result,
    Optional,
    Primary,
}

fn precedence(kind: &TypeKind) -> Precedence {
    match kind {
        TypeKind::Union(_) => Precedence::Union,
        TypeKind::Result { .. } => Precedence::Result,
        TypeKind::Option(_) => Precedence::Optional,
        _ => Precedence::Primary,
    }
}

/// Classifies an explicitly written conversion between intrinsic numeric
/// scalars. `None` means that the pair is not part of the closed numeric table.
pub fn numeric_conversion(source: ScalarType, target: ScalarType) -> Option<NumericConversion> {
    if source == target {
        return numeric_shape(source).map(|_| NumericConversion::Identity);
    }
    match (numeric_shape(source)?, numeric_shape(target)?) {
        (NumericShape::Integer(source), NumericShape::Integer(target)) => {
            Some(if integer_range_contains(target, source) {
                NumericConversion::Total
            } else {
                NumericConversion::Checked
            })
        }
        (NumericShape::Integer(_), NumericShape::Float(_)) => Some(NumericConversion::Total),
        (NumericShape::Float(32), NumericShape::Float(64)) => Some(NumericConversion::Total),
        (NumericShape::Float(_), NumericShape::Float(_))
        | (NumericShape::Float(_), NumericShape::Integer(_)) => Some(NumericConversion::Checked),
    }
}

#[derive(Debug, Clone, Copy)]
enum NumericShape {
    Integer(IntegerShape),
    Float(u8),
}

#[derive(Debug, Clone, Copy)]
struct IntegerShape {
    signed: bool,
    bits: u8,
}

fn numeric_shape(scalar: ScalarType) -> Option<NumericShape> {
    let shape = match scalar {
        ScalarType::Byte | ScalarType::UInt8 => NumericShape::Integer(IntegerShape {
            signed: false,
            bits: 8,
        }),
        ScalarType::UInt16 => NumericShape::Integer(IntegerShape {
            signed: false,
            bits: 16,
        }),
        ScalarType::UInt32 => NumericShape::Integer(IntegerShape {
            signed: false,
            bits: 32,
        }),
        ScalarType::UInt64 => NumericShape::Integer(IntegerShape {
            signed: false,
            bits: 64,
        }),
        ScalarType::Int8 => NumericShape::Integer(IntegerShape {
            signed: true,
            bits: 8,
        }),
        ScalarType::Int16 => NumericShape::Integer(IntegerShape {
            signed: true,
            bits: 16,
        }),
        ScalarType::Int32 => NumericShape::Integer(IntegerShape {
            signed: true,
            bits: 32,
        }),
        ScalarType::Int => NumericShape::Integer(IntegerShape {
            signed: true,
            bits: 64,
        }),
        ScalarType::Float32 => NumericShape::Float(32),
        ScalarType::Float => NumericShape::Float(64),
        ScalarType::Bool
        | ScalarType::Char
        | ScalarType::String
        | ScalarType::Unit
        | ScalarType::Never => return None,
    };
    Some(shape)
}

fn integer_range_contains(target: IntegerShape, source: IntegerShape) -> bool {
    match (target.signed, source.signed) {
        (true, true) | (false, false) => target.bits >= source.bits,
        (true, false) => target.bits > source.bits,
        (false, true) => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::package::{
        DeclarationPath, Edition, Name, PackageAlias, PackageGraph, PackageId, PackageNode,
    };

    use super::*;

    fn graph() -> PackageGraph {
        PackageGraph::new(
            PackageId::new("pkg:app@1").unwrap(),
            PackageId::new("pkg:std@1").unwrap(),
            [
                PackageNode::new(
                    PackageId::new("pkg:app@1").unwrap(),
                    SourceId::new("pkg:app@1").unwrap(),
                    PackageAlias::new("app").unwrap(),
                    Edition::V0_1,
                    [ModulePath::new("models").unwrap()],
                    [],
                )
                .unwrap(),
                PackageNode::new(
                    PackageId::new("pkg:std@1").unwrap(),
                    SourceId::new("pkg:std@1").unwrap(),
                    PackageAlias::new("tondoStd").unwrap(),
                    Edition::V0_1,
                    [],
                    [],
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    fn symbol(namespace: Namespace, name: &str) -> SymbolIdentity {
        let graph = graph();
        let module = graph
            .module(
                &PackageId::new("pkg:app@1").unwrap(),
                &ModulePath::new("models").unwrap(),
            )
            .unwrap();
        graph
            .symbol_identity(
                module,
                namespace,
                DeclarationPath::single(Name::new(name).unwrap()),
            )
            .unwrap()
    }

    #[test]
    fn scalar_alias_spellings_share_one_canonical_type() {
        let interner = TypeInterner::default();
        assert_eq!(interner.named_scalar("Int"), interner.named_scalar("Int64"));
        assert_eq!(
            interner.named_scalar("Float"),
            interner.named_scalar("Float64")
        );
        assert_eq!(
            interner
                .canonical(interner.named_scalar("Int64").unwrap())
                .unwrap(),
            "Int"
        );
    }

    #[test]
    fn unions_flatten_deduplicate_sort_and_remove_never() {
        let mut interner = TypeInterner::default();
        let int = interner.scalar(ScalarType::Int);
        let string = interner.scalar(ScalarType::String);
        let never = interner.scalar(ScalarType::Never);
        let inner = interner.union([string, never, int]).unwrap();
        let outer = interner.union([string, inner, int, never]).unwrap();

        assert_eq!(inner, outer);
        assert_eq!(interner.canonical(outer).unwrap(), "Int | String");
        assert_eq!(interner.union([never]).unwrap(), never);
        assert_eq!(interner.union([int, never]).unwrap(), int);
    }

    #[test]
    fn option_result_and_function_forms_have_one_serialization() {
        let mut interner = TypeInterner::default();
        let int = interner.scalar(ScalarType::Int);
        let string = interner.scalar(ScalarType::String);
        let unit = interner.scalar(ScalarType::Unit);
        let optional = interner.option(int).unwrap();
        let nested_optional = interner.option(optional).unwrap();
        let errors = interner.union([string, int]).unwrap();
        let fallible_unit = interner.result(unit, errors).unwrap();
        let outcome = interner.result(optional, string).unwrap();
        let function = interner
            .function(FunctionType::new(
                true,
                true,
                vec![
                    FunctionParameter::new(ParameterMode::Ref, errors),
                    FunctionParameter::new(ParameterMode::Value, optional),
                ],
                Some(int),
                outcome,
            ))
            .unwrap();

        assert_eq!(interner.canonical(optional).unwrap(), "Int?");
        assert_eq!(interner.canonical(nested_optional).unwrap(), "(Int?)?");
        assert_eq!(
            interner.canonical(fallible_unit).unwrap(),
            "!(Int | String)"
        );
        assert_eq!(
            interner.canonical(function).unwrap(),
            "async unsafe fn(ref (Int | String), Int?, ...Int): Int? ! String"
        );
    }

    #[test]
    fn nominal_types_use_complete_identity_and_are_invariant() {
        let mut interner = TypeInterner::default();
        let int = interner.scalar(ScalarType::Int);
        let user = interner
            .nominal(symbol(Namespace::Type, "User"), vec![int])
            .unwrap();
        let same = interner
            .nominal(symbol(Namespace::Type, "User"), vec![int])
            .unwrap();
        let string = interner.scalar(ScalarType::String);
        let other_arguments = interner
            .nominal(symbol(Namespace::Type, "User"), vec![string])
            .unwrap();

        assert_eq!(user, same);
        assert_ne!(user, other_arguments);
        assert_eq!(
            interner.canonical(user).unwrap(),
            "@9:pkg:app@1::models::type::User[Int]"
        );
        assert!(matches!(
            interner.nominal(symbol(Namespace::Value, "makeUser"), vec![]),
            Err(TypeError::InvalidNominalNamespace(Namespace::Value))
        ));
    }

    #[test]
    fn inference_and_recovery_never_leak_into_canonical_diagnostics() {
        let mut interner = TypeInterner::default();
        let inference = interner.fresh_inference().unwrap();
        assert!(matches!(
            interner.canonical(inference),
            Err(TypeError::UnresolvedInference(_))
        ));
        assert_eq!(
            interner.canonical(interner.error()),
            Err(TypeError::RecoveryTypeHasNoCanonicalName)
        );
    }

    #[test]
    fn intrinsic_arity_and_type_node_budget_are_explicit() {
        let mut interner = TypeInterner::default();
        let int = interner.scalar(ScalarType::Int);
        assert!(matches!(
            interner.intrinsic(IntrinsicType::Map, vec![int]),
            Err(TypeError::InvalidIntrinsicArity { .. })
        ));
        let map = interner
            .intrinsic(IntrinsicType::Map, vec![int, int])
            .unwrap();
        assert_eq!(interner.canonical(map).unwrap(), "Map[Int, Int]");
        assert!(matches!(
            interner.intrinsic(IntrinsicType::Join, vec![int]),
            Err(TypeError::InvalidIntrinsicArity { .. })
        ));
        let join = interner
            .intrinsic(IntrinsicType::Join, vec![int, int])
            .unwrap();
        assert_eq!(interner.canonical(join).unwrap(), "Join[Int, Int]");

        assert!(matches!(
            TypeInterner::new(ScalarType::ALL.len() as u32),
            Err(TypeError::ResourceLimit { .. })
        ));
    }

    #[test]
    fn substitutions_rebuild_and_renormalize_complete_type_graphs() {
        let mut interner = TypeInterner::default();
        let first = interner.generic_parameter(0).unwrap();
        let second = interner.generic_parameter(1).unwrap();
        let optional = interner.option(first).unwrap();
        let alternatives = interner.union([first, second]).unwrap();
        let tuple = interner.tuple(vec![optional, alternatives]).unwrap();
        let function = interner
            .function(FunctionType::new(
                false,
                false,
                vec![FunctionParameter::new(ParameterMode::Ref, first)],
                None,
                tuple,
            ))
            .unwrap();
        let int = interner.scalar(ScalarType::Int);
        let substitution = TypeSubstitution::new(vec![int, int]);
        let substituted = substitution.apply(&mut interner, function).unwrap();

        assert_eq!(
            interner.canonical(substituted).unwrap(),
            "fn(ref Int): (Int?, Int)"
        );
        assert!(matches!(
            TypeSubstitution::new(vec![int]).apply(&mut interner, second),
            Err(TypeError::MissingGenericArgument {
                position: 1,
                arity: 1
            })
        ));
    }

    #[test]
    fn substitution_uses_an_explicit_worklist_for_deep_type_graphs() {
        let mut interner = TypeInterner::new(50_000).unwrap();
        let parameter = interner.generic_parameter(0).unwrap();
        let mut nested = parameter;
        for _ in 0..20_000 {
            nested = interner.option(nested).unwrap();
        }
        let int = interner.scalar(ScalarType::Int);
        let substituted = TypeSubstitution::new(vec![int])
            .apply(&mut interner, nested)
            .unwrap();

        let mut current = substituted;
        for _ in 0..20_000 {
            current = match interner.kind(current).unwrap() {
                TypeKind::Option(item) => *item,
                other => panic!("expected nested option, got {other:?}"),
            };
        }
        assert_eq!(current, int);
        let canonical = interner.canonical(substituted).unwrap();
        assert!(canonical.starts_with("(((("));
        assert!(canonical.ends_with("?)?)?)?"));
        assert_eq!(
            canonical.bytes().filter(|byte| *byte == b'?').count(),
            20_000
        );
    }

    #[test]
    fn assignability_is_exact_and_only_widens_top_level_constructors() {
        let mut interner = TypeInterner::default();
        let int = interner.scalar(ScalarType::Int);
        let string = interner.scalar(ScalarType::String);
        let never = interner.scalar(ScalarType::Never);
        let optional_int = interner.option(int).unwrap();
        let union = interner.union([int, string]).unwrap();
        let wider = interner.union([int, string, optional_int]).unwrap();
        let array_int = interner.intrinsic(IntrinsicType::Array, vec![int]).unwrap();
        let array_union = interner
            .intrinsic(IntrinsicType::Array, vec![union])
            .unwrap();

        assert_eq!(
            interner.assignability(int, int).unwrap(),
            Some(Assignability::Exact)
        );
        assert_eq!(
            interner.assignability(int, union).unwrap(),
            Some(Assignability::UnionInjection)
        );
        assert_eq!(
            interner.assignability(union, wider).unwrap(),
            Some(Assignability::UnionWidening)
        );
        assert_eq!(
            interner.assignability(int, optional_int).unwrap(),
            Some(Assignability::OptionLift)
        );
        assert_eq!(
            interner.assignability(never, array_int).unwrap(),
            Some(Assignability::Diverging)
        );
        assert_eq!(
            interner.assignability(array_int, array_union).unwrap(),
            None
        );
        assert!(interner.accepts_none(optional_int).unwrap());
        assert!(!interner.accepts_none(union).unwrap());
    }

    #[test]
    fn first_order_unification_respects_repeated_binders_and_occurs_checks() {
        let mut interner = TypeInterner::default();
        let parameter = interner.generic_parameter(0).unwrap();
        let int = interner.scalar(ScalarType::Int);
        let string = interner.scalar(ScalarType::String);
        let pair_pattern = interner.tuple(vec![parameter, parameter]).unwrap();
        let pair_int = interner.tuple(vec![int, int]).unwrap();
        let pair_mixed = interner.tuple(vec![int, string]).unwrap();
        let recursive = interner.option(parameter).unwrap();

        assert!(
            interner
                .first_order_unifiable(pair_pattern, pair_int)
                .unwrap()
        );
        assert!(
            !interner
                .first_order_unifiable(pair_pattern, pair_mixed)
                .unwrap()
        );
        assert!(
            !interner
                .first_order_unifiable(parameter, recursive)
                .unwrap()
        );
    }

    #[test]
    fn numeric_conversion_table_distinguishes_total_and_checked_pairs() {
        assert_eq!(
            numeric_conversion(ScalarType::UInt8, ScalarType::Byte),
            Some(NumericConversion::Total)
        );
        assert_eq!(
            numeric_conversion(ScalarType::Int8, ScalarType::Int16),
            Some(NumericConversion::Total)
        );
        assert_eq!(
            numeric_conversion(ScalarType::UInt32, ScalarType::Int32),
            Some(NumericConversion::Checked)
        );
        assert_eq!(
            numeric_conversion(ScalarType::UInt64, ScalarType::Float32),
            Some(NumericConversion::Total)
        );
        assert_eq!(
            numeric_conversion(ScalarType::Float32, ScalarType::Float),
            Some(NumericConversion::Total)
        );
        assert_eq!(
            numeric_conversion(ScalarType::Float, ScalarType::Float32),
            Some(NumericConversion::Checked)
        );
        assert_eq!(numeric_conversion(ScalarType::Bool, ScalarType::Int), None);
    }

    #[test]
    fn local_inference_propagates_expected_types_through_invariant_shapes() {
        let mut interner = TypeInterner::default();
        let mut inference = InferenceContext::new();
        let item = inference.fresh(&mut interner).unwrap();
        let inferred_array = interner
            .intrinsic(IntrinsicType::Array, vec![item])
            .unwrap();
        let int = interner.scalar(ScalarType::Int);
        let expected_array = interner.intrinsic(IntrinsicType::Array, vec![int]).unwrap();

        inference
            .equate(&interner, inferred_array, expected_array)
            .unwrap();
        assert_eq!(inference.resolve(&mut interner, item).unwrap(), int);
        assert_eq!(
            inference.resolve(&mut interner, inferred_array).unwrap(),
            expected_array
        );
    }

    #[test]
    fn local_inference_has_occurs_checks_rollback_and_rigid_generics() {
        let mut interner = TypeInterner::default();
        let mut inference = InferenceContext::new();
        let unknown = inference.fresh(&mut interner).unwrap();
        let recursive = interner.option(unknown).unwrap();
        assert!(matches!(
            inference.equate(&interner, unknown, recursive),
            Err(InferenceError::RecursiveSolution { .. })
        ));
        let unknown_id = match interner.kind(unknown).unwrap() {
            TypeKind::Inference(inference) => *inference,
            _ => unreachable!(),
        };
        assert_eq!(inference.solution(unknown_id), None);

        let int = interner.scalar(ScalarType::Int);
        let string = interner.scalar(ScalarType::String);
        assert!(matches!(
            inference.equate(&interner, int, string),
            Err(InferenceError::Mismatch { .. })
        ));
        inference.equate(&interner, unknown, int).unwrap();
        assert_eq!(inference.resolve(&mut interner, unknown).unwrap(), int);

        let first = interner.generic_parameter(0).unwrap();
        let second = interner.generic_parameter(1).unwrap();
        assert!(matches!(
            inference.equate(&interner, first, second),
            Err(InferenceError::Mismatch { .. })
        ));
    }

    #[test]
    fn finishing_inference_rejects_unsolved_variables_and_rebuilds_functions() {
        let mut interner = TypeInterner::default();
        let mut inference = InferenceContext::new();
        let parameter = inference.fresh(&mut interner).unwrap();
        let outcome = inference.fresh(&mut interner).unwrap();
        let inferred = interner
            .function(FunctionType::new(
                false,
                false,
                vec![FunctionParameter::new(ParameterMode::Value, parameter)],
                None,
                outcome,
            ))
            .unwrap();
        assert!(matches!(
            inference.resolve(&mut interner, inferred),
            Err(InferenceError::Unsolved(_))
        ));

        let int = interner.scalar(ScalarType::Int);
        let string = interner.scalar(ScalarType::String);
        inference.equate(&interner, parameter, int).unwrap();
        inference.equate(&interner, outcome, string).unwrap();
        let resolved = inference.resolve(&mut interner, inferred).unwrap();
        assert_eq!(interner.canonical(resolved).unwrap(), "fn(Int): String");
    }

    #[test]
    fn generated_and_cursor_names_are_location_stable() {
        let mut interner = TypeInterner::default();
        let int = interner.scalar(ScalarType::Int);
        let generated = interner
            .generated(
                GeneratedTypeIdentity::new(
                    GeneratedTypeKind::Closure,
                    SourceId::new("source:app").unwrap(),
                    ModulePath::new("main").unwrap(),
                    LogicalPath::new("src/main.to").unwrap(),
                    42,
                ),
                vec![int],
            )
            .unwrap();
        let cursor = interner.cursor(CursorMode::Ref, generated).unwrap();

        assert_eq!(
            interner.canonical(generated).unwrap(),
            "generated[\"closure\",\"source:app\",\"main\",\"src/main.to\",42][Int]"
        );
        assert_eq!(
            interner.canonical(cursor).unwrap(),
            "cursor[ref,generated[\"closure\",\"source:app\",\"main\",\"src/main.to\",42][Int]]"
        );
    }
}
