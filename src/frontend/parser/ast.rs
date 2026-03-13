/// Root node of a parsed C source file.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationUnit {
    pub items: Vec<ExternalDecl>,
}

/// One top-level item in a translation unit.
#[derive(Debug, Clone, PartialEq)]
pub enum ExternalDecl {
    FunctionDef(FunctionDef),
    Declaration(Declaration),
}

/// Function definition at file scope.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef {
    pub specifiers: DeclSpec,
    pub declarator: Declarator,
    pub declarations: Vec<Declaration>,
    pub body: CompoundStmt,
}

/// Declaration.
///
/// Examples:
/// - `int x;`
/// - `const int *p = 0;`
/// - `int a, *p, arr[10];`
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub specifiers: DeclSpec,
    pub declarators: Vec<InitDeclarator>,
}

/// Declaration specifiers shared by all declarators in a declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclSpec {
    pub storage: Vec<StorageClass>,
    pub qualifiers: Vec<TypeQualifier>,
    pub function: Vec<FunctionSpecifier>,
    pub ty: Vec<TypeSpecifier>,
}

/// Type name used in casts, `sizeof(type-name)`, and compound literals.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeName {
    pub specifiers: DeclSpec,
    pub declarator: Option<Box<Declarator>>,
}

/// Storage-class specifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageClass {
    Auto,
    Extern,
    Register,
    Static,
    Typedef,
}

/// Individual type qualifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeQualifier {
    Const,
    Volatile,
    Restrict,
}

/// Function specifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionSpecifier {
    Inline,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeSpecifier {
    Void,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    Signed,
    Unsigned,
    Bool,
    TypedefName(String),
}

/// One declarator together with its optional initializer.
#[derive(Debug, Clone, PartialEq)]
pub struct InitDeclarator {
    pub declarator: Declarator,
    pub init: Option<Initializer>,
}

/// Chumsky-friendly declarator shape:
/// parse `pointer*` first, then one direct declarator, then fold postfix suffixes.
#[derive(Debug, Clone, PartialEq)]
pub struct Declarator {
    pub pointers: Vec<Pointer>,
    pub direct: Box<DirectDeclarator>,
}

/// One `*` layer in a declarator.
#[derive(Debug, Clone, PartialEq)]
pub struct Pointer {
    pub qualifiers: Vec<TypeQualifier>,
}

/// Direct declarator forms.
///
/// These map naturally to:
/// - identifier
/// - parenthesized declarator
/// - array suffixes
/// - function suffixes
#[derive(Debug, Clone, PartialEq)]
pub enum DirectDeclarator {
    /// Plain identifier declarator.
    Ident(String),
    /// Abstract declarator without an identifier (e.g. unnamed parameter declarator).
    Abstract,
    /// Parenthesized declarator `(declarator)`.
    Grouped(Box<Declarator>),
    /// Array declarator `inner[...]`.
    Array {
        inner: Box<DirectDeclarator>,
        qualifiers: Vec<TypeQualifier>,
        is_static: bool,
        size: Box<ArraySize>,
    },
    /// Function declarator `inner(params...)`.
    Function {
        inner: Box<DirectDeclarator>,
        params: FunctionParams,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArraySize {
    /// `[]`
    Unspecified,
    /// `[expr]`
    Expr(Expr),
    /// `[*]` in prototype scope (reserved; currently not produced by parser)
    Variable,
}

/// Function parameter list forms.
///
/// `Nonspecified` corresponds to declarations like `int f();`
#[derive(Debug, Clone, PartialEq)]
pub enum FunctionParams {
    /// Prototype-style parameter list.
    Prototype {
        params: Vec<ParameterDecl>,
        variadic: bool,
    },
    /// Parameter list omitted, e.g. `int f();`.
    NonPrototype,
}

/// A single function parameter declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterDecl {
    pub specifiers: DeclSpec,
    /// `declarator = None`: No declarator present (e.g. `int`).
    /// Unnamed but structured declarators like `char *` use `DirectDeclarator::Abstract`.
    pub declarator: Option<Box<Declarator>>,
}

/// Wrapper node for initializers.
#[derive(Debug, Clone, PartialEq)]
pub struct Initializer {
    pub kind: InitializerKind,
}

/// Initializer forms.
#[derive(Debug, Clone, PartialEq)]
pub enum InitializerKind {
    /// Scalar initialization (`= expr`)
    Expr(Expr),
    /// Aggregate initialization (`= { ... }`).
    Aggregate(Vec<InitializerItem>),
}

/// One item inside an aggregate initializer.
#[derive(Debug, Clone, PartialEq)]
pub struct InitializerItem {
    pub designators: Vec<Designator>,
    pub init: Initializer,
}

/// One designator segment.
#[derive(Debug, Clone, PartialEq)]
pub enum Designator {
    Index(Expr),
    Field(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompoundStmt {
    pub items: Vec<BlockItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockItem {
    Decl(Declaration),
    Stmt(Stmt),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ForInit {
    Expr(Expr),
    Decl(Declaration),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `;`
    Empty,
    /// Expression statement `expr;`.
    Expr(Expr),
    /// Compound statement `{ ... }`
    Compound(CompoundStmt),
    /// `if (cond) then_branch else else_branch`
    If {
        cond: Expr,
        then_branch: Box<Stmt>,
        else_branch: Option<Box<Stmt>>,
    },
    /// `switch (expr) body`
    Switch { expr: Expr, body: Box<Stmt> },
    /// `while (cond) body`
    While { cond: Expr, body: Box<Stmt> },
    /// `do body while (cond);`
    DoWhile { body: Box<Stmt>, cond: Expr },
    /// `for (init; cond; step) body`
    For {
        init: Option<ForInit>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
    },
    /// `return <expr>;` / `return;`
    Return(Option<Expr>),
    /// `break;`
    Break,
    /// `continue;`
    Continue,
    /// `goto label;`
    Goto(String),
    /// `label: stmt`
    Label { label: String, stmt: Box<Stmt> },
    /// `case expr: stmt`
    Case { expr: Expr, stmt: Box<Stmt> },
    /// `default: stmt`
    Default { stmt: Box<Stmt> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
}

impl Expr {
    /// Create a new expression from an ExprKind.
    pub fn new(kind: ExprKind) -> Self {
        Self { kind }
    }

    pub fn int(value: u64) -> Self {
        Self::int_with_base(value, IntLiteralSuffix::Int)
    }

    pub fn int_with_base(value: u64, base: IntLiteralSuffix) -> Self {
        Self::new(ExprKind::Literal(Literal::Int { value, base }))
    }

    pub fn float(value: f64) -> Self {
        Self::new(ExprKind::Literal(Literal::Float(value)))
    }

    pub fn char(value: char) -> Self {
        Self::new(ExprKind::Literal(Literal::Char(value)))
    }

    pub fn string(value: String) -> Self {
        Self::new(ExprKind::Literal(Literal::String(value)))
    }

    pub fn var(name: String) -> Self {
        Self::new(ExprKind::Var(name))
    }

    pub fn unary(op: UnaryOp, expr: Self) -> Self {
        Self::new(ExprKind::Unary {
            op,
            expr: Box::new(expr),
        })
    }

    pub fn binary(left: Self, op: BinaryOp, right: Self) -> Self {
        Self::new(ExprKind::Binary {
            left: Box::new(left),
            op,
            right: Box::new(right),
        })
    }

    pub fn assign(left: Self, op: AssignOp, right: Self) -> Self {
        Self::new(ExprKind::Assign {
            left: Box::new(left),
            op,
            right: Box::new(right),
        })
    }

    pub fn conditional(cond: Self, then_expr: Self, else_expr: Self) -> Self {
        Self::new(ExprKind::Conditional {
            cond: Box::new(cond),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
        })
    }

    pub fn call(callee: Self, args: Vec<Self>) -> Self {
        Self::new(ExprKind::Call {
            callee: Box::new(callee),
            args,
        })
    }

    pub fn index(base: Self, index: Self) -> Self {
        Self::new(ExprKind::Index {
            base: Box::new(base),
            index: Box::new(index),
        })
    }

    pub fn member(base: Self, field: String, deref: bool) -> Self {
        Self::new(ExprKind::Member {
            base: Box::new(base),
            field,
            deref,
        })
    }

    pub fn comma(left: Self, right: Self) -> Self {
        Self::new(ExprKind::Comma {
            left: Box::new(left),
            right: Box::new(right),
        })
    }

    pub fn pre_inc(expr: Self) -> Self {
        Self::new(ExprKind::PreInc(Box::new(expr)))
    }

    pub fn pre_dec(expr: Self) -> Self {
        Self::new(ExprKind::PreDec(Box::new(expr)))
    }

    pub fn post_inc(expr: Self) -> Self {
        Self::new(ExprKind::PostInc(Box::new(expr)))
    }

    pub fn post_dec(expr: Self) -> Self {
        Self::new(ExprKind::PostDec(Box::new(expr)))
    }

    pub fn sizeof_expr(expr: Self) -> Self {
        Self::new(ExprKind::SizeofExpr(Box::new(expr)))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// Numeric / Character / String literal.
    Literal(Literal),
    /// Identifier.
    Var(String),
    /// Prefix unary expression.
    Unary { op: UnaryOp, expr: Box<Expr> },
    /// Binary expression.
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    /// Assignment expression.
    Assign {
        left: Box<Expr>,
        op: AssignOp,
        right: Box<Expr>,
    },
    /// Comma expression.
    Comma { left: Box<Expr>, right: Box<Expr> },
    /// Ternary conditional expression.
    Conditional {
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    /// Array subscripting expression.
    Index { base: Box<Expr>, index: Box<Expr> },
    /// Member access expression.
    ///
    /// - `deref = false` represents `a.b`.
    /// - `deref = true` represents `a->b`.
    Member {
        base: Box<Expr>,
        field: String,
        deref: bool,
    },
    /// Function call expression.
    Call { callee: Box<Expr>, args: Vec<Expr> },
    /// Cast expression `(type-name) expr`.
    Cast { ty: Box<TypeName>, expr: Box<Expr> },
    /// `sizeof expr`
    SizeofExpr(Box<Expr>),
    /// `sizeof(type-name)`
    SizeofType(Box<TypeName>),
    /// Prefix increment `++expr`
    PreInc(Box<Expr>),
    /// Prefix decrement `--expr`
    PreDec(Box<Expr>),
    /// Postfix increment `expr++`
    PostInc(Box<Expr>),
    /// Postfix decrement `expr--`
    PostDec(Box<Expr>),
    /// Compound literal `(type-name){ ... }`
    CompoundLiteral {
        ty: Box<TypeName>,
        init: Box<Initializer>,
    },
}

/// Literal values that can appear in source code.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int { value: u64, base: IntLiteralSuffix },
    Float(f64),
    Char(char),
    String(String),
}

/// Integer literal base type determined by suffix.
///
/// When there is no integer suffix, parser defaults it to `Int`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntLiteralSuffix {
    Int,
    UInt,
    Long,
    ULong,
    LongLong,
    ULongLong,
}

/// Prefix unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Minus,
    LogicalNot,
    BitNot,
    Deref,
    AddressOf,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Mul,
    Div,
    Mod,
    Add,
    Sub,
    Shl,
    Shr,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    BitAnd,
    BitXor,
    BitOr,
    LogicalAnd,
    LogicalOr,
}

/// Assignment operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
    ShlAssign,
    ShrAssign,
    BitAndAssign,
    BitXorAssign,
    BitOrAssign,
}
