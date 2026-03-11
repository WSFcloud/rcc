use crate::common::token::TokenKind;
use crate::frontend::parser::ast::*;
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
#[derive(Clone, Copy)]
enum PostfixExprOp {
    PostInc,
    PostDec,
}

impl PostfixExprOp {
    /// Build the corresponding AST node for a parsed postfix operator.
    fn apply(self, lhs: Expr) -> Expr {
        match self {
            Self::PostInc => Expr::post_inc(lhs),
            Self::PostDec => Expr::post_dec(lhs),
        }
    }
}

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

fn postfix_expr_op_parser<'tokens, I>()
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

fn pratt_expr_parser<'tokens, I, P>(
    atom: P,
) -> impl Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
    P: Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone,
{
    atom.pratt((
        postfix(15, postfix_expr_op_parser(), |lhs, op: PostfixExprOp, _| {
            op.apply(lhs)
        }),
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

    // Atoms are the leaves of the expression tree:
    // literals, identifiers, parenthesized sub-expressions.
    let atom = choice((
        literal_expr_parser(),
        identifier_expr_parser(),
        grouped_expr
            .clone()
            .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
    ))
    .labelled(ParserLabel::Expr.as_str());
    // Pratt parsing handles the regular precedence ladder.
    // Larger binding power means tighter binding.
    let pratt = pratt_expr_parser(atom);
    let assign_op = assignment_op_parser();

    let assignment = recursive(|assignment| {
        // In `cond ? then : else`, the then-branch is an `expression`
        // (comma expressions are allowed), while else-branch is assignment-expression.
        let then_expr = comma_sequence_parser(
            assignment.clone(),
            "then-branch requires at least one expression",
        );

        let conditional = pratt
            .clone()
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
            .clone()
            .then(assign_op.clone().then(assignment).or_not())
            .map(|(left, assign)| match assign {
                Some((op, right)) => Expr::assign(left, op, right),
                None => left,
            })
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

/// Parse the currently supported declarator subset:
/// zero-or-more pointer stars followed by an identifier direct declarator.
fn declarator_parser<'tokens, I>()
-> impl Parser<'tokens, I, Declarator, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    let pointer = just(TokenKind::Star)
        .ignore_then(
            type_qualifier_parser()
                .repeated()
                .collect::<Vec<TypeQualifier>>(),
        )
        .map(|qualifiers| Pointer { qualifiers });

    let direct = select! {
        TokenKind::Identifier(name) => DirectDeclarator::Ident(name),
    }
    .labelled(ParserLabel::IdentifierDeclarator.as_str());

    pointer
        .repeated()
        .collect::<Vec<_>>()
        .then(direct)
        .map(|(pointers, direct)| Declarator {
            pointers,
            direct: Box::new(direct),
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

/// Parse the currently supported statement subset.
fn statement_parser<'tokens, I>()
-> impl Parser<'tokens, I, Stmt, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    recursive(|statement| {
        let block_item = choice((
            declaration_parser().map(BlockItem::Decl),
            statement.clone().map(BlockItem::Stmt),
        ))
        .labelled(ParserLabel::BlockItem.as_str());

        let compound_stmt = block_item
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(just(TokenKind::LBrace), just(TokenKind::RBrace))
            .map(|items| Stmt::Compound(CompoundStmt { items }))
            .labelled(ParserLabel::CompoundStatement.as_str());

        let return_stmt = just(TokenKind::Return)
            .ignore_then(expr_parser::<'tokens, I, true>().or_not())
            .then_ignore(just(TokenKind::Semicolon))
            .map(Stmt::Return)
            .labelled(ParserLabel::ReturnStatement.as_str());

        let if_stmt = just(TokenKind::If)
            .ignore_then(
                expr_parser::<'tokens, I, true>()
                    .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
            )
            .then(statement.clone())
            .then(
                just(TokenKind::Else)
                    .ignore_then(statement.clone())
                    .or_not(),
            )
            .map(|((cond, then_branch), else_branch)| Stmt::If {
                cond,
                then_branch: Box::new(then_branch),
                else_branch: else_branch.map(Box::new),
            })
            .labelled(ParserLabel::IfStatement.as_str());

        let while_stmt = just(TokenKind::While)
            .ignore_then(
                expr_parser::<'tokens, I, true>()
                    .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
            )
            .then(statement.clone())
            .map(|(cond, body)| Stmt::While {
                cond,
                body: Box::new(body),
            })
            .labelled(ParserLabel::WhileStatement.as_str());

        let for_init = choice((
            declaration_parser().map(|decl| Some(ForInit::Decl(decl))),
            expr_parser::<'tokens, I, true>()
                .or_not()
                .then_ignore(just(TokenKind::Semicolon))
                .map(|expr| expr.map(ForInit::Expr)),
        ));

        let for_stmt = just(TokenKind::For)
            .ignore_then(
                for_init
                    .then(expr_parser::<'tokens, I, true>().or_not())
                    .then_ignore(just(TokenKind::Semicolon))
                    .then(expr_parser::<'tokens, I, true>().or_not())
                    .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
            )
            .then(statement.clone())
            .map(|(((init, cond), step), body)| Stmt::For {
                init,
                cond,
                step,
                body: Box::new(body),
            })
            .labelled(ParserLabel::ForStatement.as_str());

        choice((
            compound_stmt,
            return_stmt,
            if_stmt,
            while_stmt,
            for_stmt,
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
    choice((
        declaration_parser().map(BlockItem::Decl),
        statement_parser().map(BlockItem::Stmt),
    ))
    .labelled(ParserLabel::BlockItem.as_str())
}

/// Parse the whole translation unit as a sequence of external declarations.
fn parser<'tokens, I>()
-> impl Parser<'tokens, I, TranslationUnit, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    declaration_parser()
        .repeated()
        .collect::<Vec<_>>()
        .then_ignore(end())
        .map(|declarations| TranslationUnit {
            items: declarations
                .into_iter()
                .map(ExternalDecl::Declaration)
                .collect(),
        })
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

    fn assert_ident_declarator(init_declarator: &InitDeclarator, expected: &str) {
        match init_declarator.declarator.direct.as_ref() {
            DirectDeclarator::Ident(name) => assert_eq!(name, expected),
            other => panic!("expected identifier declarator, got {other:?}"),
        }
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
}
