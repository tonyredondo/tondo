//! In-memory, typed bytecode shared by the compiler and the bootstrap VM.
//!
//! This representation deliberately has no serializer and is not an ABI. Its
//! indices are request-local, every executable value lives in an explicit
//! frame slot, and all control-flow targets remain visible to verification.

mod disassemble;
mod verify;

pub use disassemble::disassemble;
pub(crate) use verify::verify_bytecode_with_trace_metadata;
pub use verify::{
    BytecodeVerificationError, BytecodeVerificationLimits, derive_copy_capabilities,
    derive_discard_capabilities, derive_terminal_statuses, derive_trace_metadata, verify_bytecode,
    verify_bytecode_with_limits,
};

macro_rules! index_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            pub const fn new(index: u32) -> Self {
                Self(index)
            }

            pub const fn index(self) -> u32 {
                self.0
            }
        }
    };
}

index_type!(BytecodeTypeId);
index_type!(BytecodeNominalId);
index_type!(BytecodeCallableId);
index_type!(BytecodeFunctionId);
index_type!(BytecodeConstantId);
index_type!(BytecodeSlotId);
index_type!(BytecodeLoanId);
index_type!(BytecodeBlockId);
index_type!(BytecodeSpanId);
index_type!(BytecodeScopeId);

/// Normalizes one Tondo array index without a signed intermediate addition.
///
/// Nonnegative indices are used directly. A negative index denotes its
/// absolute distance from the end, so `-1` selects the final element and
/// `-length` selects the first. Every value outside the array returns `None`,
/// including the minimum signed integer.
pub fn normalize_array_index(index: i128, length: usize) -> Option<usize> {
    let normalized = if index < 0 {
        let distance = index
            .checked_neg()
            .and_then(|value| usize::try_from(value).ok())?;
        length.checked_sub(distance)?
    } else {
        usize::try_from(index).ok()?
    };
    (normalized < length).then_some(normalized)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArraySliceError {
    ZeroStep,
    LengthNotRepresentable,
}

/// Returns the exact ordered indices selected by one Tondo array slice.
///
/// Explicit negative bounds are offset from the end before clamping. Omitted
/// bounds retain their sign-dependent sentinels, which deliberately makes
/// `[::-1]` different from `[:-1:-1]`. Progress checks avoid overflowing even
/// for the minimum signed step.
pub fn normalize_array_slice_indices(
    start: Option<i128>,
    end: Option<i128>,
    step: Option<i128>,
    length: usize,
) -> Result<Vec<usize>, ArraySliceError> {
    let step = step.unwrap_or(1);
    if step == 0 {
        return Err(ArraySliceError::ZeroStep);
    }
    let length = i64::try_from(length)
        .map(i128::from)
        .map_err(|_| ArraySliceError::LengthNotRepresentable)?;
    let explicit_bound = |value: i128, minimum: i128, maximum: i128| {
        let offset = if value < 0 { length + value } else { value };
        offset.clamp(minimum, maximum)
    };

    let mut output = Vec::new();
    if step > 0 {
        let mut index = start.map_or(0, |value| explicit_bound(value, 0, length));
        let end = end.map_or(length, |value| explicit_bound(value, 0, length));
        while index < end {
            output.push(index as usize);
            if step >= end - index {
                break;
            }
            index += step;
        }
    } else {
        let maximum = length - 1;
        let mut index = start.map_or(maximum, |value| explicit_bound(value, -1, maximum));
        let end = end.map_or(-1, |value| explicit_bound(value, -1, maximum));
        while index > end {
            output.push(index as usize);
            if step.unsigned_abs() >= (index - end) as u128 {
                break;
            }
            index += step;
        }
    }
    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BytecodeSpan {
    pub file: u32,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeProgram {
    pub types: Vec<BytecodeType>,
    pub nominals: Vec<BytecodeNominal>,
    pub callables: Vec<BytecodeCallable>,
    pub constants: Vec<BytecodeNamedConstant>,
    pub functions: Vec<BytecodeFunction>,
}

impl BytecodeProgram {
    pub fn function(&self, id: BytecodeFunctionId) -> Option<&BytecodeFunction> {
        self.functions.get(id.index() as usize)
    }

    pub fn callable(&self, id: BytecodeCallableId) -> Option<&BytecodeCallable> {
        self.callables.get(id.index() as usize)
    }

    pub fn ty(&self, id: BytecodeTypeId) -> Option<&BytecodeType> {
        self.types.get(id.index() as usize)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeType {
    pub name: String,
    pub kind: BytecodeTypeKind,
}

/// Source-visible closed capabilities retained by an opaque result.
///
/// The concrete witness remains available to the verifier for representation
/// checks, but it must not strengthen the contract visible through the opaque
/// type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BytecodeCapabilitySet {
    pub copy: bool,
    pub discard: bool,
    pub equatable: bool,
    pub key: bool,
    pub send: bool,
    pub share: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeTypeKind {
    Scalar(BytecodeScalarType),
    Nominal {
        nominal: Option<BytecodeNominalId>,
        identity: String,
        arguments: Vec<BytecodeTypeId>,
    },
    Tuple(Vec<BytecodeTypeId>),
    Function(BytecodeFunctionType),
    Option(BytecodeTypeId),
    Result {
        success: BytecodeTypeId,
        error: BytecodeTypeId,
    },
    Union(Vec<BytecodeTypeId>),
    Intrinsic {
        constructor: BytecodeIntrinsicType,
        arguments: Vec<BytecodeTypeId>,
    },
    GenericParameter(u32),
    OpaqueResult {
        identity: String,
        arguments: Vec<BytecodeTypeId>,
        witness: BytecodeTypeId,
        capabilities: BytecodeCapabilitySet,
    },
    Generated {
        identity: String,
        arguments: Vec<BytecodeTypeId>,
    },
    Cursor {
        mode: BytecodeCursorMode,
        collection: BytecodeTypeId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BytecodeScalarType {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BytecodeIntrinsicType {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BytecodeTerminalStatus {
    Absent,
    Potential,
    Present,
}

/// Closed description of the managed edges in one bytecode type.
///
/// The bootstrap VM derives this independently from the verified catalog and
/// attaches the corresponding type ID to every heap allocation. Template field
/// types retain the nominal arguments needed to interpret generic layouts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeTraceDescriptor {
    Inline,
    String,
    Tuple {
        fields: Vec<BytecodeTypeId>,
    },
    Array {
        element: BytecodeTypeId,
    },
    Map {
        key: BytecodeTypeId,
        value: BytecodeTypeId,
    },
    Set {
        element: BytecodeTypeId,
    },
    Closure {
        callable: BytecodeCallableId,
        captures: Vec<BytecodeTypeId>,
    },
    Newtype {
        nominal: BytecodeNominalId,
        arguments: Vec<BytecodeTypeId>,
        value: BytecodeTypeId,
    },
    Record {
        nominal: BytecodeNominalId,
        arguments: Vec<BytecodeTypeId>,
        fields: Vec<BytecodeField>,
    },
    Variant {
        nominal: Option<BytecodeNominalId>,
        arguments: Vec<BytecodeTypeId>,
        variants: Vec<BytecodeVariant>,
    },
    Option {
        value: BytecodeTypeId,
    },
    Result {
        success: BytecodeTypeId,
        error: BytecodeTypeId,
    },
    Union {
        members: Vec<BytecodeTypeId>,
    },
    Range {
        element: BytecodeTypeId,
    },
    Ref {
        value: BytecodeTypeId,
    },
    Cursor {
        mode: BytecodeCursorMode,
        collection: BytecodeTypeId,
    },
}

/// Trace roots carried by one active or suspended frame of a function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeFrameTraceDescriptor {
    pub function: BytecodeFunctionId,
    pub slots: Vec<BytecodeTypeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeTraceMetadata {
    pub types: Vec<BytecodeTraceDescriptor>,
    pub frames: Vec<BytecodeFrameTraceDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeTerminalOperation {
    JoinAwait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeTerminalUnwindAction {
    JoinTeardown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeTerminalContract {
    pub operation: BytecodeTerminalOperation,
    pub unwind: BytecodeTerminalUnwindAction,
    pub unwind_may_suspend: bool,
}

impl BytecodeIntrinsicType {
    pub const fn arity(self) -> usize {
        match self {
            Self::Map | Self::Join => 2,
            Self::Array | Self::Set | Self::Range | Self::Ref | Self::Pointer => 1,
            Self::Command | Self::Pipeline | Self::NumericConversionError => 0,
        }
    }

    /// Returns the sealed language contract for a direct terminal root.
    ///
    /// Structural containers are deliberately absent here: their terminal
    /// status is derived from the values they own.
    pub const fn terminal_contract(self) -> Option<BytecodeTerminalContract> {
        match self {
            Self::Join => Some(BytecodeTerminalContract {
                operation: BytecodeTerminalOperation::JoinAwait,
                unwind: BytecodeTerminalUnwindAction::JoinTeardown,
                unwind_may_suspend: true,
            }),
            Self::Array
            | Self::Map
            | Self::Set
            | Self::Range
            | Self::Ref
            | Self::Pointer
            | Self::Command
            | Self::Pipeline
            | Self::NumericConversionError => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BytecodeCursorMode {
    Own,
    Ref,
    Mut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeFunctionType {
    pub is_async: bool,
    pub is_unsafe: bool,
    pub parameters: Vec<BytecodeFunctionParameter>,
    pub variadic: Option<BytecodeTypeId>,
    pub outcome: BytecodeTypeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeFunctionParameter {
    pub mode: BytecodeParameterMode,
    pub ty: BytecodeTypeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BytecodeParameterMode {
    Value,
    Ref,
    Mut,
    Var,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeNominal {
    pub name: String,
    pub identity: String,
    pub generic_arity: u32,
    pub shape: BytecodeNominalShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeNominalShape {
    Newtype { underlying: BytecodeTypeId },
    Record { fields: Vec<BytecodeField> },
    Enum { variants: Vec<BytecodeVariant> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeField {
    pub member: u32,
    pub ty: BytecodeTypeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeVariant {
    pub member: u32,
    pub payload: BytecodeVariantPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeVariantPayload {
    Unit,
    Tuple(Vec<BytecodeTypeId>),
    Record(Vec<BytecodeField>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeCallable {
    pub name: String,
    pub generic_arity: u32,
    pub parameters: Vec<BytecodeParameter>,
    pub outcome: BytecodeTypeId,
    pub function_type: BytecodeTypeId,
    pub implementation: Option<BytecodeFunctionId>,
    pub closure: Option<BytecodeClosure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeClosure {
    pub environment: BytecodeTypeId,
    pub captures: Vec<BytecodeTypeId>,
    pub protocols: BytecodeClosureProtocols,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeClosureProtocols {
    pub call: bool,
    pub call_mut: bool,
    pub call_once: bool,
}

impl BytecodeClosureProtocols {
    pub const fn supports(self, protocol: BytecodeCallProtocol) -> bool {
        match protocol {
            BytecodeCallProtocol::Call => self.call,
            BytecodeCallProtocol::CallMut => self.call_mut,
            BytecodeCallProtocol::CallOnce => self.call_once,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeParameter {
    pub mode: BytecodeParameterMode,
    pub ty: BytecodeTypeId,
    pub variadic_element: Option<BytecodeTypeId>,
    pub receiver: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeNamedConstant {
    pub name: String,
    pub value: BytecodeConstantValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeConstantValue {
    pub ty: BytecodeTypeId,
    pub kind: BytecodeConstantValueKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeConstantValueKind {
    Unit,
    Bool(bool),
    Integer(i128),
    Float(u64),
    Char(char),
    String(String),
    Function {
        callable: BytecodeCallableId,
        arguments: Vec<BytecodeTypeId>,
    },
    Tuple(Vec<BytecodeConstantValue>),
    Array(Vec<BytecodeConstantValue>),
    Map(Vec<(BytecodeConstantValue, BytecodeConstantValue)>),
    Set(Vec<BytecodeConstantValue>),
    Newtype {
        nominal: BytecodeNominalId,
        value: Box<BytecodeConstantValue>,
    },
    Record {
        nominal: BytecodeNominalId,
        fields: Vec<(u32, BytecodeConstantValue)>,
    },
    Variant {
        variant: u32,
        payload: BytecodeConstantVariantValue,
    },
    OptionNone,
    OptionSome(Box<BytecodeConstantValue>),
    ResultOk(Box<BytecodeConstantValue>),
    ResultErr(Box<BytecodeConstantValue>),
    Range {
        kind: BytecodeRangeKind,
        start: Box<BytecodeConstantValue>,
        end: Box<BytecodeConstantValue>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeConstantVariantValue {
    Unit,
    Tuple(Vec<BytecodeConstantValue>),
    Record(Vec<(u32, BytecodeConstantValue)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeFunction {
    pub callable: BytecodeCallableId,
    pub source: BytecodeSpan,
    pub types: Vec<BytecodeTypeId>,
    pub spans: Vec<BytecodeSpan>,
    pub slots: Vec<BytecodeSlot>,
    pub loans: Vec<BytecodeLoan>,
    pub parameters: Vec<BytecodeSlotId>,
    pub return_slot: BytecodeSlotId,
    pub entry: BytecodeBlockId,
    pub unwind: BytecodeBlockId,
    pub blocks: Vec<BytecodeBlock>,
}

impl BytecodeFunction {
    pub fn block(&self, id: BytecodeBlockId) -> Option<&BytecodeBlock> {
        self.blocks.get(id.index() as usize)
    }

    pub fn slot(&self, id: BytecodeSlotId) -> Option<&BytecodeSlot> {
        self.slots.get(id.index() as usize)
    }

    pub fn span(&self, id: BytecodeSpanId) -> Option<BytecodeSpan> {
        self.spans.get(id.index() as usize).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeSlot {
    pub ty: BytecodeTypeId,
    pub span: BytecodeSpanId,
    pub kind: BytecodeSlotKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeSlotKind {
    Return,
    Parameter { index: u32 },
    User { local: u32 },
    Temporary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeBlock {
    pub kind: BytecodeBlockKind,
    pub instructions: Vec<BytecodeInstruction>,
    pub terminator: BytecodeTerminator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeBlockKind {
    Normal,
    Cleanup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeInstruction {
    pub span: BytecodeSpanId,
    pub kind: BytecodeInstructionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeInstructionKind {
    StorageLive(BytecodeSlotId),
    StorageDead(BytecodeSlotId),
    ReserveLoan(BytecodeLoanId),
    ReleaseLoan(BytecodeLoanId),
    Store {
        destination: BytecodePlace,
        value: BytecodeRvalue,
    },
    RegisterDefer {
        scope: BytecodeScopeId,
        action: BytecodeOperation,
        guard: Option<BytecodePlace>,
    },
    RegisterFallback {
        scope: BytecodeScopeId,
        owner: BytecodePlace,
    },
    RetargetCleanup {
        from: BytecodePlace,
        to: BytecodePlace,
    },
    DisarmCleanup(BytecodePlace),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodePlace {
    pub slot: BytecodeSlotId,
    pub ty: BytecodeTypeId,
    pub projections: Vec<BytecodeProjection>,
    pub source_loan: Option<BytecodeLoanId>,
}

impl BytecodePlace {
    pub(crate) fn is_structurally_replaceable(&self) -> bool {
        matches!(
            self.projections.last().map(|projection| &projection.kind),
            Some(
                BytecodeProjectionKind::ClosureCapture { .. }
                    | BytecodeProjectionKind::Field(_)
                    | BytecodeProjectionKind::TupleField(_)
                    | BytecodeProjectionKind::NewtypeValue
                    | BytecodeProjectionKind::VariantTuple { .. }
                    | BytecodeProjectionKind::VariantField { .. }
                    | BytecodeProjectionKind::OptionValue
                    | BytecodeProjectionKind::ResultOkValue
                    | BytecodeProjectionKind::ResultErrValue
                    | BytecodeProjectionKind::UnionValue(_)
                    | BytecodeProjectionKind::ArrayPatternIndex(_)
                    | BytecodeProjectionKind::IteratorElement { .. }
                    | BytecodeProjectionKind::Index {
                        access: BytecodeIndexAccess::Array,
                        ..
                    }
            )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeLoanKind {
    CallLocal,
    Region,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeLoan {
    pub kind: BytecodeLoanKind,
    pub mode: BytecodeParameterMode,
    pub place: BytecodePlace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeProjection {
    pub ty: BytecodeTypeId,
    pub kind: BytecodeProjectionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeProjectionKind {
    ClosureCapture {
        callable: BytecodeCallableId,
        index: u32,
    },
    Field(u32),
    TupleField(u32),
    NewtypeValue,
    RefValue,
    VariantTuple {
        variant: u32,
        index: u32,
    },
    VariantField {
        variant: u32,
        field: u32,
    },
    OptionValue,
    ResultOkValue,
    ResultErrValue,
    UnionValue(BytecodeTypeId),
    ArrayPatternIndex(u32),
    ArrayPatternRest {
        start: u32,
        suffix: u32,
    },
    IteratorElement {
        index: BytecodeSlotId,
    },
    IteratorSource,
    Index {
        index: BytecodeSlotId,
        access: BytecodeIndexAccess,
    },
    Slice {
        start: Option<BytecodeSlotId>,
        end: Option<BytecodeSlotId>,
        step: Option<BytecodeSlotId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeOperand {
    pub ty: BytecodeTypeId,
    pub kind: BytecodeOperandKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeOperandKind {
    Constant(BytecodeConstant),
    Copy(BytecodePlace),
    Move(BytecodePlace),
    Borrow(BytecodePlace),
    Loan(BytecodeLoanId),
    Function {
        callable: BytecodeCallableId,
        arguments: Vec<BytecodeTypeId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeConstant {
    Unit,
    Bool(bool),
    Integer(String),
    Float(String),
    Char(String),
    String(String),
    Named(BytecodeConstantId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeRvalue {
    pub ty: BytecodeTypeId,
    pub kind: BytecodeRvalueKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeRvalueKind {
    Use(BytecodeOperand),
    Prefix {
        operator: BytecodePrefixOperator,
        operand: BytecodeOperand,
    },
    Binary {
        operator: BytecodeBinaryOperator,
        left: BytecodeOperand,
        right: BytecodeOperand,
    },
    Construct {
        shape: BytecodeAggregateKind,
        values: Vec<BytecodeOperand>,
    },
    RecordUpdate {
        base: BytecodeOperand,
        fields: Vec<(u32, BytecodeOperand)>,
    },
    Coerce {
        kind: BytecodeCoercion,
        value: BytecodeOperand,
    },
    NumericConversion {
        target: BytecodeScalarType,
        conversion: BytecodeNumericConversion,
        value: BytecodeOperand,
    },
    Range {
        kind: BytecodeRangeKind,
        start: BytecodeOperand,
        end: BytecodeOperand,
    },
    Contains {
        kind: BytecodeContainmentKind,
        item: BytecodeOperand,
        container: BytecodeOperand,
    },
    MapRemove {
        map: BytecodePlace,
        key: BytecodeOperand,
    },
    Interpolate {
        segments: Vec<String>,
        values: Vec<BytecodeOperand>,
    },
    Length(BytecodeOperand),
    IteratorState(BytecodeOperand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeAggregateKind {
    Tuple,
    Array,
    Set,
    Closure {
        callable: BytecodeCallableId,
        captures: Vec<BytecodeTypeId>,
    },
    Newtype {
        nominal: BytecodeNominalId,
    },
    Ref,
    Record {
        nominal: BytecodeNominalId,
        fields: Vec<u32>,
    },
    Variant {
        variant: u32,
        fields: Vec<Option<u32>>,
    },
    OptionNone,
    OptionSome,
    ResultOk,
    ResultErr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeCoercion {
    Exact,
    Opaque,
    CallableErasure,
    UnionInjection,
    UnionWidening,
    OptionLift,
    Diverging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeNumericConversion {
    Identity,
    Total,
    Checked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BytecodeNumericConversionError {
    OutOfRange,
    NotFinite,
    NotIntegral,
}

impl BytecodeNumericConversionError {
    pub const ALL: [Self; 3] = [Self::OutOfRange, Self::NotFinite, Self::NotIntegral];

    pub const fn index(self) -> u32 {
        match self {
            Self::OutOfRange => 0,
            Self::NotFinite => 1,
            Self::NotIntegral => 2,
        }
    }

    pub fn from_index(index: u32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|variant| variant.index() == index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodePrefixOperator {
    Negate,
    LogicalNot,
    BitwiseNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeBinaryOperator {
    Multiply,
    Divide,
    Remainder,
    Add,
    Subtract,
    ShiftLeft,
    ShiftRight,
    BitwiseAnd,
    BitwiseXor,
    BitwiseOr,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    LogicalAnd,
    LogicalOr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeRangeKind {
    Exclusive,
    Inclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeContainmentKind {
    Array,
    MapKey,
    Set,
    Range,
    StringChar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeArraySequenceKind {
    Concat,
    Repeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BytecodeIndexAccess {
    Array,
    String,
    MapLookup,
    MapEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeOperation {
    pub ty: BytecodeTypeId,
    pub kind: BytecodeOperationKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeOperationKind {
    CheckedPrefix {
        operator: BytecodePrefixOperator,
        operand: BytecodeOperand,
    },
    CheckedBinary {
        operator: BytecodeBinaryOperator,
        left: BytecodeOperand,
        right: BytecodeOperand,
    },
    ArraySequence {
        kind: BytecodeArraySequenceKind,
        array: BytecodeOperand,
        argument: BytecodeOperand,
    },
    BuildMap {
        entries: Vec<(BytecodeOperand, BytecodeOperand)>,
        reject_dynamic_duplicates: bool,
    },
    Index {
        base: BytecodeOperand,
        index: BytecodeOperand,
        access: BytecodeIndexAccess,
        against: Vec<BytecodeLoanId>,
    },
    Slice {
        base: BytecodeOperand,
        bounds: Box<BytecodeSliceBounds>,
        against: Vec<BytecodeLoanId>,
    },
    Call {
        callee: BytecodeOperand,
        arguments: Vec<BytecodeCallArgument>,
        signature: BytecodeTypeId,
        protocol: BytecodeCallProtocol,
    },
    Display {
        argument: BytecodeCallArgument,
    },
    ExplicitPanic {
        message: BytecodeOperand,
    },
    Assert {
        condition: BytecodeOperand,
        condition_repr: String,
        message_parts: Vec<BytecodeAssertMessagePart>,
    },
    BootstrapHostCall {
        function: BytecodeBootstrapHostFunction,
        arguments: Vec<BytecodeOperand>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeSliceBounds {
    pub start: Option<BytecodeOperand>,
    pub end: Option<BytecodeOperand>,
    pub step: Option<BytecodeOperand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeCallProtocol {
    Call,
    CallMut,
    CallOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeBootstrapHostFunction {
    ConsolePrint,
}

impl BytecodeBootstrapHostFunction {
    pub const fn name(self) -> &'static str {
        match self {
            Self::ConsolePrint => "std.console.print",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeAssertMessagePart {
    pub value: BytecodeOperand,
    pub spread: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeCallArgument {
    pub mode: BytecodeParameterMode,
    pub target: BytecodeCallArgumentTarget,
    pub value: BytecodeOperand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytecodeCallArgumentTarget {
    Receiver,
    Fixed(u32),
    VariadicElement,
    VariadicSpread,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeTerminator {
    pub span: BytecodeSpanId,
    pub kind: BytecodeTerminatorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeTerminatorKind {
    Goto {
        target: BytecodeBlockId,
    },
    BranchBool {
        condition: BytecodeOperand,
        if_true: BytecodeBlockId,
        if_false: BytecodeBlockId,
    },
    BranchTag {
        value: BytecodeOperand,
        cases: Vec<(BytecodeTag, BytecodeBlockId)>,
        otherwise: BytecodeBlockId,
    },
    Invoke {
        operation: BytecodeOperation,
        destination: Option<BytecodePlace>,
        target: Option<BytecodeBlockId>,
        unwind: BytecodeBlockId,
    },
    IteratorNext {
        state: BytecodePlace,
        destination: BytecodePlace,
        borrowed_source: Option<BytecodePlace>,
        exhaustion_guard: Option<BytecodePlace>,
        has_value: BytecodeBlockId,
        exhausted: BytecodeBlockId,
        unwind: BytecodeBlockId,
    },
    ValidatePlaces {
        places: Vec<BytecodePlace>,
        replacements: Vec<Option<BytecodeOperand>>,
        against: Vec<Vec<BytecodeLoanId>>,
        for_write: bool,
        target: BytecodeBlockId,
        unwind: BytecodeBlockId,
    },
    ValidateLoan {
        loan: BytecodeLoanId,
        against: Vec<BytecodeLoanId>,
        target: BytecodeBlockId,
        unwind: BytecodeBlockId,
    },
    DrainDefers {
        scopes: Vec<BytecodeScopeId>,
        target: BytecodeBlockId,
        unwind: BytecodeBlockId,
    },
    DrainUnwind {
        target: BytecodeBlockId,
    },
    Return,
    ResumePanic,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BytecodeTag {
    OptionNone,
    OptionSome,
    ResultOk,
    ResultErr,
    Variant(u32),
    Union(BytecodeTypeId),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projected_place(kind: BytecodeProjectionKind) -> BytecodePlace {
        let ty = BytecodeTypeId::new(0);
        BytecodePlace {
            slot: BytecodeSlotId::new(0),
            ty,
            projections: vec![BytecodeProjection { ty, kind }],
            source_loan: None,
        }
    }

    #[test]
    fn indices_are_explicit_and_never_cross_kinds_by_type() {
        let slot = BytecodeSlotId::new(7);
        let block = BytecodeBlockId::new(7);
        assert_eq!(slot.index(), block.index());
        assert_ne!(format!("{slot:?}"), format!("{block:?}"));
    }

    #[test]
    fn intrinsic_arities_are_closed() {
        assert_eq!(BytecodeIntrinsicType::Map.arity(), 2);
        assert_eq!(BytecodeIntrinsicType::Array.arity(), 1);
        assert_eq!(BytecodeIntrinsicType::Command.arity(), 0);
    }

    #[test]
    fn array_index_normalization_is_mathematical_and_closed() {
        assert_eq!(normalize_array_index(0, 4), Some(0));
        assert_eq!(normalize_array_index(3, 4), Some(3));
        assert_eq!(normalize_array_index(-1, 4), Some(3));
        assert_eq!(normalize_array_index(-4, 4), Some(0));

        assert_eq!(normalize_array_index(4, 4), None);
        assert_eq!(normalize_array_index(-5, 4), None);
        assert_eq!(normalize_array_index(0, 0), None);
        assert_eq!(normalize_array_index(-1, 0), None);
        assert_eq!(normalize_array_index(i64::MAX as i128, 4), None);
        assert_eq!(normalize_array_index(i64::MIN as i128, 4), None);
        assert_eq!(normalize_array_index(i128::MIN, 4), None);
    }

    #[test]
    fn array_slice_normalization_preserves_defaults_clamping_and_extremes() {
        let indices = |start, end, step, length| {
            normalize_array_slice_indices(start, end, step, length).unwrap()
        };

        assert_eq!(indices(None, None, None, 5), [0, 1, 2, 3, 4]);
        assert_eq!(indices(Some(1), Some(4), None, 5), [1, 2, 3]);
        assert_eq!(indices(Some(-100), Some(100), None, 5), [0, 1, 2, 3, 4]);
        assert_eq!(indices(None, None, Some(2), 5), [0, 2, 4]);
        assert_eq!(indices(None, None, Some(-1), 5), [4, 3, 2, 1, 0]);
        assert_eq!(indices(None, Some(-1), Some(-1), 5), []);
        assert_eq!(indices(Some(4), Some(0), Some(-2), 5), [4, 2]);
        assert_eq!(indices(Some(-1), Some(-6), Some(-2), 5), [4, 2, 0]);
        assert_eq!(
            indices(Some(i64::MIN as i128), Some(i64::MAX as i128), None, 5,),
            [0, 1, 2, 3, 4]
        );
        assert_eq!(
            indices(Some(i64::MAX as i128), Some(i64::MIN as i128), Some(-1), 5,),
            [4, 3, 2, 1, 0]
        );
        assert_eq!(indices(None, None, Some(i64::MIN as i128), 5), [4]);
        assert_eq!(indices(None, None, None, 0), []);
        assert_eq!(indices(None, None, Some(-1), 0), []);
        assert_eq!(
            normalize_array_slice_indices(None, None, Some(0), 5),
            Err(ArraySliceError::ZeroStep)
        );
        #[cfg(target_pointer_width = "64")]
        assert_eq!(
            normalize_array_slice_indices(None, None, None, usize::MAX),
            Err(ArraySliceError::LengthNotRepresentable)
        );
    }

    #[test]
    fn intrinsic_terminal_registry_is_sealed_to_join() {
        let all = [
            BytecodeIntrinsicType::Array,
            BytecodeIntrinsicType::Map,
            BytecodeIntrinsicType::Set,
            BytecodeIntrinsicType::Range,
            BytecodeIntrinsicType::Ref,
            BytecodeIntrinsicType::Pointer,
            BytecodeIntrinsicType::Join,
            BytecodeIntrinsicType::Command,
            BytecodeIntrinsicType::Pipeline,
            BytecodeIntrinsicType::NumericConversionError,
        ];
        let registered = all
            .into_iter()
            .filter_map(BytecodeIntrinsicType::terminal_contract)
            .collect::<Vec<_>>();
        assert_eq!(
            registered,
            [BytecodeTerminalContract {
                operation: BytecodeTerminalOperation::JoinAwait,
                unwind: BytecodeTerminalUnwindAction::JoinTeardown,
                unwind_may_suspend: true,
            }]
        );
    }

    #[test]
    fn structural_reborrows_require_a_complete_strict_subplace() {
        assert!(
            projected_place(BytecodeProjectionKind::TupleField(0)).is_structurally_replaceable()
        );
        assert!(
            projected_place(BytecodeProjectionKind::Index {
                index: BytecodeSlotId::new(1),
                access: BytecodeIndexAccess::Array,
            })
            .is_structurally_replaceable()
        );
        assert!(
            !projected_place(BytecodeProjectionKind::Slice {
                start: None,
                end: None,
                step: None,
            })
            .is_structurally_replaceable()
        );
        assert!(
            !projected_place(BytecodeProjectionKind::ArrayPatternRest {
                start: 0,
                suffix: 0,
            })
            .is_structurally_replaceable()
        );
        assert!(
            !projected_place(BytecodeProjectionKind::Index {
                index: BytecodeSlotId::new(1),
                access: BytecodeIndexAccess::MapEntry,
            })
            .is_structurally_replaceable()
        );
    }
}
