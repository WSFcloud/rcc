/// Top-level MIR container.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MirProgram {
    /// Global objects (definitions and extern declarations).
    pub globals: Vec<MirGlobal>,
    /// Function bodies defined in this translation unit.
    pub functions: Vec<MirFunction>,
    /// Extern function declarations.
    pub extern_functions: Vec<MirExternFunction>,
}

/// A MIR function definition.
#[derive(Debug, Clone, PartialEq)]
pub struct MirFunction {
    /// Function symbol name.
    pub name: String,
    /// Linkage kind visible at MIR layer.
    pub linkage: MirLinkage,
    /// ABI-normalized parameter descriptors.
    pub params: Vec<MirAbiParam>,
    /// ABI-normalized return type.
    pub return_type: MirType,
    /// Source-level boundary ABI used when this function crosses the C ABI.
    pub boundary_sig: MirBoundarySignature,
    /// Whether this function accepts variadic arguments.
    pub variadic: bool,
    /// Function-local stack slots.
    pub stack_slots: Vec<StackSlot>,
    /// Basic blocks. `blocks[0]` is the entry block.
    pub blocks: Vec<BasicBlock>,
    /// Monotonic allocator cursor for virtual registers.
    pub virtual_reg_counter: u32,
}

impl MirFunction {
    /// Allocate a new virtual register id.
    pub fn alloc_vreg(&mut self, ty: MirType) -> TypedVReg {
        let reg = VReg(self.virtual_reg_counter);
        self.virtual_reg_counter += 1;
        TypedVReg { reg, ty }
    }
}

/// Function declaration metadata for extern functions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirExternFunction {
    pub name: String,
    pub sig: MirFunctionSig,
    pub boundary_sig: MirBoundarySignature,
}

/// Function signature information used by extern decls / indirect calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirFunctionSig {
    pub params: Vec<MirAbiParam>,
    pub return_type: MirType,
    pub variadic: bool,
}

/// Source-level ABI used at the object boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirBoundarySignature {
    pub params: Vec<MirBoundaryParam>,
    pub return_type: MirBoundaryReturn,
    pub variadic: bool,
}

impl MirBoundarySignature {
    #[must_use]
    pub fn from_internal(params: &[MirAbiParam], return_type: MirType, variadic: bool) -> Self {
        let mut boundary_params = Vec::with_capacity(params.len());
        for &param in params {
            let boundary = match param.purpose {
                MirAbiParamPurpose::Normal => MirBoundaryParam::Scalar(param.ty),
                MirAbiParamPurpose::StructArgument { size } => MirBoundaryParam::AggregateMemory {
                    size,
                    abi_size: size,
                },
                MirAbiParamPurpose::StructReturn => continue,
            };
            boundary_params.push(boundary);
        }

        let boundary_return = if let Some(param) = params
            .iter()
            .find(|param| param.purpose == MirAbiParamPurpose::StructReturn)
        {
            let _ = param;
            MirBoundaryReturn::AggregateMemory { size: 0 }
        } else if return_type == MirType::Void {
            MirBoundaryReturn::Void
        } else {
            MirBoundaryReturn::Scalar(return_type)
        };

        Self {
            params: boundary_params,
            return_type: boundary_return,
            variadic,
        }
    }

    #[must_use]
    pub fn requires_wrapper(&self) -> bool {
        self.params
            .iter()
            .any(|param| matches!(param, MirBoundaryParam::AggregateScalarized { .. }))
            || matches!(
                self.return_type,
                MirBoundaryReturn::AggregateScalarized { .. }
            )
    }

    #[must_use]
    pub fn has_unsupported_aggregate(&self) -> bool {
        self.params
            .iter()
            .any(|param| matches!(param, MirBoundaryParam::AggregateUnsupported { .. }))
            || matches!(
                self.return_type,
                MirBoundaryReturn::AggregateUnsupported { .. }
            )
    }
}

/// One source-level boundary ABI parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirBoundaryParam {
    Scalar(MirType),
    AggregateScalarized { parts: Vec<MirType>, size: u32 },
    AggregateMemory { size: u32, abi_size: u32 },
    AggregateUnsupported { size: u32 },
}

/// Source-level boundary ABI return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirBoundaryReturn {
    Void,
    Scalar(MirType),
    AggregateScalarized { parts: Vec<MirType>, size: u32 },
    AggregateMemory { size: u32 },
    AggregateUnsupported { size: u32 },
}

/// One ABI-normalized function parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MirAbiParam {
    pub ty: MirType,
    pub purpose: MirAbiParamPurpose,
}

impl MirAbiParam {
    #[must_use]
    pub const fn new(ty: MirType) -> Self {
        Self {
            ty,
            purpose: MirAbiParamPurpose::Normal,
        }
    }

    #[must_use]
    pub const fn struct_argument(size: u32) -> Self {
        Self {
            ty: MirType::Ptr,
            purpose: MirAbiParamPurpose::StructArgument { size },
        }
    }

    #[must_use]
    pub const fn struct_return() -> Self {
        Self {
            ty: MirType::Ptr,
            purpose: MirAbiParamPurpose::StructReturn,
        }
    }
}

/// Special ABI handling for one function parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MirAbiParamPurpose {
    Normal,
    StructArgument { size: u32 },
    StructReturn,
}

/// A global variable declaration/definition.
#[derive(Debug, Clone, PartialEq)]
pub struct MirGlobal {
    pub name: String,
    pub size: u64,
    pub alignment: u32,
    pub linkage: MirLinkage,
    /// `None` means this is an extern declaration.
    pub init: Option<MirGlobalInit>,
}

/// Linkage kind visible at MIR layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MirLinkage {
    External,
    Internal,
}

/// Global initializer payload.
#[derive(Debug, Clone, PartialEq)]
pub enum MirGlobalInit {
    /// Plain constant bytes (integers/floats/strings flattened into data bytes).
    Data(Vec<u8>),
    /// Data with relocation entries.
    RelocatedData {
        bytes: Vec<u8>,
        relocations: Vec<MirRelocation>,
    },
    /// Zero-initialized object.
    Zero,
}

/// One relocation record inside global initializer data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirRelocation {
    /// Byte offset into the containing initializer data.
    pub offset: u64,
    pub target: MirRelocationTarget,
    /// Symbol addend.
    pub addend: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirRelocationTarget {
    Global(String),
    Function(String),
}

/// MIR primitive type set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MirType {
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Ptr,
    Void,
}

impl MirType {
    #[must_use]
    pub fn is_integer(self) -> bool {
        matches!(self, Self::I8 | Self::I16 | Self::I32 | Self::I64)
    }

    #[must_use]
    pub fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }

    #[must_use]
    pub fn is_scalar(self) -> bool {
        self.is_integer() || self.is_float() || matches!(self, Self::Ptr)
    }
}

/// Immediate constants used by MIR instructions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MirConst {
    /// Integer bit-pattern payload (semantics chosen by consumer instruction).
    IntConst(i64),
    /// Float payload.
    FloatConst(f64),
    /// Zero-value sentinel.
    ZeroConst,
}

/// Virtual register identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VReg(pub u32);

/// Typed virtual register (MIR keeps value type at definition sites).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypedVReg {
    pub reg: VReg,
    pub ty: MirType,
}

/// Stack slot identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotId(pub u32);

/// Basic block identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

/// Function-local stack allocation metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackSlot {
    pub id: SlotId,
    pub size: u32,
    pub alignment: u32,
    /// Whether `slot_addr` was emitted for this slot.
    pub address_taken: bool,
}

/// Instruction operand.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    VReg(VReg),
    Const(MirConst),
    /// Stack-slot address operand (for `load/store` stack access forms).
    StackSlot(SlotId),
}

/// Basic block = linear instructions + one terminator.
#[derive(Debug, Clone, PartialEq)]
pub struct BasicBlock {
    pub id: BlockId,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}

/// MIR instruction set.
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    // Memory
    Load {
        dst: TypedVReg,
        slot: SlotId,
        offset: i64,
        volatile: bool,
    },
    Store {
        slot: SlotId,
        offset: i64,
        value: Operand,
        ty: MirType,
        volatile: bool,
    },
    PtrLoad {
        dst: TypedVReg,
        ptr: Operand,
        ty: MirType,
        volatile: bool,
    },
    PtrStore {
        ptr: Operand,
        value: Operand,
        ty: MirType,
        volatile: bool,
    },
    Memcpy {
        dst_ptr: Operand,
        src_ptr: Operand,
        size: u32,
    },
    Memset {
        dst_ptr: Operand,
        value: Operand,
        size: u32,
    },

    // Arithmetic / bitwise
    Binary {
        dst: TypedVReg,
        op: BinaryOp,
        lhs: Operand,
        rhs: Operand,
        ty: MirType,
    },
    Unary {
        dst: TypedVReg,
        op: UnaryOp,
        operand: Operand,
        ty: MirType,
    },

    // Comparison (result must be i8: 0/1)
    Cmp {
        dst: TypedVReg,
        kind: CmpKind,
        domain: CmpDomain,
        lhs: Operand,
        rhs: Operand,
        ty: MirType,
    },

    // Casts
    Cast {
        dst: TypedVReg,
        kind: CastKind,
        src: Operand,
        from_ty: MirType,
        to_ty: MirType,
    },

    // Calls
    Call {
        dst: Option<TypedVReg>,
        callee: String,
        args: Vec<Operand>,
        /// Present for variadic calls (`printf`-style).
        fixed_arg_count: Option<usize>,
    },
    CallIndirect {
        dst: Option<TypedVReg>,
        callee_ptr: Operand,
        args: Vec<Operand>,
        sig: MirFunctionSig,
        boundary_sig: Option<MirBoundarySignature>,
        /// Present for variadic calls.
        fixed_arg_count: Option<usize>,
    },

    // Address / move
    SlotAddr {
        dst: TypedVReg,
        slot: SlotId,
    },
    GlobalAddr {
        dst: TypedVReg,
        global: String,
    },
    PtrAdd {
        dst: TypedVReg,
        base: Operand,
        byte_offset: Operand,
    },
    Copy {
        dst: TypedVReg,
        src: Operand,
    },
}

impl Instruction {
    /// Whether this instruction has observable side effects and therefore
    /// cannot be removed by a generic dead-code pass.
    #[must_use]
    pub fn has_side_effects(&self) -> bool {
        match self {
            Self::Load { volatile, .. } | Self::PtrLoad { volatile, .. } => *volatile,
            Self::Store { .. }
            | Self::PtrStore { .. }
            | Self::Memcpy { .. }
            | Self::Memset { .. }
            | Self::Call { .. }
            | Self::CallIndirect { .. } => true,
            Self::Binary { .. }
            | Self::Unary { .. }
            | Self::Cmp { .. }
            | Self::Cast { .. }
            | Self::SlotAddr { .. }
            | Self::GlobalAddr { .. }
            | Self::PtrAdd { .. }
            | Self::Copy { .. } => false,
        }
    }

    /// Pure instruction (no observable side effects).
    #[must_use]
    pub fn is_pure(&self) -> bool {
        !self.has_side_effects()
    }
}

/// Binary operator set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    // Integer/bitwise arithmetic
    Add,
    Sub,
    Mul,
    SDiv,
    UDiv,
    SRem,
    URem,
    And,
    Or,
    Xor,
    Shl,
    AShr,
    LShr,

    // Floating-point arithmetic
    FAdd,
    FSub,
    FMul,
    FDiv,
    FRem,
}

/// Unary operator set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// Comparison relation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpKind {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Comparison semantic domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpDomain {
    Signed,
    Unsigned,
    Float,
}

/// Type conversion kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CastKind {
    Trunc,
    ZExt,
    SExt,
    SIToF,
    UIToF,
    FToSI,
    FToUI,
    FExt,
    FTrunc,
    PToI,
    IToP,
}

/// Basic-block terminators.
#[derive(Debug, Clone, PartialEq)]
pub enum Terminator {
    Jump(BlockId),
    Branch {
        cond: VReg,
        then_bb: BlockId,
        else_bb: BlockId,
    },
    Switch {
        discr: VReg,
        cases: Vec<SwitchCase>,
        default: BlockId,
    },
    Ret(Option<Operand>),
    Unreachable,
}

/// One switch dispatch edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchCase {
    pub value: i64,
    pub target: BlockId,
}
