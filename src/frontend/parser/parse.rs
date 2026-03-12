use crate::common::token::TokenKind;
use crate::frontend::parser::ast::{
    ArraySize, AssignOp, BinaryOp, BlockItem, CompoundStmt, DeclSpec, Declaration, Declarator,
    DirectDeclarator, Expr, ExternalDecl, ForInit, FunctionDef, FunctionParams, FunctionSpecifier,
    InitDeclarator, Initializer, InitializerKind, IntLiteralBase, ParameterDecl, Pointer, Stmt,
    StorageClass, TranslationUnit, TypeQualifier, TypeSpecifier, UnaryOp,
};
use crate::frontend::parser::labels::ParserLabel;
use chumsky::{
    error::Rich,
    input::ValueInput,
    pratt::{infix, left, postfix, prefix},
    prelude::*,
    span::SimpleSpan,
};

pub type Span = SimpleSpan<usize>;
pub type ParseError<'tokens> = Rich<'tokens, TokenKind, Span>;

// ============================
// Expression parsing
// ============================

/// Prefix operators represented before binding to a concrete RHS expression.
#[derive(Clone, Copy)]
enum PrefixExprOp {
    Unary(UnaryOp),
    PreInc,
    PreDec,
    Sizeof,
}

impl PrefixExprOp {
    /// Build the corresponding AST node for a parsed prefix operator.
    fn apply(self, rhs: Expr) -> Expr {
        match self {
            Self::Unary(op) => Expr::unary(op, rhs),
            Self::PreInc => Expr::pre_inc(rhs),
            Self::PreDec => Expr::pre_dec(rhs),
            Self::Sizeof => Expr::sizeof_expr(rhs),
        }
    }
}

/// Postfix operators represented before binding to a concrete LHS expression.
#[derive(Clone)]
enum PostfixExprOp {
    PostInc,
    PostDec,
    /// Function call postfix: `callee(args...)`.
    Call(Vec<Expr>),
    /// Array subscript postfix: `base[index]`.
    Index(Expr),
    /// Member access postfix: `base.field` or `base->field`.
    Member {
        field: String,
        deref: bool,
    },
}

impl PostfixExprOp {
    /// Build the corresponding AST node for a parsed postfix operator.
    fn apply(self, lhs: Expr) -> Expr {
        match self {
            Self::PostInc => Expr::post_inc(lhs),
            Self::PostDec => Expr::post_dec(lhs),
            Self::Call(args) => Expr::call(lhs, args),
            Self::Index(index) => Expr::index(lhs, index),
            Self::Member { field, deref } => Expr::member(lhs, field, deref),
        }
    }
}

/// Fold `a, b, c` into nested comma AST: `((a, b), c)`.
fn fold_comma_expr(exprs: Vec<Expr>, context: &'static str) -> Expr {
    exprs.into_iter().reduce(Expr::comma).expect(context)
}

fn literal_expr_parser<'tokens, I>()
-> impl Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
        select! {
            TokenKind::IntLiteral(value) => Expr::int(value),
            TokenKind::UIntLiteral(value) => Expr::int_with_base(value, IntLiteralBase::UInt),
            TokenKind::LongLiteral(value) => Expr::int_with_base(value, IntLiteralBase::Long),
            TokenKind::ULongLiteral(value) => Expr::int_with_base(value, IntLiteralBase::ULong),
            TokenKind::LongLongLiteral(value) => Expr::int_with_base(value, IntLiteralBase::LongLong),
            TokenKind::ULongLongLiteral(value) => Expr::int_with_base(value, IntLiteralBase::ULongLong),
        },
        select! {
            TokenKind::FloatLiteral(value) => Expr::float(value),
            TokenKind::FloatLiteralF32(value) => Expr::float(value),
        },
        select! {
            TokenKind::CharLiteral(value) => Expr::char(value),
            TokenKind::StringLiteral(value) => Expr::string(value),
        },
    ))
}

fn identifier_expr_parser<'tokens, I>()
-> impl Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    select! {
        TokenKind::Identifier(name) => Expr::var(name),
    }
}

fn prefix_expr_op_parser<'tokens, I>()
-> impl Parser<'tokens, I, PrefixExprOp, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
        just(TokenKind::PlusPlus).to(PrefixExprOp::PreInc),
        just(TokenKind::MinusMinus).to(PrefixExprOp::PreDec),
        just(TokenKind::Sizeof).to(PrefixExprOp::Sizeof),
        just(TokenKind::Plus).to(PrefixExprOp::Unary(UnaryOp::Plus)),
        just(TokenKind::Minus).to(PrefixExprOp::Unary(UnaryOp::Minus)),
        just(TokenKind::Bang).to(PrefixExprOp::Unary(UnaryOp::LogicalNot)),
        just(TokenKind::Tilde).to(PrefixExprOp::Unary(UnaryOp::BitNot)),
        just(TokenKind::Star).to(PrefixExprOp::Unary(UnaryOp::Deref)),
        just(TokenKind::Amp).to(PrefixExprOp::Unary(UnaryOp::AddressOf)),
    ))
}

fn basic_postfix_expr_op_parser<'tokens, I>()
-> impl Parser<'tokens, I, PostfixExprOp, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
        just(TokenKind::PlusPlus).to(PostfixExprOp::PostInc),
        just(TokenKind::MinusMinus).to(PostfixExprOp::PostDec),
    ))
}

fn assignment_op_parser<'tokens, I>()
-> impl Parser<'tokens, I, AssignOp, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
        just(TokenKind::Assign).to(AssignOp::Assign),
        just(TokenKind::PlusAssign).to(AssignOp::AddAssign),
        just(TokenKind::MinusAssign).to(AssignOp::SubAssign),
        just(TokenKind::StarAssign).to(AssignOp::MulAssign),
        just(TokenKind::SlashAssign).to(AssignOp::DivAssign),
        just(TokenKind::PercentAssign).to(AssignOp::ModAssign),
        just(TokenKind::LessLessAssign).to(AssignOp::ShlAssign),
        just(TokenKind::GreaterGreaterAssign).to(AssignOp::ShrAssign),
        just(TokenKind::AmpAssign).to(AssignOp::BitAndAssign),
        just(TokenKind::CaretAssign).to(AssignOp::BitXorAssign),
        just(TokenKind::PipeAssign).to(AssignOp::BitOrAssign),
    ))
}

fn pratt_expr_parser<'tokens, I, P, PO>(
    atom: P,
    postfix_op: PO,
) -> impl Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone,
    PO: Parser<'tokens, I, PostfixExprOp, extra::Err<ParseError<'tokens>>> + Clone,
{
    atom.pratt((
        postfix(15, postfix_op, |lhs, op: PostfixExprOp, _| op.apply(lhs)),
        prefix(14, prefix_expr_op_parser(), |op: PrefixExprOp, rhs, _| {
            op.apply(rhs)
        }),
        infix(
            left(13),
            choice((
                just(TokenKind::Star).to(BinaryOp::Mul),
                just(TokenKind::Slash).to(BinaryOp::Div),
                just(TokenKind::Percent).to(BinaryOp::Mod),
            )),
            |lhs, op, rhs, _| Expr::binary(lhs, op, rhs),
        ),
        infix(
            left(12),
            choice((
                just(TokenKind::Plus).to(BinaryOp::Add),
                just(TokenKind::Minus).to(BinaryOp::Sub),
            )),
            |lhs, op, rhs, _| Expr::binary(lhs, op, rhs),
        ),
        infix(
            left(11),
            choice((
                just(TokenKind::LessLess).to(BinaryOp::Shl),
                just(TokenKind::GreaterGreater).to(BinaryOp::Shr),
            )),
            |lhs, op, rhs, _| Expr::binary(lhs, op, rhs),
        ),
        infix(
            left(10),
            choice((
                just(TokenKind::Less).to(BinaryOp::Lt),
                just(TokenKind::LessEqual).to(BinaryOp::Le),
                just(TokenKind::Greater).to(BinaryOp::Gt),
                just(TokenKind::GreaterEqual).to(BinaryOp::Ge),
            )),
            |lhs, op, rhs, _| Expr::binary(lhs, op, rhs),
        ),
        infix(
            left(9),
            choice((
                just(TokenKind::EqualEqual).to(BinaryOp::Eq),
                just(TokenKind::BangEqual).to(BinaryOp::Ne),
            )),
            |lhs, op, rhs, _| Expr::binary(lhs, op, rhs),
        ),
        infix(
            left(8),
            just(TokenKind::Amp).to(BinaryOp::BitAnd),
            |lhs, op, rhs, _| Expr::binary(lhs, op, rhs),
        ),
        infix(
            left(7),
            just(TokenKind::Caret).to(BinaryOp::BitXor),
            |lhs, op, rhs, _| Expr::binary(lhs, op, rhs),
        ),
        infix(
            left(6),
            just(TokenKind::Pipe).to(BinaryOp::BitOr),
            |lhs, op, rhs, _| Expr::binary(lhs, op, rhs),
        ),
        infix(
            left(5),
            just(TokenKind::AmpAmp).to(BinaryOp::LogicalAnd),
            |lhs, op, rhs, _| Expr::binary(lhs, op, rhs),
        ),
        infix(
            left(4),
            just(TokenKind::PipePipe).to(BinaryOp::LogicalOr),
            |lhs, op, rhs, _| Expr::binary(lhs, op, rhs),
        ),
    ))
}

fn comma_sequence_parser<'tokens, I, P>(
    operand: P,
    context: &'static str,
) -> impl Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone,
{
    operand
        .separated_by(just(TokenKind::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .map(move |exprs| fold_comma_expr(exprs, context))
}

/// Parse expression atoms: literals, identifiers, and parenthesized expressions.
fn expr_atom_parser<'tokens, I, P>(
    grouped_expr: P,
) -> impl Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone,
{
    choice((
        literal_expr_parser(),
        identifier_expr_parser(),
        grouped_expr.delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
    ))
    .labelled(ParserLabel::Expr.as_str())
}

/// Parse function call postfix operator: `(arg0, arg1, ...)`.
fn call_postfix_expr_op_parser<'tokens, I, P>(
    assignment: P,
) -> impl Parser<'tokens, I, PostfixExprOp, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone,
{
    assignment
        .separated_by(just(TokenKind::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .or_not()
        .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen))
        .map(|args| PostfixExprOp::Call(args.unwrap_or_default()))
}

/// Parse array subscript postfix operator: `[index_expr]`.
fn index_postfix_expr_op_parser<'tokens, I, P>(
    index_expr: P,
) -> impl Parser<'tokens, I, PostfixExprOp, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone,
{
    index_expr
        .delimited_by(just(TokenKind::LBracket), just(TokenKind::RBracket))
        .map(PostfixExprOp::Index)
}

/// Parse member access postfix operators: `.field` and `->field`.
fn member_postfix_expr_op_parser<'tokens, I>()
-> impl Parser<'tokens, I, PostfixExprOp, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
        just(TokenKind::Dot)
            .ignore_then(select! { TokenKind::Identifier(name) => name })
            .map(|field| PostfixExprOp::Member {
                field,
                deref: false,
            }),
        just(TokenKind::Arrow)
            .ignore_then(select! { TokenKind::Identifier(name) => name })
            .map(|field| PostfixExprOp::Member { field, deref: true }),
    ))
}

/// Build `conditional-expression` and assignment chain on top of Pratt expressions.
fn assignment_core_expr_parser<'tokens, I, P, Q, R>(
    pratt: P,
    assignment: Q,
    assign_op: R,
) -> impl Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone,
    Q: Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone,
    R: Parser<'tokens, I, AssignOp, extra::Err<ParseError<'tokens>>> + Clone,
{
    // In `cond ? then : else`, the then-branch is an `expression`
    // (comma expressions are allowed), while else-branch is assignment-expression.
    let then_expr = comma_sequence_parser(
        assignment.clone(),
        "then-branch requires at least one expression",
    );

    let conditional = pratt
        .then(
            just(TokenKind::Question)
                .ignore_then(then_expr)
                .then_ignore(just(TokenKind::Colon))
                .then(assignment.clone())
                .or_not(),
        )
        .map(|(cond, branch)| match branch {
            Some((then_expr, else_expr)) => Expr::conditional(cond, then_expr, else_expr),
            None => cond,
        });

    conditional
        .then(assign_op.then(assignment).or_not())
        .map(|(left, assign)| match assign {
            Some((op, right)) => Expr::assign(left, op, right),
            None => left,
        })
}

/// Parse C expressions.
///
/// `ALLOW_COMMA` controls whether top-level comma expressions are accepted.
/// Parenthesized sub-expressions always parse as full `expression` grammar.
fn expr_parser<'tokens, I, const ALLOW_COMMA: bool>()
-> impl Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    // This parser is used by parenthesized expressions and must always allow commas.
    let mut grouped_expr = Recursive::declare();

    let atom = expr_atom_parser(grouped_expr.clone());
    let assign_op = assignment_op_parser();

    let assignment = recursive(|assignment| {
        // In C, call arguments are assignment-expressions separated by commas.
        // `foo(1, x = 2)` is valid; `foo((1, 2))` still works via parenthesized expr.
        let subscript_expr = comma_sequence_parser(
            assignment.clone(),
            "array subscript requires one expression",
        );

        let postfix_op = choice((
            basic_postfix_expr_op_parser(),
            call_postfix_expr_op_parser(assignment.clone()),
            index_postfix_expr_op_parser(subscript_expr),
            member_postfix_expr_op_parser(),
        ));

        // Pratt parsing handles the regular precedence ladder.
        // Larger binding power means tighter binding.
        let pratt = pratt_expr_parser(atom.clone(), postfix_op);

        assignment_core_expr_parser(pratt, assignment, assign_op.clone())
    });

    let comma_expr = comma_sequence_parser(
        assignment.clone(),
        "comma expression requires at least one operand",
    );

    // C grammar uses `expression` inside parentheses, so comma expressions
    // must remain available there even in assignment-expression contexts.
    grouped_expr.define(comma_expr.clone());

    if ALLOW_COMMA {
        comma_expr.boxed()
    } else {
        assignment.boxed()
    }
}

/// Parse an assignment-expression (comma not allowed at top level).
fn assignment_expression_parser<'tokens, I>()
-> impl Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    expr_parser::<'tokens, I, false>()
}

// ============================
// Declaration parsing
// ============================

/// Parse one type qualifier token.
fn type_qualifier_parser<'tokens, I>()
-> impl Parser<'tokens, I, TypeQualifier, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
        just(TokenKind::Const).to(TypeQualifier::Const),
        just(TokenKind::Volatile).to(TypeQualifier::Volatile),
        just(TokenKind::Restrict).to(TypeQualifier::Restrict),
    ))
}

/// Parse a single pointer layer: `*` with optional qualifiers.
fn pointer_layer_parser<'tokens, I>()
-> impl Parser<'tokens, I, Pointer, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    just(TokenKind::Star)
        .ignore_then(
            type_qualifier_parser()
                .repeated()
                .collect::<Vec<TypeQualifier>>(),
        )
        .map(|qualifiers| Pointer { qualifiers })
}

/// Parse zero-or-more pointer layers for declarators.
fn pointer_layers_parser<'tokens, I>()
-> impl Parser<'tokens, I, Vec<Pointer>, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    pointer_layer_parser().repeated().collect::<Vec<_>>()
}

#[derive(Clone)]
enum DeclSpecifierPiece {
    Storage(StorageClass),
    Qualifier(TypeQualifier),
    Function(FunctionSpecifier),
    Type(TypeSpecifier),
}

/// Parse declaration specifiers and keep the original sequence information
/// grouped by category for later semantic validation.
fn decl_spec_parser<'tokens, I>()
-> impl Parser<'tokens, I, DeclSpec, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let storage = choice((
        just(TokenKind::Extern).to(StorageClass::Extern),
        just(TokenKind::Register).to(StorageClass::Register),
        just(TokenKind::Static).to(StorageClass::Static),
        just(TokenKind::Typedef).to(StorageClass::Typedef),
    ))
    .map(DeclSpecifierPiece::Storage);

    let qualifiers = type_qualifier_parser().map(DeclSpecifierPiece::Qualifier);

    let function_specifier = just(TokenKind::Inline)
        .to(FunctionSpecifier::Inline)
        .map(DeclSpecifierPiece::Function);

    let ty = choice((
        just(TokenKind::Void).to(TypeSpecifier::Void),
        just(TokenKind::Char).to(TypeSpecifier::Char),
        just(TokenKind::Short).to(TypeSpecifier::Short),
        just(TokenKind::Int).to(TypeSpecifier::Int),
        just(TokenKind::Long).to(TypeSpecifier::Long),
        just(TokenKind::Float).to(TypeSpecifier::Float),
        just(TokenKind::Double).to(TypeSpecifier::Double),
        just(TokenKind::Signed).to(TypeSpecifier::Signed),
        just(TokenKind::Unsigned).to(TypeSpecifier::Unsigned),
    ))
    .map(DeclSpecifierPiece::Type);

    choice((storage, qualifiers, function_specifier, ty))
        .repeated()
        .at_least(1)
        .collect::<Vec<_>>()
        .map(|pieces| {
            let mut specifiers = DeclSpec {
                storage: Vec::new(),
                qualifiers: Vec::new(),
                function: Vec::new(),
                ty: Vec::new(),
            };

            for piece in pieces {
                match piece {
                    DeclSpecifierPiece::Storage(storage) => specifiers.storage.push(storage),
                    DeclSpecifierPiece::Qualifier(qualifier) => {
                        specifiers.qualifiers.push(qualifier)
                    }
                    DeclSpecifierPiece::Function(function_specifier) => {
                        specifiers.function.push(function_specifier)
                    }
                    DeclSpecifierPiece::Type(ty) => specifiers.ty.push(ty),
                }
            }

            specifiers
        })
        .labelled(ParserLabel::DeclarationSpecifier.as_str())
}

/// `(void)` means an empty prototype parameter list in C.
fn is_void_parameter_decl(param: &ParameterDecl) -> bool {
    param.declarator.is_none()
        && param.specifiers.storage.is_empty()
        && param.specifiers.qualifiers.is_empty()
        && param.specifiers.function.is_empty()
        && param.specifiers.ty == vec![TypeSpecifier::Void]
}

/// Parse an optional parameter declarator:
/// - `x`
/// - `*p`
/// - omitted name for forms like `int f(int, char *)`.
///
/// Returns:
/// - `None` when there is no declarator at all (e.g. parameter is just `int`).
/// - `Some(Declarator { direct: Abstract, .. })` for unnamed abstract forms like `char *`.
fn parameter_declarator_parser<'tokens, I>()
-> impl Parser<'tokens, I, Option<Declarator>, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let array_size = assignment_expression_parser()
        .map(ArraySize::Expr)
        .or_not()
        .map(|size| size.unwrap_or(ArraySize::Unspecified));

    let array_suffixes = array_size
        .delimited_by(just(TokenKind::LBracket), just(TokenKind::RBracket))
        .repeated()
        .collect::<Vec<_>>();

    pointer_layers_parser()
        .then(
            select! {
                TokenKind::Identifier(name) => name,
            }
            .or_not(),
        )
        .then(array_suffixes)
        .map(|((pointers, name), suffixes)| {
            let direct_base = match name {
                Some(name) => Some(DirectDeclarator::Ident(name)),
                None if !suffixes.is_empty() || !pointers.is_empty() => {
                    Some(DirectDeclarator::Abstract)
                }
                None => None,
            };

            direct_base.map(|base| {
                let direct =
                    suffixes
                        .into_iter()
                        .fold(base, |inner, size| DirectDeclarator::Array {
                            inner: Box::new(inner),
                            qualifiers: Vec::new(),
                            is_static: false,
                            size: Box::new(size),
                        });

                Declarator {
                    pointers,
                    direct: Box::new(direct),
                }
            })
        })
}

/// Parse function parameter list forms:
/// - `()` as old-style non-prototype
/// - `(void)` as empty prototype
/// - `(int x, char *p)` as named prototype
fn function_params_parser<'tokens, I>()
-> impl Parser<'tokens, I, FunctionParams, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let parameter =
        decl_spec_parser()
            .then(parameter_declarator_parser())
            .map(|(specifiers, declarator)| ParameterDecl {
                specifiers,
                declarator: declarator.map(Box::new),
            });

    parameter
        .separated_by(just(TokenKind::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .or_not()
        .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen))
        .map(|params| match params {
            None => FunctionParams::NonPrototype,
            Some(params) if params.len() == 1 && is_void_parameter_decl(&params[0]) => {
                FunctionParams::Prototype {
                    params: Vec::new(),
                    variadic: false,
                }
            }
            Some(params) => FunctionParams::Prototype {
                params,
                variadic: false,
            },
        })
}

#[derive(Clone)]
enum DirectDeclaratorSuffix {
    /// Function suffix: `(params...)`
    Function(FunctionParams),
    /// Array suffix: `[...]`
    Array(ArraySize),
}

/// Parse one direct-declarator suffix.
///
/// This parser is intentionally suffix-only so declarator parsing can build:
/// `base-ident + suffix*` and fold left-to-right.
fn direct_declarator_suffix_parser<'tokens, I>()
-> impl Parser<'tokens, I, DirectDeclaratorSuffix, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let function_suffix = function_params_parser().map(DirectDeclaratorSuffix::Function);

    // Current minimal array-size support:
    // `[e]` -> expression size
    // `[]`  -> unspecified size
    //
    // `[*]` (VLA marker in prototype scope) is intentionally unsupported.
    let array_size = assignment_expression_parser()
        .map(ArraySize::Expr)
        .or_not()
        .map(|size| size.unwrap_or(ArraySize::Unspecified));

    let array_suffix = array_size
        .delimited_by(just(TokenKind::LBracket), just(TokenKind::RBracket))
        .map(DirectDeclaratorSuffix::Array);

    choice((function_suffix, array_suffix))
}

/// Parse the currently supported declarator subset:
/// zero-or-more pointer stars, then identifier and postfix direct-declarator suffixes.
fn declarator_parser<'tokens, I>()
-> impl Parser<'tokens, I, Declarator, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    recursive(|declarator| {
        let direct_ident = select! {
            TokenKind::Identifier(name) => DirectDeclarator::Ident(name),
        }
        .labelled(ParserLabel::IdentifierDeclarator.as_str());

        let direct_grouped = declarator
            .clone()
            .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen))
            .map(|decl| DirectDeclarator::Grouped(Box::new(decl)));

        let direct_base = choice((direct_ident, direct_grouped));

        let direct = direct_base
            .then(
                direct_declarator_suffix_parser()
                    .repeated()
                    .collect::<Vec<_>>(),
            )
            .map(|(base, suffixes)| {
                suffixes
                    .into_iter()
                    .fold(base, |inner, suffix| match suffix {
                        DirectDeclaratorSuffix::Function(params) => DirectDeclarator::Function {
                            inner: Box::new(inner),
                            params,
                        },
                        DirectDeclaratorSuffix::Array(size) => DirectDeclarator::Array {
                            inner: Box::new(inner),
                            qualifiers: Vec::new(),
                            is_static: false,
                            size: Box::new(size),
                        },
                    })
            });

        pointer_layers_parser()
            .then(direct)
            .map(|(pointers, direct)| Declarator {
                pointers,
                direct: Box::new(direct),
            })
    })
}

/// Parse scalar initializer syntax: `= assignment-expression`.
fn initializer_parser<'tokens, I>()
-> impl Parser<'tokens, I, Initializer, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    assignment_expression_parser().map(|expr| Initializer {
        kind: InitializerKind::Expr(expr),
    })
}

/// Parse a declaration statement ending with `;`.
fn declaration_parser<'tokens, I>()
-> impl Parser<'tokens, I, Declaration, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let init_declarator = declarator_parser()
        .then(
            just(TokenKind::Assign)
                .ignore_then(initializer_parser())
                .or_not(),
        )
        .map(|(declarator, init)| InitDeclarator { declarator, init });

    decl_spec_parser()
        .then(
            init_declarator
                .separated_by(just(TokenKind::Comma))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(just(TokenKind::Semicolon))
        .map(|(specifiers, declarators)| Declaration {
            specifiers,
            declarators,
        })
        .labelled(ParserLabel::Declaration.as_str())
}

// ============================
// Statement parsing
// ============================

/// Build a `block-item` parser from an existing statement parser.
///
/// A block item is either:
/// - a declaration, or
/// - a statement.
fn block_item_with_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, BlockItem, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone,
{
    choice((
        declaration_parser().map(BlockItem::Decl),
        statement.map(BlockItem::Stmt),
    ))
    .labelled(ParserLabel::BlockItem.as_str())
}

/// Parse a compound statement (`{ ... }`) from a statement parser.
fn compound_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone,
{
    block_item_with_statement_parser(statement)
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(TokenKind::LBrace), just(TokenKind::RBrace))
        .map(|items| Stmt::Compound(CompoundStmt { items }))
        .labelled(ParserLabel::CompoundStatement.as_str())
}

/// Parse `for` initializer clause:
/// - declaration form: `int i = 0;`
/// - expression form: `i = 0;` or empty `;`
fn for_init_parser<'tokens, I>()
-> impl Parser<'tokens, I, Option<ForInit>, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
        declaration_parser().map(|decl| Some(ForInit::Decl(decl))),
        expr_parser::<'tokens, I, true>()
            .or_not()
            .then_ignore(just(TokenKind::Semicolon))
            .map(|expr| expr.map(ForInit::Expr)),
    ))
}

/// Parse an expression statement: either `;` or `expression;`.
fn expression_statement_parser<'tokens, I>()
-> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    expr_parser::<'tokens, I, true>()
        .or_not()
        .then_ignore(just(TokenKind::Semicolon))
        .map(|expr| match expr {
            Some(expr) => Stmt::Expr(expr),
            None => Stmt::Empty,
        })
        .labelled(ParserLabel::ExpressionStatement.as_str())
}

/// Parse `return;` and `return expr;`.
fn return_statement_parser<'tokens, I>()
-> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    just(TokenKind::Return)
        .ignore_then(expr_parser::<'tokens, I, true>().or_not())
        .then_ignore(just(TokenKind::Semicolon))
        .map(Stmt::Return)
        .labelled(ParserLabel::ReturnStatement.as_str())
}

/// Parse `if (cond) stmt` with optional `else stmt`.
fn if_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone,
{
    just(TokenKind::If)
        .ignore_then(
            expr_parser::<'tokens, I, true>()
                .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
        )
        .then(statement.clone())
        .then(just(TokenKind::Else).ignore_then(statement).or_not())
        .map(|((cond, then_branch), else_branch)| Stmt::If {
            cond,
            then_branch: Box::new(then_branch),
            else_branch: else_branch.map(Box::new),
        })
        .labelled(ParserLabel::IfStatement.as_str())
}

/// Parse `while (cond) stmt`.
fn while_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone,
{
    just(TokenKind::While)
        .ignore_then(
            expr_parser::<'tokens, I, true>()
                .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
        )
        .then(statement)
        .map(|(cond, body)| Stmt::While {
            cond,
            body: Box::new(body),
        })
        .labelled(ParserLabel::WhileStatement.as_str())
}

/// Parse `do stmt while (cond);`.
fn do_while_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone,
{
    just(TokenKind::Do)
        .ignore_then(statement)
        .then_ignore(just(TokenKind::While))
        .then(
            expr_parser::<'tokens, I, true>()
                .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
        )
        .then_ignore(just(TokenKind::Semicolon))
        .map(|(body, cond)| Stmt::DoWhile {
            body: Box::new(body),
            cond,
        })
        .labelled(ParserLabel::DoWhileStatement.as_str())
}

/// Parse `for (init; cond; step) stmt`.
fn for_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone,
{
    just(TokenKind::For)
        .ignore_then(
            for_init_parser()
                .then(expr_parser::<'tokens, I, true>().or_not())
                .then_ignore(just(TokenKind::Semicolon))
                .then(expr_parser::<'tokens, I, true>().or_not())
                .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
        )
        .then(statement)
        .map(|(((init, cond), step), body)| Stmt::For {
            init,
            cond,
            step,
            body: Box::new(body),
        })
        .labelled(ParserLabel::ForStatement.as_str())
}

/// Parse `switch (expr) stmt`.
fn switch_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone,
{
    just(TokenKind::Switch)
        .ignore_then(
            expr_parser::<'tokens, I, true>()
                .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
        )
        .then(statement)
        .map(|(expr, body)| Stmt::Switch {
            expr,
            body: Box::new(body),
        })
        .labelled(ParserLabel::SwitchStatement.as_str())
}

/// Parse `case expr: stmt`.
fn case_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone,
{
    just(TokenKind::Case)
        .ignore_then(expr_parser::<'tokens, I, true>())
        .then_ignore(just(TokenKind::Colon))
        .then(statement)
        .map(|(expr, stmt)| Stmt::Case {
            expr,
            stmt: Box::new(stmt),
        })
        .labelled(ParserLabel::CaseStatement.as_str())
}

/// Parse `default: stmt`.
fn default_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone,
{
    just(TokenKind::Default)
        .ignore_then(just(TokenKind::Colon))
        .ignore_then(statement)
        .map(|stmt| Stmt::Default {
            stmt: Box::new(stmt),
        })
        .labelled(ParserLabel::DefaultStatement.as_str())
}

/// Parse `label: stmt`.
fn label_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone,
{
    select! {
        TokenKind::Identifier(name) => name,
    }
    .then_ignore(just(TokenKind::Colon))
    .then(statement)
    .map(|(label, stmt)| Stmt::Label {
        label,
        stmt: Box::new(stmt),
    })
    .labelled(ParserLabel::LabelStatement.as_str())
}

/// Parse `break;`.
fn break_statement_parser<'tokens, I>()
-> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    just(TokenKind::Break)
        .then_ignore(just(TokenKind::Semicolon))
        .to(Stmt::Break)
        .labelled(ParserLabel::BreakStatement.as_str())
}

/// Parse `continue;`.
fn continue_statement_parser<'tokens, I>()
-> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    just(TokenKind::Continue)
        .then_ignore(just(TokenKind::Semicolon))
        .to(Stmt::Continue)
        .labelled(ParserLabel::ContinueStatement.as_str())
}

/// Parse `goto identifier;`.
fn goto_statement_parser<'tokens, I>()
-> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    just(TokenKind::Goto)
        .ignore_then(select! {
            TokenKind::Identifier(name) => name,
        })
        .then_ignore(just(TokenKind::Semicolon))
        .map(Stmt::Goto)
        .labelled(ParserLabel::GotoStatement.as_str())
}

/// Parse the currently supported statement subset.
fn statement_parser<'tokens, I>()
-> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    recursive(|statement| {
        let compound_stmt = compound_statement_parser(statement.clone());

        choice((
            compound_stmt,
            return_statement_parser(),
            if_statement_parser(statement.clone()),
            while_statement_parser(statement.clone()),
            do_while_statement_parser(statement.clone()),
            for_statement_parser(statement.clone()),
            switch_statement_parser(statement.clone()),
            case_statement_parser(statement.clone()),
            default_statement_parser(statement.clone()),
            // Must appear before expression statement to parse `label: ...` correctly.
            label_statement_parser(statement.clone()),
            goto_statement_parser(),
            break_statement_parser(),
            continue_statement_parser(),
            expression_statement_parser(),
        ))
    })
    .labelled(ParserLabel::Statement.as_str())
}

/// Parse one block item as either a declaration or a statement.
fn block_item_parser<'tokens, I>()
-> impl Parser<'tokens, I, BlockItem, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    block_item_with_statement_parser(statement_parser())
}

// ============================
// Translation unit parsing
// ============================

fn function_definition_parser<'tokens, I>()
-> impl Parser<'tokens, I, FunctionDef, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    decl_spec_parser()
        .then(declarator_parser())
        // Lookahead: only treat as function definition when a body starts here.
        // The leading `{` is preserved for `statement_parser`.
        .then_ignore(just(TokenKind::LBrace).rewind())
        .then(statement_parser())
        .map(|((specifiers, declarator), stmt)| {
            let Stmt::Compound(body) = stmt else {
                unreachable!("function body must start with '{{'");
            };

            FunctionDef {
                specifiers,
                declarator,
                declarations: Vec::new(),
                body,
            }
        })
}

/// Parse the whole translation unit as a sequence of external declarations.
fn parser<'tokens, I>()
-> impl Parser<'tokens, I, TranslationUnit, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    // Try function definition first, then declaration.
    // Both start with declaration-specifiers + declarator, so order matters.
    choice((
        function_definition_parser().map(ExternalDecl::FunctionDef),
        declaration_parser().map(ExternalDecl::Declaration),
    ))
    .repeated()
    .collect::<Vec<_>>()
    .then_ignore(end())
    .map(|items| TranslationUnit { items })
}

/// Entry point for parser consumers.
pub fn parse<'tokens, I>(input: I) -> Result<TranslationUnit, Vec<ParseError<'tokens>>>
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    parser().parse(input).into_result()
}

/// Parse a single statement from input.
pub fn parse_statement<'tokens, I>(input: I) -> Result<Stmt, Vec<ParseError<'tokens>>>
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    statement_parser()
        .then_ignore(end())
        .parse(input)
        .into_result()
}

/// Parse a single block item from input.
pub fn parse_block_item<'tokens, I>(input: I) -> Result<BlockItem, Vec<ParseError<'tokens>>>
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    block_item_parser()
        .then_ignore(end())
        .parse(input)
        .into_result()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::lexer::lexer_from_source;
    use crate::frontend::parser::ast::{ExprKind, Literal};
    use chumsky::input::{Input, Stream};

    fn parse_source(src: &str) -> TranslationUnit {
        let tokens = lexer_from_source(src);
        let stream = Stream::from_iter(tokens)
            .map((src.len()..src.len()).into(), |(token, span)| (token, span));

        parse(stream).expect("source should parse")
    }

    fn parse_statement_source(src: &str) -> Stmt {
        let tokens = lexer_from_source(src);
        let stream = Stream::from_iter(tokens)
            .map((src.len()..src.len()).into(), |(token, span)| (token, span));

        parse_statement(stream).expect("statement should parse")
    }

    fn parse_block_item_source(src: &str) -> BlockItem {
        let tokens = lexer_from_source(src);
        let stream = Stream::from_iter(tokens)
            .map((src.len()..src.len()).into(), |(token, span)| (token, span));

        parse_block_item(stream).expect("block item should parse")
    }

    fn assert_direct_ident(direct: &DirectDeclarator, expected: &str) {
        match direct {
            DirectDeclarator::Ident(name) => assert_eq!(name, expected),
            other => panic!("expected identifier declarator, got {other:?}"),
        }
    }

    fn assert_ident_declarator(init_declarator: &InitDeclarator, expected: &str) {
        assert_direct_ident(init_declarator.declarator.direct.as_ref(), expected);
    }

    #[test]
    fn parses_single_int_declaration() {
        let unit = parse_source("int a;");
        assert_eq!(unit.items.len(), 1);

        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
        assert!(decl.specifiers.storage.is_empty());
        assert!(decl.specifiers.qualifiers.is_empty());
        assert_eq!(decl.declarators.len(), 1);
        assert_ident_declarator(&decl.declarators[0], "a");
        assert!(decl.declarators[0].init.is_none());
    }

    #[test]
    fn parses_declaration_with_initializer_and_multiple_declarators() {
        let unit = parse_source("int a = 1, b;");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
        assert_eq!(decl.declarators.len(), 2);

        assert_ident_declarator(&decl.declarators[0], "a");
        let Some(init) = decl.declarators[0].init.as_ref() else {
            panic!("first declarator should contain initializer");
        };
        assert_eq!(init.kind, InitializerKind::Expr(Expr::int(1)));

        assert_ident_declarator(&decl.declarators[1], "b");
        assert!(decl.declarators[1].init.is_none());
    }

    #[test]
    fn parses_static_storage_class_declaration() {
        let unit = parse_source("static int x, y;");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        assert_eq!(decl.specifiers.storage, vec![StorageClass::Static]);
        assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
        assert_eq!(decl.declarators.len(), 2);
        assert_ident_declarator(&decl.declarators[0], "x");
        assert_ident_declarator(&decl.declarators[1], "y");
    }

    #[test]
    fn parses_const_double_declaration() {
        let unit = parse_source("const double pi = 3.14;");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        assert_eq!(decl.specifiers.qualifiers, vec![TypeQualifier::Const]);
        assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Double]);
        assert_eq!(decl.declarators.len(), 1);
        assert_ident_declarator(&decl.declarators[0], "pi");

        let Some(init) = decl.declarators[0].init.as_ref() else {
            panic!("pi should contain initializer");
        };
        match &init.kind {
            InitializerKind::Expr(Expr {
                kind: ExprKind::Literal(Literal::Float(value)),
            }) => assert!((*value - 3.14).abs() < f64::EPSILON),
            other => panic!("expected float initializer, got {other:?}"),
        }
    }

    #[test]
    fn parses_parenthesized_comma_expression_initializer() {
        let unit = parse_source("int value = (1, 2);");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        let Some(init) = decl.declarators[0].init.as_ref() else {
            panic!("initializer expected");
        };

        match &init.kind {
            InitializerKind::Expr(Expr {
                kind: ExprKind::Comma { left, right },
            }) => {
                assert_eq!(**left, Expr::int(1));
                assert_eq!(**right, Expr::int(2));
            }
            other => panic!("expected comma expression initializer, got {other:?}"),
        }
    }

    #[test]
    fn parses_array_declaration_with_constant_size() {
        let unit = parse_source("int arr[10];");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        let direct = decl.declarators[0].declarator.direct.as_ref();
        let DirectDeclarator::Array {
            inner,
            qualifiers,
            is_static,
            size,
        } = direct
        else {
            panic!("expected array declarator");
        };
        assert!(qualifiers.is_empty());
        assert!(!is_static);
        assert_eq!(size.as_ref(), &ArraySize::Expr(Expr::int(10)));
        assert_direct_ident(inner.as_ref(), "arr");
    }

    #[test]
    fn parses_multi_dimensional_array_declaration() {
        let unit = parse_source("int matrix[2][3];");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        let direct = decl.declarators[0].declarator.direct.as_ref();
        let DirectDeclarator::Array {
            inner: outer_inner,
            size: outer_size,
            ..
        } = direct
        else {
            panic!("expected outer array declarator");
        };
        assert_eq!(outer_size.as_ref(), &ArraySize::Expr(Expr::int(3)));

        let DirectDeclarator::Array {
            inner: inner_inner,
            size: inner_size,
            ..
        } = outer_inner.as_ref()
        else {
            panic!("expected inner array declarator");
        };
        assert_eq!(inner_size.as_ref(), &ArraySize::Expr(Expr::int(2)));
        assert_direct_ident(inner_inner.as_ref(), "matrix");
    }

    #[test]
    fn rejects_vla_marker_array_declaration() {
        let src = "int arr[*];";
        let tokens = lexer_from_source(src);
        let stream = Stream::from_iter(tokens)
            .map((src.len()..src.len()).into(), |(token, span)| (token, span));

        let errors = parse(stream).expect_err("VLA marker should be rejected");
        assert!(
            !errors.is_empty(),
            "expected at least one parser error for VLA marker syntax"
        );
    }

    #[test]
    fn parses_empty_expression_statement() {
        let stmt = parse_statement_source(";");
        assert_eq!(stmt, Stmt::Empty);
    }

    #[test]
    fn parses_non_empty_expression_statement() {
        let stmt = parse_statement_source("a = 1 + 2;");
        assert_eq!(
            stmt,
            Stmt::Expr(Expr::assign(
                Expr::var("a".to_string()),
                AssignOp::Assign,
                Expr::binary(Expr::int(1), BinaryOp::Add, Expr::int(2)),
            ))
        );
    }

    #[test]
    fn parses_declaration_block_item() {
        let item = parse_block_item_source("int counter;");
        let BlockItem::Decl(decl) = item else {
            panic!("expected declaration block item");
        };

        assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
        assert_eq!(decl.declarators.len(), 1);
        assert_ident_declarator(&decl.declarators[0], "counter");
    }

    #[test]
    fn parses_statement_block_item() {
        let item = parse_block_item_source("counter++;");
        assert_eq!(
            item,
            BlockItem::Stmt(Stmt::Expr(Expr::post_inc(Expr::var("counter".to_string()))))
        );
    }

    #[test]
    fn parses_function_declaration_with_void_params() {
        let unit = parse_source("int main(void);");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        let direct = decl.declarators[0].declarator.direct.as_ref();
        let DirectDeclarator::Function { inner, params } = direct else {
            panic!("expected function declarator");
        };
        assert_direct_ident(inner.as_ref(), "main");
        assert_eq!(
            params,
            &FunctionParams::Prototype {
                params: Vec::new(),
                variadic: false
            }
        );
    }

    #[test]
    fn parses_function_declaration_with_named_params() {
        let unit = parse_source("int sum(int x, char *p);");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        let direct = decl.declarators[0].declarator.direct.as_ref();
        let DirectDeclarator::Function { inner, params } = direct else {
            panic!("expected function declarator");
        };
        assert_direct_ident(inner.as_ref(), "sum");

        let FunctionParams::Prototype { params, variadic } = params else {
            panic!("expected prototype params");
        };
        assert!(!variadic);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
        assert_eq!(params[1].specifiers.ty, vec![TypeSpecifier::Char]);

        let first = params[0]
            .declarator
            .as_ref()
            .expect("first parameter should have declarator");
        assert_direct_ident(first.direct.as_ref(), "x");

        let second = params[1]
            .declarator
            .as_ref()
            .expect("second parameter should have declarator");
        assert_eq!(second.pointers.len(), 1);
        assert_direct_ident(second.direct.as_ref(), "p");
    }

    #[test]
    fn parses_function_declaration_with_array_param() {
        let unit = parse_source("int f(int a[]);");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        let direct = decl.declarators[0].declarator.direct.as_ref();
        let DirectDeclarator::Function { params, .. } = direct else {
            panic!("expected function declarator");
        };

        let FunctionParams::Prototype { params, variadic } = params else {
            panic!("expected prototype params");
        };
        assert!(!variadic);
        assert_eq!(params.len(), 1);

        let param_decl = params[0]
            .declarator
            .as_ref()
            .expect("parameter declarator expected");

        let DirectDeclarator::Array { inner, size, .. } = param_decl.direct.as_ref() else {
            panic!("expected array declarator for parameter");
        };
        assert_eq!(size.as_ref(), &ArraySize::Unspecified);
        assert_direct_ident(inner.as_ref(), "a");
    }

    #[test]
    fn parses_function_declaration_with_const_char_pointer_array_param() {
        let unit = parse_source("void p(const char *strings[], int count);");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        let direct = decl.declarators[0].declarator.direct.as_ref();
        let DirectDeclarator::Function { inner, params } = direct else {
            panic!("expected function declarator");
        };
        assert_direct_ident(inner.as_ref(), "p");

        let FunctionParams::Prototype { params, variadic } = params else {
            panic!("expected prototype params");
        };
        assert!(!variadic);
        assert_eq!(params.len(), 2);

        assert_eq!(params[0].specifiers.qualifiers, vec![TypeQualifier::Const]);
        assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Char]);

        let first = params[0]
            .declarator
            .as_ref()
            .expect("first parameter declarator expected");
        assert_eq!(first.pointers.len(), 1);

        let DirectDeclarator::Array { inner, size, .. } = first.direct.as_ref() else {
            panic!("expected array declarator for first parameter");
        };
        assert_eq!(size.as_ref(), &ArraySize::Unspecified);
        assert_direct_ident(inner.as_ref(), "strings");

        assert_eq!(params[1].specifiers.ty, vec![TypeSpecifier::Int]);
        let second = params[1]
            .declarator
            .as_ref()
            .expect("second parameter declarator expected");
        assert_direct_ident(second.direct.as_ref(), "count");
    }

    #[test]
    fn parses_grouped_function_pointer_declaration() {
        let unit = parse_source("int (*fp)(int);");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        let direct = decl.declarators[0].declarator.direct.as_ref();
        let DirectDeclarator::Function { inner, params } = direct else {
            panic!("expected function declarator");
        };

        let DirectDeclarator::Grouped(grouped_decl) = inner.as_ref() else {
            panic!("expected grouped declarator as function inner");
        };
        assert_eq!(grouped_decl.pointers.len(), 1);
        assert_direct_ident(grouped_decl.direct.as_ref(), "fp");

        let FunctionParams::Prototype { params, variadic } = params else {
            panic!("expected prototype parameters");
        };
        assert!(!variadic);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
    }

    #[test]
    fn preserves_pointer_layers_for_unnamed_parameter_declarator() {
        let unit = parse_source("int sum(int, char *);");
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration item");
        };

        let direct = decl.declarators[0].declarator.direct.as_ref();
        let DirectDeclarator::Function { params, .. } = direct else {
            panic!("expected function declarator");
        };

        let FunctionParams::Prototype { params, variadic } = params else {
            panic!("expected prototype params");
        };
        assert!(!variadic);
        assert_eq!(params.len(), 2);

        assert!(
            params[0].declarator.is_none(),
            "plain unnamed `int` parameter should have no declarator"
        );

        let second = params[1]
            .declarator
            .as_ref()
            .expect("unnamed `char *` should keep declarator");
        assert_eq!(second.pointers.len(), 1);
        assert_eq!(second.direct.as_ref(), &DirectDeclarator::Abstract);
    }

    #[test]
    fn parses_function_definition_with_compound_body() {
        let unit = parse_source("int main(void) { return 0; }");
        let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
            panic!("expected function definition");
        };

        assert_eq!(def.specifiers.ty, vec![TypeSpecifier::Int]);
        let DirectDeclarator::Function { inner, .. } = def.declarator.direct.as_ref() else {
            panic!("expected function declarator");
        };
        assert_direct_ident(inner.as_ref(), "main");
        assert!(def.declarations.is_empty());
        assert_eq!(def.body.items.len(), 1);
        assert_eq!(
            def.body.items[0],
            BlockItem::Stmt(Stmt::Return(Some(Expr::int(0))))
        );
    }

    #[test]
    fn parses_compound_statement_with_decl_and_expr_stmt() {
        let stmt = parse_statement_source("{ int x; x = 1; }");
        let Stmt::Compound(compound) = stmt else {
            panic!("expected compound statement");
        };

        assert_eq!(compound.items.len(), 2);
        let BlockItem::Decl(decl) = &compound.items[0] else {
            panic!("first item should be declaration");
        };
        assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
        assert_ident_declarator(&decl.declarators[0], "x");

        assert_eq!(
            compound.items[1],
            BlockItem::Stmt(Stmt::Expr(Expr::assign(
                Expr::var("x".to_string()),
                AssignOp::Assign,
                Expr::int(1),
            )))
        );
    }

    #[test]
    fn parses_return_statements() {
        assert_eq!(parse_statement_source("return;"), Stmt::Return(None));
        assert_eq!(
            parse_statement_source("return x + 1;"),
            Stmt::Return(Some(Expr::binary(
                Expr::var("x".to_string()),
                BinaryOp::Add,
                Expr::int(1),
            )))
        );
    }

    #[test]
    fn parses_if_else_statement() {
        let stmt = parse_statement_source("if (flag) x = 1; else x = 2;");
        assert_eq!(
            stmt,
            Stmt::If {
                cond: Expr::var("flag".to_string()),
                then_branch: Box::new(Stmt::Expr(Expr::assign(
                    Expr::var("x".to_string()),
                    AssignOp::Assign,
                    Expr::int(1),
                ))),
                else_branch: Some(Box::new(Stmt::Expr(Expr::assign(
                    Expr::var("x".to_string()),
                    AssignOp::Assign,
                    Expr::int(2),
                )))),
            }
        );
    }

    #[test]
    fn parses_while_statement() {
        let stmt = parse_statement_source("while (x < 10) x++;");
        assert_eq!(
            stmt,
            Stmt::While {
                cond: Expr::binary(Expr::var("x".to_string()), BinaryOp::Lt, Expr::int(10)),
                body: Box::new(Stmt::Expr(Expr::post_inc(Expr::var("x".to_string())))),
            }
        );
    }

    #[test]
    fn parses_do_while_statement() {
        let stmt = parse_statement_source("do x++; while (x < 10);");
        assert_eq!(
            stmt,
            Stmt::DoWhile {
                body: Box::new(Stmt::Expr(Expr::post_inc(Expr::var("x".to_string())))),
                cond: Expr::binary(Expr::var("x".to_string()), BinaryOp::Lt, Expr::int(10)),
            }
        );
    }

    #[test]
    fn parses_switch_statement() {
        let stmt = parse_statement_source("switch (x) break;");
        assert_eq!(
            stmt,
            Stmt::Switch {
                expr: Expr::var("x".to_string()),
                body: Box::new(Stmt::Break),
            }
        );
    }

    #[test]
    fn parses_case_statement() {
        let stmt = parse_statement_source("case 1: break;");
        assert_eq!(
            stmt,
            Stmt::Case {
                expr: Expr::int(1),
                stmt: Box::new(Stmt::Break),
            }
        );
    }

    #[test]
    fn parses_default_statement() {
        let stmt = parse_statement_source("default: continue;");
        assert_eq!(
            stmt,
            Stmt::Default {
                stmt: Box::new(Stmt::Continue),
            }
        );
    }

    #[test]
    fn parses_label_statement() {
        let stmt = parse_statement_source("entry: x = 1;");
        assert_eq!(
            stmt,
            Stmt::Label {
                label: "entry".to_string(),
                stmt: Box::new(Stmt::Expr(Expr::assign(
                    Expr::var("x".to_string()),
                    AssignOp::Assign,
                    Expr::int(1),
                ))),
            }
        );
    }

    #[test]
    fn parses_goto_statement() {
        let stmt = parse_statement_source("goto entry;");
        assert_eq!(stmt, Stmt::Goto("entry".to_string()));
    }

    #[test]
    fn parses_for_statement_with_expression_init() {
        let stmt = parse_statement_source("for (i = 0; i < 10; i++) i;");
        assert_eq!(
            stmt,
            Stmt::For {
                init: Some(ForInit::Expr(Expr::assign(
                    Expr::var("i".to_string()),
                    AssignOp::Assign,
                    Expr::int(0),
                ))),
                cond: Some(Expr::binary(
                    Expr::var("i".to_string()),
                    BinaryOp::Lt,
                    Expr::int(10),
                )),
                step: Some(Expr::post_inc(Expr::var("i".to_string()))),
                body: Box::new(Stmt::Expr(Expr::var("i".to_string()))),
            }
        );
    }

    #[test]
    fn parses_for_statement_with_declaration_init() {
        let stmt = parse_statement_source("for (int i = 0; i < 3; i++) ;");
        let Stmt::For {
            init,
            cond,
            step,
            body,
        } = stmt
        else {
            panic!("expected for statement");
        };

        let Some(ForInit::Decl(decl)) = init else {
            panic!("for init should be declaration");
        };
        assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
        assert_eq!(decl.declarators.len(), 1);
        assert_ident_declarator(&decl.declarators[0], "i");
        assert_eq!(
            decl.declarators[0]
                .init
                .as_ref()
                .expect("initializer expected")
                .kind,
            InitializerKind::Expr(Expr::int(0))
        );

        assert_eq!(
            cond,
            Some(Expr::binary(
                Expr::var("i".to_string()),
                BinaryOp::Lt,
                Expr::int(3),
            ))
        );
        assert_eq!(step, Some(Expr::post_inc(Expr::var("i".to_string()))));
        assert_eq!(*body, Stmt::Empty);
    }

    #[test]
    fn parses_function_call_expression_statement() {
        let stmt = parse_statement_source("result = add(1, 2);");
        assert_eq!(
            stmt,
            Stmt::Expr(Expr::assign(
                Expr::var("result".to_string()),
                AssignOp::Assign,
                Expr::call(
                    Expr::var("add".to_string()),
                    vec![Expr::int(1), Expr::int(2)]
                ),
            ))
        );
    }

    #[test]
    fn parses_empty_argument_function_call() {
        let stmt = parse_statement_source("result = get();");
        assert_eq!(
            stmt,
            Stmt::Expr(Expr::assign(
                Expr::var("result".to_string()),
                AssignOp::Assign,
                Expr::call(Expr::var("get".to_string()), vec![]),
            ))
        );
    }

    #[test]
    fn parses_chained_function_call() {
        let stmt = parse_statement_source("result = factory()(42);");
        assert_eq!(
            stmt,
            Stmt::Expr(Expr::assign(
                Expr::var("result".to_string()),
                AssignOp::Assign,
                Expr::call(
                    Expr::call(Expr::var("factory".to_string()), vec![]),
                    vec![Expr::int(42)],
                ),
            ))
        );
    }

    #[test]
    fn parses_grouped_comma_expression_as_single_call_argument() {
        let stmt = parse_statement_source("result = f((1, 2));");
        assert_eq!(
            stmt,
            Stmt::Expr(Expr::assign(
                Expr::var("result".to_string()),
                AssignOp::Assign,
                Expr::call(
                    Expr::var("f".to_string()),
                    vec![Expr::comma(Expr::int(1), Expr::int(2))],
                ),
            ))
        );
    }

    #[test]
    fn parses_array_subscript_expression_statement() {
        let stmt = parse_statement_source("value = arr[i + 1];");
        assert_eq!(
            stmt,
            Stmt::Expr(Expr::assign(
                Expr::var("value".to_string()),
                AssignOp::Assign,
                Expr::index(
                    Expr::var("arr".to_string()),
                    Expr::binary(Expr::var("i".to_string()), BinaryOp::Add, Expr::int(1)),
                ),
            ))
        );
    }

    #[test]
    fn parses_member_access_expression_statement() {
        let stmt = parse_statement_source("value = point.x;");
        assert_eq!(
            stmt,
            Stmt::Expr(Expr::assign(
                Expr::var("value".to_string()),
                AssignOp::Assign,
                Expr::member(Expr::var("point".to_string()), "x".to_string(), false),
            ))
        );
    }

    #[test]
    fn parses_pointer_member_access_expression_statement() {
        let stmt = parse_statement_source("value = node->next;");
        assert_eq!(
            stmt,
            Stmt::Expr(Expr::assign(
                Expr::var("value".to_string()),
                AssignOp::Assign,
                Expr::member(Expr::var("node".to_string()), "next".to_string(), true),
            ))
        );
    }

    #[test]
    fn parses_chained_postfix_access_expression_statement() {
        let stmt = parse_statement_source("value = factory().items[i].count;");
        assert_eq!(
            stmt,
            Stmt::Expr(Expr::assign(
                Expr::var("value".to_string()),
                AssignOp::Assign,
                Expr::member(
                    Expr::index(
                        Expr::member(
                            Expr::call(Expr::var("factory".to_string()), vec![]),
                            "items".to_string(),
                            false,
                        ),
                        Expr::var("i".to_string()),
                    ),
                    "count".to_string(),
                    false,
                ),
            ))
        );
    }

    #[test]
    fn parses_break_statement() {
        assert_eq!(parse_statement_source("break;"), Stmt::Break);
    }

    #[test]
    fn parses_continue_statement() {
        assert_eq!(parse_statement_source("continue;"), Stmt::Continue);
    }
}
