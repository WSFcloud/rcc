use crate::frontend::sema::diagnostic::{SemaDiagnostic, SemaDiagnosticCode};
use crate::frontend::sema::typed_ast::{ConstValue, TypedExpr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstExprContext {
    IntegerConstant,
    ArithmeticConstant,
    AddressConstant,
}

/// Evaluate constant expressions used by ICE/address-constant contexts.
///
/// The framework version only reuses already attached `TypedExpr::const_value`.
pub fn eval_const_expr(
    expr: &TypedExpr,
    _context: ConstExprContext,
) -> Result<ConstValue, SemaDiagnostic> {
    if let Some(value) = expr.const_value {
        return Ok(value);
    }

    let _ = (expr, SemaDiagnosticCode::NonConstantInRequiredContext);
    todo!("full constant-expression evaluator in const_eval::eval_const_expr")
}
