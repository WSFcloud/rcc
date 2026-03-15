use crate::common::token::TokenKind;
use crate::frontend::parser::ast::{
    ArraySize, AssignOp, BinaryOp, BlockItem, CompoundStmt, DeclSpec, Declaration, Declarator,
    Designator, DirectDeclarator, EnumSpecifier, EnumVariant, Expr, ExprKind, ExternalDecl,
    ForInit, FunctionDef, FunctionParams, FunctionSpecifier, InitDeclarator, Initializer,
    InitializerItem, InitializerKind, IntLiteralSuffix, ParameterDecl, Pointer, RecordKind,
    RecordMemberDecl, RecordSpecifier, Stmt, StorageClass, TranslationUnit, TypeName,
    TypeQualifier, TypeSpecifier, UnaryOp,
};
use crate::frontend::parser::labels::ParserLabel;
use crate::frontend::parser::typedefs::{BindingKind, Typedefs};
use chumsky::{
    error::Rich,
    input::ValueInput,
    pratt::{infix, left},
    prelude::*,
    span::SimpleSpan,
};

pub type Span = SimpleSpan<usize>;
pub type ParseError<'tokens> = Rich<'tokens, TokenKind, Span>;
type ParserExtra<'tokens, I> =
    extra::Full<ParseError<'tokens>, Typedefs, std::marker::PhantomData<I>>;

// ============================
// Expression parsing
// ============================

/// Prefix operators represented before binding to a concrete RHS expression.
#[derive(Clone, Copy)]
enum PrefixExprOp {
    Unary(UnaryOp),
    PreInc,
    PreDec,
}

impl PrefixExprOp {
    /// Build the corresponding AST node for a parsed prefix operator.
    fn apply(self, rhs: Expr) -> Expr {
        match self {
            Self::Unary(op) => Expr::unary(op, rhs),
            Self::PreInc => Expr::pre_inc(rhs),
            Self::PreDec => Expr::pre_dec(rhs),
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
-> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
        select! {
            TokenKind::IntLiteral(value) => Expr::int(value),
            TokenKind::UIntLiteral(value) => Expr::int_with_base(value, IntLiteralSuffix::UInt),
            TokenKind::LongLiteral(value) => Expr::int_with_base(value, IntLiteralSuffix::Long),
            TokenKind::ULongLiteral(value) => Expr::int_with_base(value, IntLiteralSuffix::ULong),
            TokenKind::LongLongLiteral(value) => Expr::int_with_base(value, IntLiteralSuffix::LongLong),
            TokenKind::ULongLongLiteral(value) => Expr::int_with_base(value, IntLiteralSuffix::ULongLong),
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
-> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    select! {
        TokenKind::Identifier(name) => Expr::var(name),
    }
}

fn prefix_expr_op_parser<'tokens, I>()
-> impl Parser<'tokens, I, PrefixExprOp, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
        just(TokenKind::PlusPlus).to(PrefixExprOp::PreInc),
        just(TokenKind::MinusMinus).to(PrefixExprOp::PreDec),
        just(TokenKind::Plus).to(PrefixExprOp::Unary(UnaryOp::Plus)),
        just(TokenKind::Minus).to(PrefixExprOp::Unary(UnaryOp::Minus)),
        just(TokenKind::Bang).to(PrefixExprOp::Unary(UnaryOp::LogicalNot)),
        just(TokenKind::Tilde).to(PrefixExprOp::Unary(UnaryOp::BitNot)),
        just(TokenKind::Star).to(PrefixExprOp::Unary(UnaryOp::Deref)),
        just(TokenKind::Amp).to(PrefixExprOp::Unary(UnaryOp::AddressOf)),
    ))
}

fn basic_postfix_expr_op_parser<'tokens, I>()
-> impl Parser<'tokens, I, PostfixExprOp, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
        just(TokenKind::PlusPlus).to(PostfixExprOp::PostInc),
        just(TokenKind::MinusMinus).to(PostfixExprOp::PostDec),
    ))
}

fn assignment_op_parser<'tokens, I>()
-> impl Parser<'tokens, I, AssignOp, ParserExtra<'tokens, I>> + Clone
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

fn binary_pratt_expr_parser<'tokens, I, P>(
    atom: P,
) -> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
{
    atom.pratt((
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
) -> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
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
) -> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
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
) -> impl Parser<'tokens, I, PostfixExprOp, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
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
) -> impl Parser<'tokens, I, PostfixExprOp, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
{
    index_expr
        .delimited_by(just(TokenKind::LBracket), just(TokenKind::RBracket))
        .map(PostfixExprOp::Index)
}

/// Parse member access postfix operators: `.field` and `->field`.
fn member_postfix_expr_op_parser<'tokens, I>()
-> impl Parser<'tokens, I, PostfixExprOp, ParserExtra<'tokens, I>> + Clone
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

/// Parse postfix-expression from a primary-expression and repeated postfix operators.
fn postfix_expr_parser<'tokens, I, P, A>(
    primary: P,
    assignment: A,
) -> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
    A: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
{
    let subscript_expr = comma_sequence_parser(
        assignment.clone(),
        "array subscript requires one expression",
    );

    let postfix_op = choice((
        basic_postfix_expr_op_parser(),
        call_postfix_expr_op_parser(assignment),
        index_postfix_expr_op_parser(subscript_expr),
        member_postfix_expr_op_parser(),
    ));

    primary
        .then(postfix_op.repeated().collect::<Vec<_>>())
        .map(|(base, ops)| ops.into_iter().fold(base, |lhs, op| op.apply(lhs)))
}

/// Build `conditional-expression` and assignment chain on top of Pratt expressions.
fn assignment_core_expr_parser<'tokens, I, P, Q, R>(
    binary: P,
    assignment: Q,
    assign_op: R,
) -> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
    Q: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
    R: Parser<'tokens, I, AssignOp, ParserExtra<'tokens, I>> + Clone,
{
    // In `cond ? then : else`, the then-branch is an `expression`
    // (comma expressions are allowed), while else-branch is assignment-expression.
    let then_expr = comma_sequence_parser(
        assignment.clone(),
        "then-branch requires at least one expression",
    );

    let conditional = binary
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
-> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    // This parser is used by parenthesized expressions and must always allow commas.
    let mut grouped_expr = Recursive::declare();
    let assign_op = assignment_op_parser();

    let assignment = recursive(|assignment| {
        let primary = expr_atom_parser(grouped_expr.clone());

        let unary = recursive(|unary| {
            let sizeof_type = just(TokenKind::Sizeof)
                .ignore_then(
                    type_name_parser()
                        .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
                )
                .map(|ty| Expr::new(ExprKind::SizeofType(Box::new(ty))));

            let sizeof_expr = just(TokenKind::Sizeof)
                .ignore_then(unary.clone())
                .map(Expr::sizeof_expr);

            let cast = type_name_parser()
                .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen))
                .then(unary.clone())
                .map(|(ty, expr)| {
                    Expr::new(ExprKind::Cast {
                        ty: Box::new(ty),
                        expr: Box::new(expr),
                    })
                });

            let prefix = prefix_expr_op_parser()
                .then(unary.clone())
                .map(|(op, rhs)| op.apply(rhs));

            let postfix = postfix_expr_parser(primary.clone(), assignment.clone());

            choice((sizeof_type, sizeof_expr, cast, prefix, postfix))
        });

        let binary = binary_pratt_expr_parser(unary);
        assignment_core_expr_parser(binary, assignment, assign_op.clone())
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
-> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
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
-> impl Parser<'tokens, I, TypeQualifier, ParserExtra<'tokens, I>> + Clone
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
-> impl Parser<'tokens, I, Pointer, ParserExtra<'tokens, I>> + Clone
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
-> impl Parser<'tokens, I, Vec<Pointer>, ParserExtra<'tokens, I>> + Clone
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

fn assemble_decl_spec(pieces: Vec<DeclSpecifierPiece>) -> DeclSpec {
    let mut specifiers = DeclSpec {
        storage: Vec::new(),
        qualifiers: Vec::new(),
        function: Vec::new(),
        ty: Vec::new(),
    };

    for piece in pieces {
        match piece {
            DeclSpecifierPiece::Storage(storage) => specifiers.storage.push(storage),
            DeclSpecifierPiece::Qualifier(qualifier) => specifiers.qualifiers.push(qualifier),
            DeclSpecifierPiece::Function(function_specifier) => {
                specifiers.function.push(function_specifier)
            }
            DeclSpecifierPiece::Type(ty) => specifiers.ty.push(ty),
        }
    }

    specifiers
}

fn typedef_name_type_specifier_parser<'tokens, I>()
-> impl Parser<'tokens, I, TypeSpecifier, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    chumsky::primitive::select(
        |token: TokenKind,
         extra: &mut chumsky::input::MapExtra<'tokens, '_, I, ParserExtra<'tokens, I>>| {
            match token {
                TokenKind::Identifier(name) if extra.state().is_typedef_name(&name) => {
                    Some(TypeSpecifier::TypedefName(name))
                }
                _ => None,
            }
        },
    )
    .boxed()
}

fn builtin_type_specifier_parser<'tokens, I>()
-> impl Parser<'tokens, I, TypeSpecifier, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    choice((
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
    .boxed()
}

fn record_or_enum_tag_ref_type_specifier_parser<'tokens, I>()
-> impl Parser<'tokens, I, TypeSpecifier, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let tag = select! {
        TokenKind::Identifier(name) => name,
    };

    let record_ref = choice((
        just(TokenKind::Struct).to(RecordKind::Struct),
        just(TokenKind::Union).to(RecordKind::Union),
    ))
    .then(tag.clone())
    .map(|(kind, tag)| {
        TypeSpecifier::StructOrUnion(RecordSpecifier {
            kind,
            tag: Some(tag),
            members: None,
        })
    });

    let enum_ref = just(TokenKind::Enum).ignore_then(tag).map(|tag| {
        TypeSpecifier::Enum(EnumSpecifier {
            tag: Some(tag),
            variants: None,
        })
    });

    choice((record_ref, enum_ref)).boxed()
}

fn bind_enum_variants(enum_spec: &EnumSpecifier, state: &mut Typedefs) {
    let Some(variants) = &enum_spec.variants else {
        return;
    };

    for variant in variants {
        state.bind(variant.name.clone(), BindingKind::Ordinary);
    }
}

fn enum_enumerator_name_parser<'tokens, I>()
-> impl Parser<'tokens, I, String, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    chumsky::primitive::select(
        |token: TokenKind,
         extra: &mut chumsky::input::MapExtra<'tokens, '_, I, ParserExtra<'tokens, I>>| {
            match token {
                TokenKind::Identifier(name)
                    if !extra.state().is_typedef_name_in_current_scope(&name) =>
                {
                    Some(name)
                }
                _ => None,
            }
        },
    )
}

fn integer_literal_expr_parser<'tokens, I>()
-> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let int_literal = select! {
        TokenKind::IntLiteral(value) => Expr::int(value),
        TokenKind::UIntLiteral(value) => Expr::int_with_base(value, IntLiteralSuffix::UInt),
        TokenKind::LongLiteral(value) => Expr::int_with_base(value, IntLiteralSuffix::Long),
        TokenKind::ULongLiteral(value) => Expr::int_with_base(value, IntLiteralSuffix::ULong),
        TokenKind::LongLongLiteral(value) => Expr::int_with_base(value, IntLiteralSuffix::LongLong),
        TokenKind::ULongLongLiteral(value) => Expr::int_with_base(value, IntLiteralSuffix::ULongLong),
    };

    choice((
        int_literal.clone(),
        just(TokenKind::Minus)
            .ignore_then(int_literal)
            .map(|expr| Expr::unary(UnaryOp::Minus, expr)),
    ))
    .boxed()
}

fn constant_expression_parser<'tokens, I>()
-> impl Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let sizeof_type_spec = {
        let qualifier_piece = type_qualifier_parser().map(DeclSpecifierPiece::Qualifier);
        let first_type_piece = choice((
            builtin_type_specifier_parser(),
            typedef_name_type_specifier_parser(),
            record_or_enum_tag_ref_type_specifier_parser(),
        ))
        .map(DeclSpecifierPiece::Type);
        let tail_piece = choice((
            qualifier_piece.clone(),
            builtin_type_specifier_parser().map(DeclSpecifierPiece::Type),
            typedef_name_type_specifier_parser().map(DeclSpecifierPiece::Type),
            record_or_enum_tag_ref_type_specifier_parser().map(DeclSpecifierPiece::Type),
        ));

        qualifier_piece
            .repeated()
            .collect::<Vec<_>>()
            .then(first_type_piece)
            .then(tail_piece.repeated().collect::<Vec<_>>())
            .map(|((mut prefix, first_ty), mut tail)| {
                prefix.push(first_ty);
                prefix.append(&mut tail);
                assemble_decl_spec(prefix)
            })
    };

    let sizeof_type_name = sizeof_type_spec
        .then(
            pointer_layers_parser()
                .filter(|pointers| !pointers.is_empty())
                .or_not(),
        )
        .map(|(specifiers, pointers)| TypeName {
            specifiers,
            declarator: pointers.map(|pointers| {
                Box::new(Declarator {
                    pointers,
                    direct: Box::new(DirectDeclarator::Abstract),
                })
            }),
        })
        .boxed();

    recursive(|const_expr| {
        let atom = choice((
            integer_literal_expr_parser(),
            select! {
                TokenKind::CharLiteral(value) => Expr::char(value),
            },
            select! {
                TokenKind::Identifier(name) => Expr::var(name),
            },
            const_expr
                .clone()
                .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
        ));

        let unary = recursive(|unary| {
            let sizeof_type = just(TokenKind::Sizeof)
                .ignore_then(
                    sizeof_type_name
                        .clone()
                        .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
                )
                .map(|ty| Expr::new(ExprKind::SizeofType(Box::new(ty))));

            let sizeof_expr = just(TokenKind::Sizeof)
                .ignore_then(unary.clone())
                .map(Expr::sizeof_expr);

            let prefix_op = choice((
                just(TokenKind::Plus).to(UnaryOp::Plus),
                just(TokenKind::Minus).to(UnaryOp::Minus),
                just(TokenKind::Bang).to(UnaryOp::LogicalNot),
                just(TokenKind::Tilde).to(UnaryOp::BitNot),
            ));

            choice((
                prefix_op
                    .then(unary.clone())
                    .map(|(op, expr)| Expr::unary(op, expr)),
                sizeof_type,
                sizeof_expr,
                atom.clone(),
            ))
        });

        binary_pratt_expr_parser(unary)
    })
    .boxed()
}

fn record_member_declarator_suffix_parser<'tokens, I>()
-> impl Parser<'tokens, I, DirectDeclaratorSuffix, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let member_param_decl_spec = {
        let storage = just(TokenKind::Register)
            .to(StorageClass::Register)
            .map(DeclSpecifierPiece::Storage);
        let qualifiers = type_qualifier_parser().map(DeclSpecifierPiece::Qualifier);
        let non_type_piece = choice((storage, qualifiers.clone()));
        let first_type_piece = choice((
            builtin_type_specifier_parser(),
            typedef_name_type_specifier_parser(),
            record_or_enum_tag_ref_type_specifier_parser(),
        ))
        .map(DeclSpecifierPiece::Type);
        let tail_piece = choice((
            non_type_piece.clone(),
            builtin_type_specifier_parser().map(DeclSpecifierPiece::Type),
            typedef_name_type_specifier_parser().map(DeclSpecifierPiece::Type),
            record_or_enum_tag_ref_type_specifier_parser().map(DeclSpecifierPiece::Type),
        ));

        non_type_piece
            .repeated()
            .collect::<Vec<_>>()
            .then(first_type_piece)
            .then(tail_piece.repeated().collect::<Vec<_>>())
            .map(|((mut prefix, first_ty), mut tail)| {
                prefix.push(first_ty);
                prefix.append(&mut tail);
                assemble_decl_spec(prefix)
            })
    };

    let member_parameter = member_param_decl_spec
        .then(
            pointer_layers_parser()
                .then(
                    select! {
                        TokenKind::Identifier(name) => name,
                    }
                    .or_not(),
                )
                .map(|(pointers, name)| match name {
                    Some(name) => Some(Declarator {
                        pointers,
                        direct: Box::new(DirectDeclarator::Ident(name)),
                    }),
                    None if pointers.is_empty() => None,
                    None => Some(Declarator {
                        pointers,
                        direct: Box::new(DirectDeclarator::Abstract),
                    }),
                }),
        )
        .map(|(specifiers, declarator)| ParameterDecl {
            specifiers,
            declarator: declarator.map(Box::new),
        });

    let function_suffix =
        function_params_list_parser(member_parameter).map(DirectDeclaratorSuffix::Function);

    let array_size = constant_expression_parser()
        .map(ArraySize::Expr)
        .or_not()
        .map(|size| size.unwrap_or(ArraySize::Unspecified));

    let array_suffix = array_size
        .delimited_by(just(TokenKind::LBracket), just(TokenKind::RBracket))
        .map(|size| {
            DirectDeclaratorSuffix::Array(ArrayDeclaratorSuffix {
                qualifiers: Vec::new(),
                is_static: false,
                size,
            })
        });

    choice((function_suffix, array_suffix)).boxed()
}

fn record_member_declarator_parser<'tokens, I>()
-> impl Parser<'tokens, I, Declarator, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    recursive(|declarator| {
        let direct_ident = select! {
            TokenKind::Identifier(name) => DirectDeclarator::Ident(name),
        };

        let direct_grouped = declarator
            .clone()
            .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen))
            .map(|decl| DirectDeclarator::Grouped(Box::new(decl)));

        let direct_base = choice((direct_ident, direct_grouped));

        let direct = direct_base
            .then(
                record_member_declarator_suffix_parser()
                    .repeated()
                    .collect::<Vec<_>>(),
            )
            .map(|(base, suffixes)| fold_direct_declarator_suffixes(base, suffixes));

        pointer_layers_parser()
            .then(direct)
            .map(|(pointers, direct)| Declarator {
                pointers,
                direct: Box::new(direct),
            })
    })
    .boxed()
}

fn record_member_declaration_parser<'tokens, I>()
-> impl Parser<'tokens, I, RecordMemberDecl, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let mut member_decl = Recursive::declare();
    let mut member_type_spec = Recursive::declare();

    let tag = select! {
        TokenKind::Identifier(name) => name,
    }
    .or_not();

    let nested_record_spec = choice((
        just(TokenKind::Struct).to(RecordKind::Struct),
        just(TokenKind::Union).to(RecordKind::Union),
    ))
    .then(tag.clone())
    .then(
        member_decl
            .clone()
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(just(TokenKind::LBrace), just(TokenKind::RBrace))
            .or_not(),
    )
    .try_map(|((kind, tag), members), span| {
        if tag.is_none() && members.is_none() {
            return Err(Rich::custom(
                span,
                "struct/union specifier requires a tag or a definition",
            ));
        }

        Ok(TypeSpecifier::StructOrUnion(RecordSpecifier {
            kind,
            tag,
            members,
        }))
    });

    let nested_enum_spec = just(TokenKind::Enum)
        .ignore_then(tag)
        .then(
            enum_enumerator_name_parser()
                .then(
                    just(TokenKind::Assign)
                        .ignore_then(constant_expression_parser())
                        .or_not(),
                )
                .map(|(name, value)| EnumVariant { name, value })
                .separated_by(just(TokenKind::Comma))
                .at_least(1)
                .collect::<Vec<_>>()
                .then_ignore(just(TokenKind::Comma).or_not())
                .delimited_by(just(TokenKind::LBrace), just(TokenKind::RBrace))
                .or_not(),
        )
        .try_map(|(tag, variants), span| {
            if tag.is_none() && variants.is_none() {
                return Err(Rich::custom(
                    span,
                    "enum specifier requires a tag or a definition",
                ));
            }

            Ok(TypeSpecifier::Enum(EnumSpecifier { tag, variants }))
        })
        .map_with(|ty, extra| {
            if let TypeSpecifier::Enum(enum_spec) = &ty {
                bind_enum_variants(enum_spec, extra.state());
            }
            ty
        });

    member_type_spec.define(
        choice((
            builtin_type_specifier_parser(),
            typedef_name_type_specifier_parser(),
            nested_record_spec,
            nested_enum_spec,
            record_or_enum_tag_ref_type_specifier_parser(),
        ))
        .boxed(),
    );

    let qualifiers = type_qualifier_parser().map(DeclSpecifierPiece::Qualifier);
    let first_ty = member_type_spec.clone().map(DeclSpecifierPiece::Type);
    let tail_piece = choice((
        qualifiers.clone(),
        member_type_spec.map(DeclSpecifierPiece::Type),
    ));

    member_decl.define(
        qualifiers
            .repeated()
            .collect::<Vec<_>>()
            .then(first_ty)
            .then(tail_piece.repeated().collect::<Vec<_>>())
            .map(
                |((mut prefix, first_ty), mut tail): (
                    (Vec<DeclSpecifierPiece>, DeclSpecifierPiece),
                    Vec<DeclSpecifierPiece>,
                )| {
                    prefix.push(first_ty);
                    prefix.append(&mut tail);
                    assemble_decl_spec(prefix)
                },
            )
            .then(
                record_member_declarator_parser()
                    .separated_by(just(TokenKind::Comma))
                    .at_least(1)
                    .collect::<Vec<_>>(),
            )
            .then_ignore(just(TokenKind::Semicolon))
            .map(|(specifiers, declarators)| RecordMemberDecl {
                specifiers,
                declarators,
            }),
    );

    member_decl.boxed()
}

fn record_specifier_parser<'tokens, I>()
-> impl Parser<'tokens, I, TypeSpecifier, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let kind = choice((
        just(TokenKind::Struct).to(RecordKind::Struct),
        just(TokenKind::Union).to(RecordKind::Union),
    ));

    let tag = select! {
        TokenKind::Identifier(name) => name,
    }
    .or_not();

    let members = record_member_declaration_parser()
        .repeated()
        .collect::<Vec<_>>()
        .delimited_by(just(TokenKind::LBrace), just(TokenKind::RBrace))
        .or_not();

    kind.then(tag)
        .then(members)
        .try_map(|((kind, tag), members), span| {
            if tag.is_none() && members.is_none() {
                return Err(Rich::custom(
                    span,
                    "struct/union specifier requires a tag or a definition",
                ));
            }

            Ok(TypeSpecifier::StructOrUnion(RecordSpecifier {
                kind,
                tag,
                members,
            }))
        })
        .boxed()
}

fn enum_specifier_parser<'tokens, I>()
-> impl Parser<'tokens, I, TypeSpecifier, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let tag = select! {
        TokenKind::Identifier(name) => name,
    }
    .or_not();

    let enumerator = enum_enumerator_name_parser()
        .then(
            just(TokenKind::Assign)
                .ignore_then(constant_expression_parser())
                .or_not(),
        )
        .map(|(name, value)| EnumVariant { name, value });

    let variants = enumerator
        .separated_by(just(TokenKind::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .then_ignore(just(TokenKind::Comma).or_not())
        .delimited_by(just(TokenKind::LBrace), just(TokenKind::RBrace))
        .or_not();

    just(TokenKind::Enum)
        .ignore_then(tag)
        .then(variants)
        .try_map(|(tag, variants), span| {
            if tag.is_none() && variants.is_none() {
                return Err(Rich::custom(
                    span,
                    "enum specifier requires a tag or a definition",
                ));
            }

            Ok(TypeSpecifier::Enum(EnumSpecifier { tag, variants }))
        })
        .map_with(|ty, extra| {
            if let TypeSpecifier::Enum(enum_spec) = &ty {
                bind_enum_variants(enum_spec, extra.state());
            }
            ty
        })
        .boxed()
}

/// Parse declaration specifiers and keep the original sequence information
/// grouped by category for later semantic validation.
fn decl_spec_parser<'tokens, I>()
-> impl Parser<'tokens, I, DeclSpec, ParserExtra<'tokens, I>> + Clone
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

    let builtin_ty = builtin_type_specifier_parser();
    let typedef_name = typedef_name_type_specifier_parser();
    let record_specifier = record_specifier_parser();
    let enum_specifier = enum_specifier_parser();

    let non_type_piece = choice((storage, qualifiers, function_specifier));
    let first_type_piece = choice((
        builtin_ty.clone(),
        typedef_name,
        record_specifier,
        enum_specifier,
    ))
    .map(DeclSpecifierPiece::Type);
    let tail_piece = choice((
        non_type_piece.clone(),
        builtin_ty.map(DeclSpecifierPiece::Type),
    ));

    non_type_piece
        .repeated()
        .collect::<Vec<_>>()
        .then(first_type_piece)
        .then(tail_piece.repeated().collect::<Vec<_>>())
        .map(|((mut prefix, first_ty), mut tail)| {
            prefix.push(first_ty);
            prefix.append(&mut tail);
            assemble_decl_spec(prefix)
        })
        .labelled(ParserLabel::DeclarationSpecifier.as_str())
        .boxed()
}

fn pointer_ident_array_declarator_parser<'tokens, I, S>(
    array_suffix: S,
) -> impl Parser<'tokens, I, Option<Declarator>, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, DirectDeclaratorSuffix, ParserExtra<'tokens, I>> + Clone + 'tokens,
{
    let array_suffixes = array_suffix.repeated().collect::<Vec<_>>();

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

            direct_base.map(|base| Declarator {
                pointers,
                direct: Box::new(fold_direct_declarator_suffixes(base, suffixes)),
            })
        })
}

/// Parse a `type-name` used by cast and `sizeof(type-name)`.
///
/// This phase supports declaration specifiers and optional pointer layers.
fn type_name_parser<'tokens, I>()
-> impl Parser<'tokens, I, TypeName, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let type_name_array_suffix =
        array_declarator_suffix_with_size_expr_parser(constant_expression_parser());

    let type_name_parameter_declarator =
        pointer_ident_array_declarator_parser(type_name_array_suffix.clone());

    let type_name_parameter =
        decl_spec_parser()
            .then(type_name_parameter_declarator)
            .map(|(specifiers, declarator)| ParameterDecl {
                specifiers,
                declarator: declarator.map(Box::new),
            });

    let type_name_function_params = function_params_list_parser(type_name_parameter);

    let type_name_suffix = choice((
        type_name_function_params.map(DirectDeclaratorSuffix::Function),
        type_name_array_suffix,
    ));

    let abstract_declarator = abstract_declarator_with_suffix_parser(type_name_suffix);

    decl_spec_parser()
        .then(abstract_declarator.or_not())
        .map(|(specifiers, declarator)| TypeName {
            specifiers,
            declarator: declarator.map(Box::new),
        })
        .boxed()
}

/// `(void)` means an empty prototype parameter list in C.
fn is_void_parameter_decl(param: &ParameterDecl) -> bool {
    param.declarator.is_none()
        && param.specifiers.storage.is_empty()
        && param.specifiers.qualifiers.is_empty()
        && param.specifiers.function.is_empty()
        && param.specifiers.ty == vec![TypeSpecifier::Void]
}

fn map_parameter_list(params: Option<Vec<ParameterDecl>>, variadic: bool) -> FunctionParams {
    match params {
        None => FunctionParams::NonPrototype,
        Some(params) if params.len() == 1 && is_void_parameter_decl(&params[0]) => {
            FunctionParams::Prototype {
                params: Vec::new(),
                variadic: false,
            }
        }
        Some(params) => FunctionParams::Prototype { params, variadic },
    }
}

/// Parse the variadic suffix `, ...` in function parameter lists.
///
/// Returns `true` if the parameter list is variadic, `false` otherwise.
fn variadic_suffix_parser<'tokens, I>()
-> impl Parser<'tokens, I, bool, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    just(TokenKind::Comma)
        .then(just(TokenKind::Ellipsis))
        .or_not()
        .map(|opt| opt.is_some())
}

/// Parse a function parameter list: `(param1, param2, ...)`.
fn function_params_list_parser<'tokens, I, P>(
    parameter: P,
) -> impl Parser<'tokens, I, FunctionParams, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, ParameterDecl, ParserExtra<'tokens, I>> + Clone + 'tokens,
{
    parameter
        .separated_by(just(TokenKind::Comma))
        .at_least(1)
        .collect::<Vec<_>>()
        .or_not()
        .then(variadic_suffix_parser())
        .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen))
        .try_map(|(params, variadic), span| {
            if variadic {
                if params.is_none() {
                    return Err(Rich::custom(
                        span,
                        "variadic parameter list requires at least one named or typed parameter",
                    ));
                }
                if matches!(params.as_deref(), Some([param]) if is_void_parameter_decl(param)) {
                    return Err(Rich::custom(
                        span,
                        "variadic parameter list cannot use `void` as the only fixed parameter",
                    ));
                }
            }
            Ok(map_parameter_list(params, variadic))
        })
}

fn basic_parameter_declarator_parser<'tokens, I>()
-> impl Parser<'tokens, I, Option<Declarator>, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    pointer_ident_array_declarator_parser(array_declarator_suffix_with_size_expr_parser(
        assignment_expression_parser(),
    ))
    .boxed()
}

fn simple_function_params_parser<'tokens, I>()
-> impl Parser<'tokens, I, FunctionParams, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let parameter = decl_spec_parser()
        .then(basic_parameter_declarator_parser())
        .map(|(specifiers, declarator)| ParameterDecl {
            specifiers,
            declarator: declarator.map(Box::new),
        });

    function_params_list_parser(parameter).boxed()
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
-> impl Parser<'tokens, I, Option<Declarator>, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let basic_declarator = basic_parameter_declarator_parser();

    let grouped_function_pointer = basic_declarator
        .clone()
        .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen))
        .then(simple_function_params_parser())
        .map(|(inner_declarator, params)| {
            let inner_declarator = inner_declarator.unwrap_or(Declarator {
                pointers: Vec::new(),
                direct: Box::new(DirectDeclarator::Abstract),
            });

            Some(Declarator {
                pointers: Vec::new(),
                direct: Box::new(DirectDeclarator::Function {
                    inner: Box::new(DirectDeclarator::Grouped(Box::new(inner_declarator))),
                    params,
                }),
            })
        });

    choice((grouped_function_pointer, basic_declarator)).boxed()
}

/// Parse function parameter list forms:
/// - `()` as old-style non-prototype
/// - `(void)` as empty prototype
/// - `(int x, char *p)` as named prototype
fn function_params_parser<'tokens, I>()
-> impl Parser<'tokens, I, FunctionParams, ParserExtra<'tokens, I>> + Clone
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

    function_params_list_parser(parameter).boxed()
}

#[derive(Clone)]
struct ArrayDeclaratorSuffix {
    qualifiers: Vec<TypeQualifier>,
    is_static: bool,
    size: ArraySize,
}

fn array_declarator_suffix_with_size_expr_parser<'tokens, I, SZ>(
    size_expr: SZ,
) -> impl Parser<'tokens, I, DirectDeclaratorSuffix, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    SZ: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone + 'tokens,
{
    let qualifiers = type_qualifier_parser()
        .repeated()
        .collect::<Vec<TypeQualifier>>();
    let sized = size_expr.map(ArraySize::Expr);

    let plain_array = qualifiers
        .clone()
        .then(sized.clone().or_not())
        .map(|(qualifiers, size)| ArrayDeclaratorSuffix {
            qualifiers,
            is_static: false,
            size: size.unwrap_or(ArraySize::Unspecified),
        });

    let static_then_qualifiers = just(TokenKind::Static)
        .ignore_then(qualifiers.clone())
        .then(sized.clone())
        .map(|(qualifiers, size)| ArrayDeclaratorSuffix {
            qualifiers,
            is_static: true,
            size,
        });

    let qualifiers_then_static = qualifiers
        .then_ignore(just(TokenKind::Static))
        .then(sized)
        .map(|(qualifiers, size)| ArrayDeclaratorSuffix {
            qualifiers,
            is_static: true,
            size,
        });

    choice((static_then_qualifiers, qualifiers_then_static, plain_array))
        .delimited_by(just(TokenKind::LBracket), just(TokenKind::RBracket))
        .map(DirectDeclaratorSuffix::Array)
}

#[derive(Clone)]
enum DirectDeclaratorSuffix {
    /// Function suffix: `(params...)`
    Function(FunctionParams),
    /// Array suffix: `[...]`
    Array(ArrayDeclaratorSuffix),
}

fn fold_direct_declarator_suffixes(
    base: DirectDeclarator,
    suffixes: Vec<DirectDeclaratorSuffix>,
) -> DirectDeclarator {
    suffixes
        .into_iter()
        .fold(base, |inner, suffix| match suffix {
            DirectDeclaratorSuffix::Function(params) => DirectDeclarator::Function {
                inner: Box::new(inner),
                params,
            },
            DirectDeclaratorSuffix::Array(array) => DirectDeclarator::Array {
                inner: Box::new(inner),
                qualifiers: array.qualifiers,
                is_static: array.is_static,
                size: Box::new(array.size),
            },
        })
}

/// Parse one direct-declarator suffix.
///
/// This parser is intentionally suffix-only so declarator parsing can build:
/// `base-ident + suffix*` and fold left-to-right.
fn direct_declarator_suffix_with_function_params_parser<'tokens, I, FP>(
    function_params: FP,
) -> impl Parser<'tokens, I, DirectDeclaratorSuffix, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    FP: Parser<'tokens, I, FunctionParams, ParserExtra<'tokens, I>> + Clone + 'tokens,
{
    let function_suffix = function_params.map(DirectDeclaratorSuffix::Function);

    // Supports array declarator forms with optional qualifiers/static:
    // `[]`, `[e]`, `[const]`, `[static e]`, `[const static e]`, `[static const e]`.
    // `[*]` (VLA marker in prototype scope) is still intentionally unsupported.
    let array_suffix =
        array_declarator_suffix_with_size_expr_parser(assignment_expression_parser());

    choice((function_suffix, array_suffix)).boxed()
}

/// Parse one direct-declarator suffix.
///
/// This parser is intentionally suffix-only so declarator parsing can build:
/// `base-ident + suffix*` and fold left-to-right.
fn direct_declarator_suffix_parser<'tokens, I>()
-> impl Parser<'tokens, I, DirectDeclaratorSuffix, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    direct_declarator_suffix_with_function_params_parser(function_params_parser())
}

fn abstract_declarator_with_suffix_parser<'tokens, I, S>(
    suffix: S,
) -> impl Parser<'tokens, I, Declarator, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, DirectDeclaratorSuffix, ParserExtra<'tokens, I>> + Clone + 'tokens,
{
    recursive(|abstract_declarator| {
        let grouped_base = abstract_declarator
            .clone()
            .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen))
            .map(|decl| DirectDeclarator::Grouped(Box::new(decl)));

        let grouped_with_suffix = grouped_base
            .then(suffix.clone().repeated().collect::<Vec<_>>())
            .map(|(base, suffixes)| fold_direct_declarator_suffixes(base, suffixes));

        let implicit_abstract_with_suffix = suffix
            .clone()
            .repeated()
            .at_least(1)
            .collect::<Vec<_>>()
            .map(|suffixes| fold_direct_declarator_suffixes(DirectDeclarator::Abstract, suffixes));

        let direct = choice((grouped_with_suffix, implicit_abstract_with_suffix));

        let with_direct = pointer_layers_parser()
            .then(direct)
            .map(|(pointers, direct)| Declarator {
                pointers,
                direct: Box::new(direct),
            });

        let pointer_only = pointer_layers_parser()
            .filter(|pointers| !pointers.is_empty())
            .map(|pointers| Declarator {
                pointers,
                direct: Box::new(DirectDeclarator::Abstract),
            });

        choice((with_direct, pointer_only))
    })
    .boxed()
}

/// Parse the currently supported declarator subset:
/// zero-or-more pointer stars, then identifier and postfix direct-declarator suffixes.
fn declarator_parser<'tokens, I>()
-> impl Parser<'tokens, I, Declarator, ParserExtra<'tokens, I>> + Clone
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
            .map(|(base, suffixes)| fold_direct_declarator_suffixes(base, suffixes));

        pointer_layers_parser()
            .then(direct)
            .map(|(pointers, direct)| Declarator {
                pointers,
                direct: Box::new(direct),
            })
    })
}

fn direct_declarator_name(direct: &DirectDeclarator) -> Option<&str> {
    match direct {
        DirectDeclarator::Ident(name) => Some(name),
        DirectDeclarator::Abstract => None,
        DirectDeclarator::Grouped(declarator) => declarator_name(declarator),
        DirectDeclarator::Array { inner, .. } | DirectDeclarator::Function { inner, .. } => {
            direct_declarator_name(inner)
        }
    }
}

fn declarator_name(declarator: &Declarator) -> Option<&str> {
    direct_declarator_name(declarator.direct.as_ref())
}

fn declaration_binding_kind(specifiers: &DeclSpec) -> BindingKind {
    if specifiers.storage.contains(&StorageClass::Typedef) {
        BindingKind::Typedef
    } else {
        BindingKind::Ordinary
    }
}

fn bind_declaration_names<'tokens, I>(
    declaration: &Declaration,
    extra: &mut chumsky::input::MapExtra<'tokens, '_, I, ParserExtra<'tokens, I>>,
) -> Result<(), ParseError<'tokens>>
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let kind = declaration_binding_kind(&declaration.specifiers);
    for init_declarator in &declaration.declarators {
        if let Some(name) = declarator_name(&init_declarator.declarator) {
            if let Some(existing_kind) = extra.state().binding_in_current_scope(name) {
                if existing_kind != kind {
                    return Err(Rich::custom(
                        extra.span(),
                        format!("conflicting declaration for '{name}' in the same scope"),
                    ));
                }
                continue;
            }

            let _ = extra.state().bind(name.to_string(), kind);
        }
    }

    Ok(())
}

fn function_params_from_direct_declarator(direct: &DirectDeclarator) -> Option<&FunctionParams> {
    match direct {
        DirectDeclarator::Function { params, .. } => Some(params),
        DirectDeclarator::Array { inner, .. } => function_params_from_direct_declarator(inner),
        DirectDeclarator::Grouped(declarator) => {
            function_params_from_direct_declarator(declarator.direct.as_ref())
        }
        DirectDeclarator::Ident(_) | DirectDeclarator::Abstract => None,
    }
}

fn bind_function_parameter_names(declarator: &Declarator, state: &mut Typedefs) {
    let Some(FunctionParams::Prototype { params, .. }) =
        function_params_from_direct_declarator(declarator.direct.as_ref())
    else {
        return;
    };

    for param in params {
        let Some(param_declarator) = &param.declarator else {
            continue;
        };

        if let Some(name) = declarator_name(param_declarator) {
            state.bind(name.to_string(), BindingKind::Ordinary);
        }
    }
}

/// Parse initializer syntax:
/// - scalar: `assignment-expression`
/// - aggregate: `{ ... }`
fn initializer_parser<'tokens, I>()
-> impl Parser<'tokens, I, Initializer, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    recursive(|initializer| {
        let scalar = assignment_expression_parser().map(|expr| Initializer {
            kind: InitializerKind::Expr(expr),
        });

        let designator = choice((
            constant_expression_parser()
                .delimited_by(just(TokenKind::LBracket), just(TokenKind::RBracket))
                .map(Designator::Index),
            just(TokenKind::Dot)
                .ignore_then(select! {
                    TokenKind::Identifier(field) => field,
                })
                .map(Designator::Field),
        ));

        let initializer_item = designator
            .repeated()
            .at_least(1)
            .collect::<Vec<_>>()
            .then_ignore(just(TokenKind::Assign))
            .then(initializer.clone())
            .map(|(designators, init)| InitializerItem { designators, init })
            .or(initializer.clone().map(|init| InitializerItem {
                designators: Vec::new(),
                init,
            }));

        let aggregate = initializer_item
            .separated_by(just(TokenKind::Comma))
            .at_least(1)
            .collect::<Vec<_>>()
            .then_ignore(just(TokenKind::Comma).or_not())
            .delimited_by(just(TokenKind::LBrace), just(TokenKind::RBrace))
            .map(|items| Initializer {
                kind: InitializerKind::Aggregate(items),
            });

        choice((aggregate, scalar))
    })
}

/// Parse a declaration statement ending with `;`.
fn declaration_parser<'tokens, I>()
-> impl Parser<'tokens, I, Declaration, ParserExtra<'tokens, I>> + Clone
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
                .collect::<Vec<_>>()
                .or_not()
                .map(|declarators| declarators.unwrap_or_default()),
        )
        .then_ignore(just(TokenKind::Semicolon))
        .map(|(specifiers, declarators)| Declaration {
            specifiers,
            declarators,
        })
        .try_map_with(|declaration, extra| {
            bind_declaration_names(&declaration, extra)?;
            Ok(declaration)
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
) -> impl Parser<'tokens, I, BlockItem, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone,
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
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone,
{
    just(TokenKind::LBrace)
        .map_with(|_, extra| {
            let extra: &mut chumsky::input::MapExtra<'tokens, '_, I, ParserExtra<'tokens, I>> =
                extra;
            extra.state().push_scope();
        })
        .then(
            block_item_with_statement_parser(statement)
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then_ignore(just(TokenKind::RBrace))
        .map_with(|(_, items), extra| {
            extra.state().pop_scope();
            Stmt::Compound(CompoundStmt { items })
        })
        .labelled(ParserLabel::CompoundStatement.as_str())
}

/// Parse an expression statement: either `;` or `expression;`.
fn expression_statement_parser<'tokens, I, E>(
    expr: E,
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    E: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
{
    expr.or_not()
        .then_ignore(just(TokenKind::Semicolon))
        .map(|expr| match expr {
            Some(expr) => Stmt::Expr(expr),
            None => Stmt::Empty,
        })
        .labelled(ParserLabel::ExpressionStatement.as_str())
}

/// Parse `return;` and `return expr;`.
fn return_statement_parser<'tokens, I, E>(
    expr: E,
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    E: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
{
    just(TokenKind::Return)
        .ignore_then(expr.or_not())
        .then_ignore(just(TokenKind::Semicolon))
        .map(Stmt::Return)
        .labelled(ParserLabel::ReturnStatement.as_str())
}

/// Parse `if (cond) stmt` with optional `else stmt`.
fn if_statement_parser<'tokens, I, S, E>(
    statement: S,
    expr: E,
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone,
    E: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
{
    just(TokenKind::If)
        .ignore_then(expr.delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)))
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
fn while_statement_parser<'tokens, I, S, E>(
    statement: S,
    expr: E,
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone,
    E: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
{
    just(TokenKind::While)
        .ignore_then(expr.delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)))
        .then(statement)
        .map(|(cond, body)| Stmt::While {
            cond,
            body: Box::new(body),
        })
        .labelled(ParserLabel::WhileStatement.as_str())
}

/// Parse `do stmt while (cond);`.
fn do_while_statement_parser<'tokens, I, S, E>(
    statement: S,
    expr: E,
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone,
    E: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
{
    just(TokenKind::Do)
        .ignore_then(statement)
        .then_ignore(just(TokenKind::While))
        .then(expr.delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)))
        .then_ignore(just(TokenKind::Semicolon))
        .map(|(body, cond)| Stmt::DoWhile {
            body: Box::new(body),
            cond,
        })
        .labelled(ParserLabel::DoWhileStatement.as_str())
}

/// Parse `for (init; cond; step) stmt`.
fn for_statement_parser<'tokens, I, S, E>(
    statement: S,
    expr: E,
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone,
    E: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
{
    let header_with_decl_init = just(TokenKind::LParen)
        .map_with(|_, extra| {
            let extra: &mut chumsky::input::MapExtra<'tokens, '_, I, ParserExtra<'tokens, I>> =
                extra;
            extra.state().push_scope();
        })
        .then(
            declaration_parser()
                .then(expr.clone().or_not())
                .then_ignore(just(TokenKind::Semicolon))
                .then(expr.clone().or_not()),
        )
        .then_ignore(just(TokenKind::RParen))
        .map(|(_, ((decl, cond), step))| (Some(ForInit::Decl(decl)), cond, step, true));

    let header_with_expr_init = expr
        .clone()
        .or_not()
        .then_ignore(just(TokenKind::Semicolon))
        .then(expr.clone().or_not())
        .then_ignore(just(TokenKind::Semicolon))
        .then(expr.clone().or_not())
        .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen))
        .map(|((init, cond), step)| (init.map(ForInit::Expr), cond, step, false));

    just(TokenKind::For)
        .ignore_then(choice((header_with_decl_init, header_with_expr_init)))
        .then(statement)
        .map_with(|((init, cond, step, needs_decl_scope_pop), body), extra| {
            if needs_decl_scope_pop {
                extra.state().pop_scope();
            }

            Stmt::For {
                init,
                cond,
                step,
                body: Box::new(body),
            }
        })
        .labelled(ParserLabel::ForStatement.as_str())
}

/// Parse `switch (expr) stmt`.
fn switch_statement_parser<'tokens, I, S, E>(
    statement: S,
    expr: E,
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone,
    E: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone,
{
    just(TokenKind::Switch)
        .ignore_then(expr.delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)))
        .then(statement)
        .map(|(expr, body)| Stmt::Switch {
            expr,
            body: Box::new(body),
        })
        .labelled(ParserLabel::SwitchStatement.as_str())
}

/// Parse `case constant-expression: stmt`.
fn case_statement_parser<'tokens, I, S>(
    statement: S,
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone,
{
    just(TokenKind::Case)
        // `case` expects constant-expression (no comma-expression at top level).
        .ignore_then(constant_expression_parser())
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
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone,
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
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    S: Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone,
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
-> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
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
-> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
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
-> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
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

/// Parse the currently supported statement subset using shared expression parsers.
fn statement_parser_with_expr<'tokens, I, E>(
    expr: E,
) -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    E: Parser<'tokens, I, Expr, ParserExtra<'tokens, I>> + Clone + 'tokens,
{
    recursive(|statement| {
        let compound_stmt = compound_statement_parser(statement.clone());

        choice((
            compound_stmt,
            return_statement_parser(expr.clone()),
            if_statement_parser(statement.clone(), expr.clone()),
            while_statement_parser(statement.clone(), expr.clone()),
            do_while_statement_parser(statement.clone(), expr.clone()),
            for_statement_parser(statement.clone(), expr.clone()),
            switch_statement_parser(statement.clone(), expr.clone()),
            case_statement_parser(statement.clone()),
            default_statement_parser(statement.clone()),
            // Must appear before expression statement to parse `label: ...` correctly.
            label_statement_parser(statement.clone()),
            goto_statement_parser(),
            break_statement_parser(),
            continue_statement_parser(),
            expression_statement_parser(expr.clone()),
        ))
    })
    .labelled(ParserLabel::Statement.as_str())
}

/// Parse the currently supported statement subset.
fn statement_parser<'tokens, I>() -> impl Parser<'tokens, I, Stmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let expr = expr_parser::<'tokens, I, true>();
    statement_parser_with_expr(expr)
}

/// Parse one block item as either a declaration or a statement.
fn block_item_parser<'tokens, I>()
-> impl Parser<'tokens, I, BlockItem, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    block_item_with_statement_parser(statement_parser())
}

/// Parse a function body `{ ... }` without creating an extra nested scope.
///
/// Function parameters and the outermost compound block share one scope in C.
/// `function_definition_parser` enters that scope before parsing this body.
fn function_body_parser<'tokens, I>()
-> impl Parser<'tokens, I, CompoundStmt, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    just(TokenKind::LBrace)
        .ignore_then(
            block_item_with_statement_parser(statement_parser())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then_ignore(just(TokenKind::RBrace))
        .map(|items| CompoundStmt { items })
        .labelled(ParserLabel::CompoundStatement.as_str())
}

// ============================
// Translation unit parsing
// ============================

fn function_definition_parser<'tokens, I>()
-> impl Parser<'tokens, I, FunctionDef, ParserExtra<'tokens, I>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    decl_spec_parser()
        .then(declarator_parser())
        // Lookahead: only treat as function definition when a body starts here.
        // The leading `{` is preserved for `statement_parser`.
        .then_ignore(just(TokenKind::LBrace).rewind())
        .try_map_with(|(specifiers, declarator), extra| {
            if let Some(name) = declarator_name(&declarator) {
                if let Some(existing_kind) = extra.state().binding_in_current_scope(name) {
                    if existing_kind != BindingKind::Ordinary {
                        return Err(Rich::custom(
                            extra.span(),
                            format!("conflicting declaration for '{name}' in the same scope"),
                        ));
                    }
                } else {
                    let _ = extra.state().bind(name.to_string(), BindingKind::Ordinary);
                }
            }
            extra.state().push_scope();
            bind_function_parameter_names(&declarator, extra.state());
            Ok((specifiers, declarator))
        })
        .then(function_body_parser())
        .map_with(|((specifiers, declarator), body), extra| {
            extra.state().pop_scope();

            FunctionDef {
                specifiers,
                declarator,
                declarations: Vec::new(),
                body,
            }
        })
}

/// Parse the whole translation unit as a sequence of external declarations.
fn parser<'tokens, I>() -> impl Parser<'tokens, I, TranslationUnit, ParserExtra<'tokens, I>> + Clone
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
    let mut state = Typedefs::default();
    parser().parse_with_state(input, &mut state).into_result()
}

/// Parse a single statement from input.
pub fn parse_statement<'tokens, I>(input: I) -> Result<Stmt, Vec<ParseError<'tokens>>>
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let mut state = Typedefs::default();
    statement_parser()
        .then_ignore(end())
        .parse_with_state(input, &mut state)
        .into_result()
}

/// Parse a single block item from input.
pub fn parse_block_item<'tokens, I>(input: I) -> Result<BlockItem, Vec<ParseError<'tokens>>>
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let mut state = Typedefs::default();
    block_item_parser()
        .then_ignore(end())
        .parse_with_state(input, &mut state)
        .into_result()
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
