//! In-memory, typed bytecode shared by the compiler and the bootstrap VM.
//!
//! This representation deliberately has no serializer and is not an ABI. Its
//! indices are request-local, every executable value lives in an explicit
//! frame slot, and all control-flow targets remain visible to verification.

mod disassemble;
mod verify;

pub use disassemble::disassemble;
pub use verify::{
    BytecodeVerificationError, BytecodeVerificationLimits, derive_discard_capabilities,
    verify_bytecode, verify_bytecode_with_limits,
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

impl BytecodeIntrinsicType {
    pub const fn arity(self) -> usize {
        match self {
            Self::Map | Self::Join => 2,
            Self::Array | Self::Set | Self::Range | Self::Ref | Self::Pointer => 1,
            Self::Command | Self::Pipeline | Self::NumericConversionError => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BytecodeCursorMode {
    Own,
    Ref,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodePlace {
    pub slot: BytecodeSlotId,
    pub ty: BytecodeTypeId,
    pub projections: Vec<BytecodeProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeLoan {
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
pub enum BytecodeIndexAccess {
    Array,
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
    BuildMap {
        entries: Vec<(BytecodeOperand, BytecodeOperand)>,
        reject_dynamic_duplicates: bool,
    },
    Index {
        base: BytecodeOperand,
        index: BytecodeOperand,
        access: BytecodeIndexAccess,
    },
    Slice {
        base: BytecodeOperand,
        start: Option<BytecodeOperand>,
        end: Option<BytecodeOperand>,
        step: Option<BytecodeOperand>,
    },
    Call {
        callee: BytecodeOperand,
        arguments: Vec<BytecodeCallArgument>,
        signature: BytecodeTypeId,
        protocol: BytecodeCallProtocol,
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
        has_value: BytecodeBlockId,
        exhausted: BytecodeBlockId,
        unwind: BytecodeBlockId,
    },
    ValidatePlaces {
        places: Vec<BytecodePlace>,
        replacements: Vec<Option<BytecodeOperand>>,
        for_write: bool,
        target: BytecodeBlockId,
        unwind: BytecodeBlockId,
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
}
