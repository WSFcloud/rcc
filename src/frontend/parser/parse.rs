use crate::common::token::TokenKind;
use crate::frontend::parser::ast::{AssignOp, BinaryOp, Expr, IntLiteralBase, UnaryOp};
use chumsky::{
    error::Rich,
    input::ValueInput,
    pratt::{infix, left, postfix, prefix},
    prelude::*,
    span::SimpleSpan,
};

pub type Span = SimpleSpan<usize>;
pub type ParseError<'tokens> = Rich<'tokens, TokenKind, Span>;

#[derive(Clone, Copy)]
enum PrefixExprOp {
    Unary(UnaryOp),
    PreInc,
    PreDec,
    Sizeof,
}

impl PrefixExprOp {
    fn apply(self, rhs: Expr) -> Expr {
        match self {
            Self::Unary(op) => Expr::unary(op, rhs),
            Self::PreInc => Expr::pre_inc(rhs),
            Self::PreDec => Expr::pre_dec(rhs),
            Self::Sizeof => Expr::sizeof_expr(rhs),
        }
    }
}

#[derive(Clone, Copy)]
enum PostfixExprOp {
    PostInc,
    PostDec,
}

impl PostfixExprOp {
    fn apply(self, lhs: Expr) -> Expr {
        match self {
            Self::PostInc => Expr::post_inc(lhs),
            Self::PostDec => Expr::post_dec(lhs),
        }
    }
}

fn parser<'tokens, I>() -> impl Parser<'tokens, I, Expr, extra::Err<ParseError<'tokens>>> + Clone
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    recursive(|expr| {
        // Fold all numeric token variants produced by the lexer into the
        // smaller literal set used by the AST.
        let literal = choice((
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
        ));

        let ident = select! {
            TokenKind::Identifier(name) => Expr::var(name),
        };

        // Atoms are the leaves of the expression tree:
        // literals, identifiers, parenthesized sub-expressions.
        let atom = choice((
            literal,
            ident,
            expr.clone()
                .delimited_by(just(TokenKind::LParen), just(TokenKind::RParen)),
        ))
        .labelled("expression");

        // Pratt parsing handles the regular precedence ladder.
        // Larger binding power means tighter binding.
        let postfix_op = choice((
            just(TokenKind::PlusPlus).to(PostfixExprOp::PostInc),
            just(TokenKind::MinusMinus).to(PostfixExprOp::PostDec),
        ));

        let prefix_op = choice((
            just(TokenKind::PlusPlus).to(PrefixExprOp::PreInc),
            just(TokenKind::MinusMinus).to(PrefixExprOp::PreDec),
            just(TokenKind::Sizeof).to(PrefixExprOp::Sizeof),
            just(TokenKind::Plus).to(PrefixExprOp::Unary(UnaryOp::Plus)),
            just(TokenKind::Minus).to(PrefixExprOp::Unary(UnaryOp::Minus)),
            just(TokenKind::Bang).to(PrefixExprOp::Unary(UnaryOp::LogicalNot)),
            just(TokenKind::Tilde).to(PrefixExprOp::Unary(UnaryOp::BitNot)),
            just(TokenKind::Star).to(PrefixExprOp::Unary(UnaryOp::Deref)),
            just(TokenKind::Amp).to(PrefixExprOp::Unary(UnaryOp::AddressOf)),
        ));

        let pratt = atom.pratt((
            postfix(15, postfix_op, |lhs, op: PostfixExprOp, _| op.apply(lhs)),
            prefix(14, prefix_op, |op: PrefixExprOp, rhs, _| op.apply(rhs)),
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
        ));

        let assign_op = choice((
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
        ));

        let assignment = recursive(|assignment| {
            let conditional = pratt
                .clone()
                .then(
                    just(TokenKind::Question)
                        .ignore_then(expr.clone())
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

        assignment
            .clone()
            .separated_by(just(TokenKind::Comma))
            .at_least(1)
            .collect::<Vec<_>>()
            .map(|exprs| {
                exprs
                    .into_iter()
                    .reduce(Expr::comma)
                    .expect("comma expression requires at least one operand")
            })
    })
    // Accept both `expr` and `expr;` for the current expression-only stage.
    .then_ignore(just(TokenKind::Semicolon).or_not())
    .then_ignore(end())
}

pub fn parse<'tokens, I>(input: I) -> Result<Expr, Vec<ParseError<'tokens>>>
where
    I: ValueInput<'tokens, Token = TokenKind, Span = Span>,
{
    parser().parse(input).into_result()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::lexer::lexer_from_source;
    use crate::frontend::parser::ast::{ExprKind, Literal};
    use chumsky::input::{Input, Stream};

    fn parse_source(src: &str) -> Expr {
        let tokens = lexer_from_source(src);
        let stream = Stream::from_iter(tokens)
            .map((src.len()..src.len()).into(), |(token, span)| (token, span));

        parse(stream).expect("expression should parse")
    }

    #[test]
    fn parses_operator_precedence() {
        let expr = parse_source("1 + 2 * 3");

        assert_eq!(
            expr,
            Expr::new(ExprKind::Binary {
                left: Box::new(Expr::int(1)),
                op: BinaryOp::Add,
                right: Box::new(Expr::new(ExprKind::Binary {
                    left: Box::new(Expr::int(2)),
                    op: BinaryOp::Mul,
                    right: Box::new(Expr::int(3)),
                })),
            })
        );
    }

    #[test]
    fn parses_grouped_and_unary_expressions() {
        let expr = parse_source("-(1 + foo);");

        assert_eq!(
            expr,
            Expr::new(ExprKind::Unary {
                op: UnaryOp::Minus,
                expr: Box::new(Expr::new(ExprKind::Binary {
                    left: Box::new(Expr::int(1)),
                    op: BinaryOp::Add,
                    right: Box::new(Expr::new(ExprKind::Var("foo".to_string()))),
                })),
            })
        );
    }

    #[test]
    fn parses_mixed_numeric_literals() {
        let expr = parse_source("1.5 + 2u");

        assert_eq!(
            expr,
            Expr::new(ExprKind::Binary {
                left: Box::new(Expr::new(ExprKind::Literal(Literal::Float(1.5)))),
                op: BinaryOp::Add,
                right: Box::new(Expr::int_with_base(2, IntLiteralBase::UInt)),
            })
        );
    }

    #[test]
    fn parses_logical_and_shift_precedence() {
        let expr = parse_source("a << 1 + 2 && b");

        assert_eq!(
            expr,
            Expr::new(ExprKind::Binary {
                left: Box::new(Expr::new(ExprKind::Binary {
                    left: Box::new(Expr::new(ExprKind::Var("a".to_string()))),
                    op: BinaryOp::Shl,
                    right: Box::new(Expr::new(ExprKind::Binary {
                        left: Box::new(Expr::int(1)),
                        op: BinaryOp::Add,
                        right: Box::new(Expr::int(2)),
                    })),
                })),
                op: BinaryOp::LogicalAnd,
                right: Box::new(Expr::new(ExprKind::Var("b".to_string()))),
            })
        );
    }

    #[test]
    fn parses_assignment_and_comma_expressions() {
        let expr = parse_source("a = b + 1, c");

        assert_eq!(
            expr,
            Expr::new(ExprKind::Comma {
                left: Box::new(Expr::new(ExprKind::Assign {
                    left: Box::new(Expr::new(ExprKind::Var("a".to_string()))),
                    op: AssignOp::Assign,
                    right: Box::new(Expr::new(ExprKind::Binary {
                        left: Box::new(Expr::new(ExprKind::Var("b".to_string()))),
                        op: BinaryOp::Add,
                        right: Box::new(Expr::int(1)),
                    })),
                })),
                right: Box::new(Expr::new(ExprKind::Var("c".to_string()))),
            })
        );
    }

    #[test]
    fn parses_conditional_expression() {
        let expr = parse_source("flag ? x + 1 : y");

        assert_eq!(
            expr,
            Expr::new(ExprKind::Conditional {
                cond: Box::new(Expr::new(ExprKind::Var("flag".to_string()))),
                then_expr: Box::new(Expr::new(ExprKind::Binary {
                    left: Box::new(Expr::new(ExprKind::Var("x".to_string()))),
                    op: BinaryOp::Add,
                    right: Box::new(Expr::int(1)),
                })),
                else_expr: Box::new(Expr::new(ExprKind::Var("y".to_string()))),
            })
        );
    }
}
