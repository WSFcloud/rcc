use crate::common::span::SourceSpan;
use crate::frontend::sema::symbols::SymbolId;
use crate::frontend::sema::types::{FieldId, TypeId};

/// A typed translation unit (the result of semantic analysis).
///
/// This is the top-level output of the semantic analyzer,
/// containing all file-scope declarations and function definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedTranslationUnit {
    pub items: Vec<TypedExternalDecl>,
}

/// A file-scope declaration or function definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedExternalDecl {
    Function(TypedFunctionDef),
    Declaration(TypedDeclaration),
}

/// A function definition with its typed body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedFunctionDef {
    /// Symbol ID of the function.
    pub symbol: SymbolId,
    /// Typed function body (compound statement).
    pub body: TypedStmt,
    pub span: SourceSpan,
}

/// A declaration (possibly multiple declarators).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedDeclaration {
    /// Symbol IDs of declared entities.
    pub symbols: Vec<SymbolId>,
    /// Lowered initializers attached to this declaration.
    pub initializers: Vec<TypedDeclInit>,
    pub span: SourceSpan,
}

/// One lowered initializer attached to a declared symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedDeclInit {
    pub symbol: SymbolId,
    pub init: TypedInitializer,
}

/// A block item (declaration or statement).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedBlockItem {
    Declaration(TypedDeclaration),
    Stmt(TypedStmt),
}

/// A typed statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedStmt {
    pub kind: TypedStmtKind,
    pub span: SourceSpan,
}

/// Initializer for a `for` loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedForInit {
    Expr(TypedExpr),
    Decl(TypedDeclaration),
}

/// The kind of a typed statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedStmtKind {
    Compound(Vec<TypedBlockItem>),
    Expr(Option<TypedExpr>),
    If {
        cond: TypedExpr,
        then_branch: Box<TypedStmt>,
        else_branch: Option<Box<TypedStmt>>,
    },
    Switch {
        expr: TypedExpr,
        body: Box<TypedStmt>,
    },
    While {
        cond: TypedExpr,
        body: Box<TypedStmt>,
    },
    DoWhile {
        body: Box<TypedStmt>,
        cond: TypedExpr,
    },
    For {
        init: Option<TypedForInit>,
        cond: Option<TypedExpr>,
        step: Option<TypedExpr>,
        body: Box<TypedStmt>,
    },
    Return(Option<TypedExpr>),
    Break,
    Continue,
    Goto(LabelId),
    Label {
        label: LabelId,
        stmt: Box<TypedStmt>,
    },
    Case {
        value: CaseValue,
        stmt: Box<TypedStmt>,
    },
    Default {
        stmt: Box<TypedStmt>,
    },
}

/// Case label value (resolved or unresolved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseValue {
    /// Resolved constant value.
    Resolved(i64),
    /// Framework-stage fallback before constant-expression evaluation is wired.
    Unresolved(TypedExpr),
}

/// Opaque identifier for a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LabelId(pub u32);

impl LabelId {
    #[must_use]
    pub fn placeholder() -> Self {
        Self(u32::MAX)
    }
}

/// Unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Unary plus: `+x`.
    Plus,
    /// Unary minus: `-x`.
    Minus,
    /// Logical NOT: `!x`.
    LogicalNot,
    /// Bitwise NOT: `~x`.
    BitwiseNot,
    /// Address-of: `&x`.
    AddrOf,
    /// Dereference: `*x`.
    Deref,
    /// Pre-increment: `++x`.
    PreInc,
    /// Pre-decrement: `--x`.
    PreDec,
    /// Post-increment: `x++`.
    PostInc,
    /// Post-decrement: `x--`.
    PostDec,
}

/// Binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    // Arithmetic
    /// Addition: `a + b`.
    Add,
    /// Subtraction: `a - b`.
    Sub,
    /// Multiplication: `a * b`.
    Mul,
    /// Division: `a / b`.
    Div,
    /// Modulo: `a % b`.
    Mod,

    // Bitwise
    /// Bitwise AND: `a & b`.
    BitwiseAnd,
    /// Bitwise OR: `a | b`.
    BitwiseOr,
    /// Bitwise XOR: `a ^ b`.
    BitwiseXor,
    /// Left shift: `a << b`.
    Shl,
    /// Right shift: `a >> b`.
    Shr,

    // Comparison
    /// Equal: `a == b`.
    Eq,
    /// Not equal: `a != b`.
    Ne,
    /// Less than: `a < b`.
    Lt,
    /// Less than or equal: `a <= b`.
    Le,
    /// Greater than: `a > b`.
    Gt,
    /// Greater than or equal: `a >= b`.
    Ge,

    // Logical
    /// Logical AND: `a && b`.
    LogicalAnd,
    /// Logical OR: `a || b`.
    LogicalOr,
}

/// Assignment operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    /// Simple assignment: `a = b`.
    Assign,
    /// Add and assign: `a += b`.
    AddAssign,
    /// Subtract and assign: `a -= b`.
    SubAssign,
    /// Multiply and assign: `a *= b`.
    MulAssign,
    /// Divide and assign: `a /= b`.
    DivAssign,
    /// Modulo and assign: `a %= b`.
    ModAssign,
    /// Bitwise AND and assign: `a &= b`.
    AndAssign,
    /// Bitwise OR and assign: `a |= b`.
    OrAssign,
    /// Bitwise XOR and assign: `a ^= b`.
    XorAssign,
    /// Left shift and assign: `a <<= b`.
    ShlAssign,
    /// Right shift and assign: `a >>= b`.
    ShrAssign,
}

/// Value category of an expression (C99 6.3.2.1).
///
/// C expressions are classified into lvalues and non-lvalues.
/// This enum extends the classification to handle function and array designators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueCategory {
    /// Designates an object (modifiable or not).
    LValue,
    /// A computed value with no object identity.
    RValue,
    /// A function designator (decays to pointer in most contexts).
    FunctionDesignator,
    /// An array designator (decays to pointer in most contexts).
    ArrayDesignator,
}

/// A typed expression with semantic information.
///
/// This is the result of type-checking an expression, containing:
/// - The expression kind (operation or value)
/// - The resolved type
/// - The value category (lvalue, rvalue, etc.)
/// - Optional compile-time constant value
/// - Source location
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedExpr {
    pub kind: TypedExprKind,
    pub ty: TypeId,
    pub value_category: ValueCategory,
    pub const_value: Option<ConstValue>,
    pub span: SourceSpan,
}

impl TypedExpr {
    /// Creates an opaque placeholder expression.
    #[must_use]
    pub fn opaque(span: SourceSpan, ty: TypeId) -> Self {
        Self {
            kind: TypedExprKind::Opaque,
            ty,
            value_category: ValueCategory::RValue,
            const_value: None,
            span,
        }
    }

    /// Creates a symbol reference expression.
    #[must_use]
    pub fn symbol(symbol: SymbolId, span: SourceSpan, ty: TypeId) -> Self {
        Self {
            kind: TypedExprKind::SymbolRef(symbol),
            ty,
            value_category: ValueCategory::LValue,
            const_value: None,
            span,
        }
    }

    /// Creates a literal expression with a constant value.
    #[must_use]
    pub fn literal(value: ConstValue, span: SourceSpan, ty: TypeId) -> Self {
        Self {
            kind: TypedExprKind::Literal(value),
            ty,
            value_category: ValueCategory::RValue,
            const_value: Some(value),
            span,
        }
    }

    /// Creates an implicit cast expression.
    #[must_use]
    pub fn implicit_cast(expr: TypedExpr, to: TypeId, span: SourceSpan) -> Self {
        let const_value = expr.const_value;
        Self {
            kind: TypedExprKind::ImplicitCast {
                expr: Box::new(expr),
                to,
            },
            ty: to,
            value_category: ValueCategory::RValue,
            const_value,
            span,
        }
    }
}

/// The kind of a typed expression.
///
/// This enum represents different expression forms after type checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedExprKind {
    /// Placeholder node for expressions not yet lowered in detail.
    ///
    /// Used during framework stage or for expressions that don't need detailed representation.
    Opaque,

    /// Literal constant value.
    Literal(ConstValue),

    /// String literal with its content preserved.
    /// Type is `const char[N+1]` where N is the string length.
    StringLiteral(String),

    /// Reference to a declared symbol (variable, function, etc.).
    SymbolRef(SymbolId),

    /// Unary operation.
    Unary {
        op: UnaryOp,
        operand: Box<TypedExpr>,
    },

    /// Binary operation.
    Binary {
        op: BinaryOp,
        left: Box<TypedExpr>,
        right: Box<TypedExpr>,
    },

    /// Assignment operation.
    Assign {
        op: AssignOp,
        lhs: Box<TypedExpr>,
        rhs: Box<TypedExpr>,
    },

    /// Ternary conditional: `cond ? then_expr : else_expr`.
    Conditional {
        cond: Box<TypedExpr>,
        then_expr: Box<TypedExpr>,
        else_expr: Box<TypedExpr>,
    },

    /// Function call.
    Call {
        func: Box<TypedExpr>,
        args: Vec<TypedExpr>,
    },

    /// Array subscript: `base[index]`.
    Index {
        base: Box<TypedExpr>,
        index: Box<TypedExpr>,
    },

    /// Member access: `base.field` or `base->field`.
    MemberAccess {
        base: Box<TypedExpr>,
        field: FieldId,
        /// `true` for `->`, `false` for `.`.
        deref: bool,
    },

    /// Explicit type cast.
    Cast { expr: Box<TypedExpr>, to: TypeId },

    /// Implicit type conversion (array-to-pointer, function-to-pointer, etc.).
    ImplicitCast { expr: Box<TypedExpr>, to: TypeId },

    /// `sizeof` operator applied to a type.
    SizeofType { ty: TypeId },

    /// `sizeof` operator applied to an expression.
    SizeofExpr { expr: Box<TypedExpr> },

    /// Comma expression: evaluates left, discards result, returns right.
    Comma {
        left: Box<TypedExpr>,
        right: Box<TypedExpr>,
    },

    /// Compound literal: `(type){initializer}`.
    /// `is_file_scope` is true when the literal appears at file scope,
    /// giving it static storage duration (C99 6.5.2.5p5).
    CompoundLiteral {
        ty: TypeId,
        init: Box<TypedInitializer>,
        is_file_scope: bool,
    },
}

/// An initializer for a variable or aggregate.
///
/// This represents the right-hand side of an initialization:
/// - `int x = 42;` -> `Expr(42)`
/// - `int arr[] = {1, 2, 3};` -> `Aggregate([1, 2, 3])`
/// - `struct S s = {0};` -> `ZeroInit`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedInitializer {
    /// Single expression initializer.
    Expr(TypedExpr),
    /// Aggregate initializer (array or struct) — dense, positional.
    Aggregate(Vec<TypedInitItem>),
    /// Sparse array initializer — only stores explicitly initialized indices.
    /// Unmentioned slots are implicitly zero-initialized.
    SparseArray {
        elem_ty: TypeId,
        total_len: usize,
        entries: std::collections::BTreeMap<usize, TypedInitItem>,
    },
    /// Zero-initialization (all bytes set to zero).
    ZeroInit { ty: TypeId },
}

/// A single item in an aggregate initializer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedInitItem {
    pub init: TypedInitializer,
    pub span: SourceSpan,
}

/// A compile-time constant value.
///
/// This represents values that can be computed at compile time,
/// used for constant expressions, enum values, and static initializers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstValue {
    /// Signed integer constant.
    Int(i64),
    /// Unsigned integer constant.
    UInt(u64),
    /// IEEE 754 double-precision bit representation.
    ///
    /// Stored as bits to ensure exact equality comparison and hashing.
    FloatBits(u64),
    /// Null pointer constant.
    NullPtr,
    /// Address of a symbol with optional byte offset.
    ///
    /// Used for address constants like `&global_var` or `&array[5]`.
    Addr { symbol: SymbolId, offset: i64 },
}
