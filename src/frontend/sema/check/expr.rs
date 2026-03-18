use crate::frontend::parser::ast::{Expr, ExprKind, Literal};
use crate::frontend::sema::context::SemaContext;
use crate::frontend::sema::diagnostic::{SemaDiagnostic, SemaDiagnosticCode};
use crate::frontend::sema::init;
use crate::frontend::sema::symbols::SymbolKind;
use crate::frontend::sema::typed_ast::{ConstValue, TypedExpr, TypedExprKind, ValueCategory};

/// Lower parser expressions into typed expressions.
///
/// This is the main entry point for expression type-checking.
/// In the framework stage, most expressions are lowered to opaque nodes.
///
/// Currently implemented:
/// - Variable references with symbol lookup
/// - Literal expressions with constant values
/// - Recursive lowering of subexpressions
///
/// # TODO
/// - Implement full type checking for all expression kinds
/// - Implement implicit conversions and usual arithmetic conversions
/// - Implement constant expression evaluation
/// - Implement lvalue/rvalue analysis
pub fn lower_expr(cx: &mut SemaContext<'_>, expr: &Expr) -> TypedExpr {
    match &expr.kind {
        ExprKind::Var(name) => lower_variable_expr(cx, name, expr.span),
        ExprKind::Literal(lit) => lower_literal_expr(cx, lit, expr.span),
        ExprKind::Unary { expr: inner, .. }
        | ExprKind::SizeofExpr(inner)
        | ExprKind::PreInc(inner)
        | ExprKind::PreDec(inner)
        | ExprKind::PostInc(inner)
        | ExprKind::PostDec(inner) => {
            let _ = lower_expr(cx, inner);
            TypedExpr::opaque(expr.span, cx.error_type())
        }
        ExprKind::Binary { left, right, .. }
        | ExprKind::Assign { left, right, .. }
        | ExprKind::Comma { left, right } => {
            let _ = lower_expr(cx, left);
            let _ = lower_expr(cx, right);
            TypedExpr::opaque(expr.span, cx.error_type())
        }
        ExprKind::Conditional {
            cond,
            then_expr,
            else_expr,
        } => {
            let _ = lower_expr(cx, cond);
            let _ = lower_expr(cx, then_expr);
            let _ = lower_expr(cx, else_expr);
            TypedExpr::opaque(expr.span, cx.error_type())
        }
        ExprKind::Index { base, index } => {
            let _ = lower_expr(cx, base);
            let _ = lower_expr(cx, index);
            TypedExpr::opaque(expr.span, cx.error_type())
        }
        ExprKind::Member { base, .. } => {
            let _ = lower_expr(cx, base);
            todo!("member id resolution in expr::lower_expr")
        }
        ExprKind::Call { callee, args } => {
            let _ = lower_expr(cx, callee);
            for arg in args {
                let _ = lower_expr(cx, arg);
            }
            TypedExpr::opaque(expr.span, cx.error_type())
        }
        ExprKind::Cast { expr: inner, .. } => {
            let lowered = lower_expr(cx, inner);
            let _ = lowered;
            todo!("explicit cast validity and conversion checks in expr::lower_expr")
        }
        ExprKind::SizeofType(_) => {
            todo!("sizeof(type-name) evaluation and size_t typing")
        }
        ExprKind::CompoundLiteral { init, .. } => {
            let _ = init::lower_initializer(cx, cx.error_type(), init);
            todo!("compound literal typing and storage duration in expr::lower_expr")
        }
    }
}

/// Lowers a variable reference expression.
///
/// This performs symbol lookup with declaration-before-use checking.
/// If the symbol is an enum constant, its compile-time value is attached.
///
/// # Errors
/// Emits `UndefinedSymbol` if the variable is not declared or declared after use.
fn lower_variable_expr(
    cx: &mut SemaContext<'_>,
    name: &str,
    span: crate::common::span::SourceSpan,
) -> TypedExpr {
    if let Some(symbol_id) = cx.resolve_ordinary(name, span) {
        let symbol = cx.symbol(symbol_id);
        let mut typed = TypedExpr::symbol(symbol_id, span, symbol.ty());
        if symbol.kind() == SymbolKind::EnumConst
            && let Some(value) = cx.lookup_enum_const_value(symbol_id)
        {
            typed.const_value = Some(ConstValue::Int(value));
        }
        return typed;
    }

    cx.emit(SemaDiagnostic::new(
        SemaDiagnosticCode::UndefinedSymbol,
        format!("undefined symbol '{name}'"),
        span,
    ));

    TypedExpr::opaque(span, cx.error_type())
}

/// Lowers a literal expression.
///
/// This extracts compile-time constant values from literals.
/// In the framework stage, type inference from suffixes is deferred.
///
/// # TODO
/// - Implement proper literal typing based on suffixes (U, L, LL, F, etc.)
/// - Implement overflow checking for integer literals
/// - Implement proper string literal handling
fn lower_literal_expr(
    cx: &mut SemaContext<'_>,
    lit: &Literal,
    span: crate::common::span::SourceSpan,
) -> TypedExpr {
    // Literal typing and suffix handling are deferred. We still keep useful
    // compile-time values so const-eval scaffolding can run in tests.
    let const_value = match lit {
        Literal::Int { value, .. } => Some(ConstValue::UInt(*value)),
        Literal::Char(ch) => Some(ConstValue::Int(*ch as i64)),
        Literal::Float(value) => Some(ConstValue::FloatBits(value.to_bits())),
        _ => None,
    };

    TypedExpr {
        kind: TypedExprKind::Opaque,
        ty: cx.error_type(),
        value_category: ValueCategory::RValue,
        const_value,
        span,
    }
}
