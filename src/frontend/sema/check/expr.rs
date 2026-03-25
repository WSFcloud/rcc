use crate::common::span::SourceSpan;
use crate::frontend::parser::ast::{
    AssignOp as AstAssignOp, BinaryOp as AstBinaryOp, Expr, ExprKind, IntLiteralSuffix, Literal,
    TypeName, UnaryOp as AstUnaryOp,
};
use crate::frontend::sema::check::decl;
use crate::frontend::sema::const_eval::{self, ConstEvalEnv, ConstExprContext};
use crate::frontend::sema::context::SemaContext;
use crate::frontend::sema::diagnostic::{SemaDiagnostic, SemaDiagnosticCode};
use crate::frontend::sema::init;
use crate::frontend::sema::symbols::{SymbolId, SymbolKind};
use crate::frontend::sema::typed_ast::{
    AssignOp, BinaryOp, ConstValue, TypedExpr, TypedExprKind, UnaryOp, ValueCategory,
};
use crate::frontend::sema::types::{
    ArrayLen, FunctionStyle, Qualifiers, Type, TypeId, TypeKind, assignment_compatible_with_const,
    integer_promotion, is_arithmetic, is_integer, is_scalar, is_void_pointer,
    pointer_comparison_compatible, type_size_of, types_compatible, unqualified,
    usual_arithmetic_conversions,
};

#[derive(Clone, Copy)]
/// Controls which standard conversions should be applied to an expression.
///
/// Different contexts in C enable/disable parts of the conversion pipeline.
/// For example, `sizeof` suppresses array/function decay and lvalue-to-rvalue.
struct ConversionOptions {
    decay_arrays: bool,
    decay_functions: bool,
    lvalue_to_rvalue: bool,
}

impl ConversionOptions {
    /// Full conversion set for ordinary expression contexts.
    const STANDARD: Self = Self {
        decay_arrays: true,
        decay_functions: true,
        lvalue_to_rvalue: true,
    };

    /// Conversion behavior for `sizeof` operands.
    const SIZEOF_OPERAND: Self = Self {
        decay_arrays: false,
        decay_functions: false,
        lvalue_to_rvalue: false,
    };

    /// Conversion behavior for address-of operands.
    const ADDRESS_OPERAND: Self = Self {
        decay_arrays: false,
        decay_functions: false,
        lvalue_to_rvalue: false,
    };
}

/// Lowers an expression and immediately applies context-specific conversions.
fn lower_and_convert(
    cx: &mut SemaContext<'_>,
    expr: &Expr,
    options: ConversionOptions,
) -> TypedExpr {
    let lowered = lower_expr(cx, expr);
    apply_standard_conversions(cx, lowered, options)
}

/// Lowers an expression and applies the ordinary standard conversions.
pub(crate) fn lower_expr_with_standard_conversions(
    cx: &mut SemaContext<'_>,
    expr: &Expr,
) -> TypedExpr {
    lower_and_convert(cx, expr, ConversionOptions::STANDARD)
}

/// Lower parser expressions into typed expressions.
pub fn lower_expr(cx: &mut SemaContext<'_>, expr: &Expr) -> TypedExpr {
    match &expr.kind {
        ExprKind::Var(name) => lower_variable_expr(cx, name, expr.span),
        ExprKind::Literal(lit) => lower_literal_expr(cx, lit, expr.span),
        ExprKind::Unary { op, expr: inner } => lower_unary_expr(cx, *op, inner, expr.span),
        ExprKind::Binary { left, op, right } => lower_binary_expr(cx, left, *op, right, expr.span),
        ExprKind::Assign { left, op, right } => lower_assign_expr(cx, left, *op, right, expr.span),
        ExprKind::Comma { left, right } => lower_comma_expr(cx, left, right, expr.span),
        ExprKind::Conditional {
            cond,
            then_expr,
            else_expr,
        } => lower_conditional_expr(cx, cond, then_expr, else_expr, expr.span),
        ExprKind::Index { base, index } => lower_index_expr(cx, base, index, expr.span),
        ExprKind::Member { base, field, deref } => {
            lower_member_expr(cx, base, field, *deref, expr.span)
        }
        ExprKind::Call { callee, args } => lower_call_expr(cx, callee, args, expr.span),
        ExprKind::Cast { ty, expr: inner } => lower_cast_expr(cx, ty, inner, expr.span),
        ExprKind::SizeofExpr(inner) => lower_sizeof_expr(cx, inner, expr.span),
        ExprKind::SizeofType(ty_name) => lower_sizeof_type_expr(cx, ty_name, expr.span),
        ExprKind::PreInc(inner) => lower_inc_dec_expr(cx, inner, true, true, expr.span),
        ExprKind::PreDec(inner) => lower_inc_dec_expr(cx, inner, false, true, expr.span),
        ExprKind::PostInc(inner) => lower_inc_dec_expr(cx, inner, true, false, expr.span),
        ExprKind::PostDec(inner) => lower_inc_dec_expr(cx, inner, false, false, expr.span),
        ExprKind::CompoundLiteral { ty, init } => {
            lower_compound_literal_expr(cx, ty, init, expr.span)
        }
    }
}

/// Lowers unary expressions and enforces operator-specific operand constraints.
fn lower_unary_expr(
    cx: &mut SemaContext<'_>,
    op: AstUnaryOp,
    inner: &Expr,
    span: SourceSpan,
) -> TypedExpr {
    let raw = lower_expr(cx, inner);
    if is_error_type(cx, raw.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }
    match op {
        AstUnaryOp::Plus => {
            let operand = apply_standard_conversions(cx, raw, ConversionOptions::STANDARD);
            if !is_arithmetic_type(cx, operand.ty) {
                return emit_type_mismatch(cx, span, "unary '+' requires arithmetic operand");
            }
            let mut result_ty = operand.ty;
            if is_integer(&cx.types.get(operand.ty).kind) {
                result_ty = integer_promotion(operand.ty, &mut cx.types);
            }
            let operand = cast_if_needed(operand, result_ty);
            TypedExpr {
                kind: TypedExprKind::Unary {
                    op: UnaryOp::Plus,
                    operand: Box::new(operand.clone()),
                },
                ty: result_ty,
                value_category: ValueCategory::RValue,
                const_value: operand.const_value,
                span,
            }
        }
        AstUnaryOp::Minus => {
            let operand = apply_standard_conversions(cx, raw, ConversionOptions::STANDARD);
            if !is_arithmetic_type(cx, operand.ty) {
                return emit_type_mismatch(cx, span, "unary '-' requires arithmetic operand");
            }
            let mut result_ty = operand.ty;
            if is_integer(&cx.types.get(operand.ty).kind) {
                result_ty = integer_promotion(operand.ty, &mut cx.types);
            }
            let operand = cast_if_needed(operand, result_ty);
            let const_value = if matches!(
                cx.types.get(result_ty).kind,
                TypeKind::Float | TypeKind::Double
            ) {
                operand.const_value.and_then(|v| match v {
                    ConstValue::FloatBits(bits) => {
                        Some(ConstValue::FloatBits((-f64::from_bits(bits)).to_bits()))
                    }
                    _ => None,
                })
            } else {
                const_int_value(operand.const_value)
                    .and_then(|v| v.checked_neg())
                    .map(ConstValue::Int)
            };
            TypedExpr {
                kind: TypedExprKind::Unary {
                    op: UnaryOp::Minus,
                    operand: Box::new(operand),
                },
                ty: result_ty,
                value_category: ValueCategory::RValue,
                const_value,
                span,
            }
        }
        AstUnaryOp::LogicalNot => {
            let operand = apply_standard_conversions(cx, raw, ConversionOptions::STANDARD);
            if !is_scalar(operand.ty, &cx.types) {
                return emit_type_mismatch(cx, span, "logical '!' requires scalar operand");
            }
            let int_ty = int_type(cx);
            let const_value = operand.const_value.and_then(|v| match v {
                ConstValue::Int(i) => Some(ConstValue::Int(i64::from(i == 0))),
                ConstValue::UInt(u) => Some(ConstValue::Int(i64::from(u == 0))),
                ConstValue::FloatBits(bits) => {
                    Some(ConstValue::Int(i64::from(f64::from_bits(bits) == 0.0)))
                }
                ConstValue::NullPtr => Some(ConstValue::Int(1)),
                ConstValue::Addr { .. } => Some(ConstValue::Int(0)),
            });
            TypedExpr {
                kind: TypedExprKind::Unary {
                    op: UnaryOp::LogicalNot,
                    operand: Box::new(operand),
                },
                ty: int_ty,
                value_category: ValueCategory::RValue,
                const_value,
                span,
            }
        }
        AstUnaryOp::BitNot => {
            let operand = apply_standard_conversions(cx, raw, ConversionOptions::STANDARD);
            if !is_integer(&cx.types.get(operand.ty).kind) {
                return emit_type_mismatch(cx, span, "bitwise '~' requires integer operand");
            }
            let result_ty = integer_promotion(operand.ty, &mut cx.types);
            let operand = cast_if_needed(operand, result_ty);
            let const_value = const_int_value(operand.const_value).map(|v| ConstValue::Int(!v));
            TypedExpr {
                kind: TypedExprKind::Unary {
                    op: UnaryOp::BitwiseNot,
                    operand: Box::new(operand),
                },
                ty: result_ty,
                value_category: ValueCategory::RValue,
                const_value,
                span,
            }
        }
        AstUnaryOp::AddressOf => {
            let operand = apply_standard_conversions(cx, raw, ConversionOptions::ADDRESS_OPERAND);
            if !matches!(
                operand.value_category,
                ValueCategory::LValue
                    | ValueCategory::ArrayDesignator
                    | ValueCategory::FunctionDesignator
            ) {
                return emit_type_mismatch(
                    cx,
                    span,
                    "operator '&' requires an lvalue, array, or function designator",
                );
            }

            let ptr_ty = cx.types.intern(Type {
                kind: TypeKind::Pointer {
                    pointee: operand.ty,
                },
                quals: Qualifiers::default(),
            });
            let const_value = address_of_operand_const(cx, &operand);
            TypedExpr {
                kind: TypedExprKind::Unary {
                    op: UnaryOp::AddrOf,
                    operand: Box::new(operand),
                },
                ty: ptr_ty,
                value_category: ValueCategory::RValue,
                const_value,
                span,
            }
        }
        AstUnaryOp::Deref => {
            let operand = apply_standard_conversions(cx, raw, ConversionOptions::STANDARD);
            let TypeKind::Pointer { pointee } = cx.types.get(operand.ty).kind else {
                return emit_type_mismatch(cx, span, "operator '*' requires pointer operand");
            };
            if matches!(cx.types.get(pointee).kind, TypeKind::Void) {
                return emit_type_mismatch(cx, span, "cannot dereference 'void *'");
            }
            TypedExpr {
                kind: TypedExprKind::Unary {
                    op: UnaryOp::Deref,
                    operand: Box::new(operand),
                },
                ty: pointee,
                value_category: value_category_for_designator_type(cx, pointee),
                const_value: None,
                span,
            }
        }
    }
}

/// Lowers pre/post increment and decrement expressions.
///
/// The operand must be a modifiable lvalue of arithmetic or pointer type.
fn lower_inc_dec_expr(
    cx: &mut SemaContext<'_>,
    inner: &Expr,
    increment: bool,
    prefix: bool,
    span: SourceSpan,
) -> TypedExpr {
    let operand = lower_expr(cx, inner);
    if is_error_type(cx, operand.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }
    if !is_modifiable_lvalue(cx, &operand) {
        return emit_type_mismatch(cx, span, "increment/decrement requires a modifiable lvalue");
    }

    let operand_ty = operand.ty;
    let operand_kind = &cx.types.get(operand_ty).kind;
    if !is_arithmetic(operand_kind) && !matches!(operand_kind, TypeKind::Pointer { .. }) {
        return emit_type_mismatch(
            cx,
            span,
            "increment/decrement operand must be arithmetic or pointer",
        );
    }

    let op = match (increment, prefix) {
        (true, true) => UnaryOp::PreInc,
        (false, true) => UnaryOp::PreDec,
        (true, false) => UnaryOp::PostInc,
        (false, false) => UnaryOp::PostDec,
    };

    TypedExpr {
        kind: TypedExprKind::Unary {
            op,
            operand: Box::new(operand),
        },
        ty: operand_ty,
        value_category: ValueCategory::RValue,
        const_value: None,
        span,
    }
}

/// Lowers binary expressions, applying usual arithmetic conversions and
/// pointer-specific rules where required.
fn lower_binary_expr(
    cx: &mut SemaContext<'_>,
    left_ast: &Expr,
    op: AstBinaryOp,
    right_ast: &Expr,
    span: SourceSpan,
) -> TypedExpr {
    let mut left = lower_and_convert(cx, left_ast, ConversionOptions::STANDARD);
    let mut right = lower_and_convert(cx, right_ast, ConversionOptions::STANDARD);
    if is_error_type(cx, left.ty) || is_error_type(cx, right.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }

    let (typed_op, result_ty) = match op {
        AstBinaryOp::Mul | AstBinaryOp::Div | AstBinaryOp::Mod => {
            if !is_arithmetic_type(cx, left.ty) || !is_arithmetic_type(cx, right.ty) {
                return emit_type_mismatch(
                    cx,
                    span,
                    "arithmetic binary operator requires arithmetic operands",
                );
            }
            if matches!(op, AstBinaryOp::Mod)
                && (!is_integer(&cx.types.get(left.ty).kind)
                    || !is_integer(&cx.types.get(right.ty).kind))
            {
                return emit_type_mismatch(cx, span, "operator '%' requires integer operands");
            }
            let common = usual_arithmetic_conversions(left.ty, right.ty, &mut cx.types);
            left = cast_if_needed(left, common);
            right = cast_if_needed(right, common);
            (map_binary_op(op), common)
        }
        AstBinaryOp::Add => {
            if is_arithmetic_type(cx, left.ty) && is_arithmetic_type(cx, right.ty) {
                let common = usual_arithmetic_conversions(left.ty, right.ty, &mut cx.types);
                left = cast_if_needed(left, common);
                right = cast_if_needed(right, common);
                (BinaryOp::Add, common)
            } else if is_pointer_type(cx, left.ty) && is_integer(&cx.types.get(right.ty).kind) {
                (BinaryOp::Add, left.ty)
            } else if is_pointer_type(cx, right.ty) && is_integer(&cx.types.get(left.ty).kind) {
                (BinaryOp::Add, right.ty)
            } else {
                return emit_type_mismatch(
                    cx,
                    span,
                    "operator '+' requires arithmetic operands or pointer+integer",
                );
            }
        }
        AstBinaryOp::Sub => {
            if is_arithmetic_type(cx, left.ty) && is_arithmetic_type(cx, right.ty) {
                let common = usual_arithmetic_conversions(left.ty, right.ty, &mut cx.types);
                left = cast_if_needed(left, common);
                right = cast_if_needed(right, common);
                (BinaryOp::Sub, common)
            } else if is_pointer_type(cx, left.ty) && is_integer(&cx.types.get(right.ty).kind) {
                (BinaryOp::Sub, left.ty)
            } else if is_pointer_type(cx, left.ty) && is_pointer_type(cx, right.ty) {
                if !pointer_comparison_compatible(left.ty, right.ty, &cx.types, false) {
                    return emit_type_mismatch(
                        cx,
                        span,
                        "pointer subtraction requires compatible pointee types",
                    );
                }
                (BinaryOp::Sub, long_type(cx, true))
            } else {
                return emit_type_mismatch(
                    cx,
                    span,
                    "operator '-' requires arithmetic operands, pointer-integer, or pointer-pointer",
                );
            }
        }
        AstBinaryOp::Shl | AstBinaryOp::Shr => {
            if !is_integer(&cx.types.get(left.ty).kind) || !is_integer(&cx.types.get(right.ty).kind)
            {
                return emit_type_mismatch(cx, span, "shift operators require integer operands");
            }
            let left_promoted = integer_promotion(left.ty, &mut cx.types);
            let right_promoted = integer_promotion(right.ty, &mut cx.types);
            left = cast_if_needed(left, left_promoted);
            right = cast_if_needed(right, right_promoted);
            (map_binary_op(op), left_promoted)
        }
        AstBinaryOp::BitAnd | AstBinaryOp::BitOr | AstBinaryOp::BitXor => {
            if !is_integer(&cx.types.get(left.ty).kind) || !is_integer(&cx.types.get(right.ty).kind)
            {
                return emit_type_mismatch(cx, span, "bitwise operators require integer operands");
            }
            let common = usual_arithmetic_conversions(left.ty, right.ty, &mut cx.types);
            left = cast_if_needed(left, common);
            right = cast_if_needed(right, common);
            (map_binary_op(op), common)
        }
        AstBinaryOp::Lt
        | AstBinaryOp::Le
        | AstBinaryOp::Gt
        | AstBinaryOp::Ge
        | AstBinaryOp::Eq
        | AstBinaryOp::Ne => {
            if is_arithmetic_type(cx, left.ty) && is_arithmetic_type(cx, right.ty) {
                let common = usual_arithmetic_conversions(left.ty, right.ty, &mut cx.types);
                left = cast_if_needed(left, common);
                right = cast_if_needed(right, common);
            } else if is_pointer_type(cx, left.ty) && is_pointer_type(cx, right.ty) {
                let compatible = pointer_comparison_compatible(
                    left.ty,
                    right.ty,
                    &cx.types,
                    matches!(op, AstBinaryOp::Eq | AstBinaryOp::Ne),
                );
                if !compatible {
                    return emit_type_mismatch(
                        cx,
                        span,
                        "pointer comparison requires compatible pointer types",
                    );
                }
            } else if is_pointer_type(cx, left.ty) && is_null_pointer_constant(&right) {
                right = cast_if_needed(right, left.ty);
            } else if is_pointer_type(cx, right.ty) && is_null_pointer_constant(&left) {
                left = cast_if_needed(left, right.ty);
            } else {
                return emit_type_mismatch(
                    cx,
                    span,
                    "comparison requires arithmetic or pointer-compatible operands",
                );
            }
            (map_binary_op(op), int_type(cx))
        }
        AstBinaryOp::LogicalAnd | AstBinaryOp::LogicalOr => {
            if !is_scalar(left.ty, &cx.types) || !is_scalar(right.ty, &cx.types) {
                return emit_type_mismatch(cx, span, "logical operators require scalar operands");
            }
            (map_binary_op(op), int_type(cx))
        }
    };

    TypedExpr {
        kind: TypedExprKind::Binary {
            op: typed_op,
            left: Box::new(left),
            right: Box::new(right),
        },
        ty: result_ty,
        value_category: ValueCategory::RValue,
        const_value: None,
        span,
    }
}

/// Lowers assignment and compound-assignment expressions.
///
/// This routine validates modifiable-lvalue constraints for the left operand
/// and checks assignment compatibility after required intermediate conversions.
fn lower_assign_expr(
    cx: &mut SemaContext<'_>,
    left_ast: &Expr,
    op: AstAssignOp,
    right_ast: &Expr,
    span: SourceSpan,
) -> TypedExpr {
    let lhs = lower_expr(cx, left_ast);
    let lhs_ty = lhs.ty;
    if is_error_type(cx, lhs_ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }
    if !is_modifiable_lvalue(cx, &lhs) {
        return emit_type_mismatch(
            cx,
            span,
            "assignment requires a modifiable lvalue on the left",
        );
    }

    let mut rhs = lower_and_convert(cx, right_ast, ConversionOptions::STANDARD);
    if is_error_type(cx, rhs.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }

    if matches!(op, AstAssignOp::Assign) {
        let rhs_const_int = const_int_value(rhs.const_value);
        if !assignment_compatible_with_const(rhs.ty, rhs_const_int, lhs_ty, &cx.types) {
            return emit_type_mismatch(cx, span, "incompatible assignment operands");
        }
        rhs = cast_if_needed(rhs, lhs_ty);
        return TypedExpr {
            kind: TypedExprKind::Assign {
                op: AssignOp::Assign,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            ty: lhs_ty,
            value_category: ValueCategory::RValue,
            const_value: None,
            span,
        };
    }

    let lhs_rvalue = apply_standard_conversions(cx, lhs.clone(), ConversionOptions::STANDARD);
    let compound_result_ty = match op {
        AstAssignOp::AddAssign => {
            if is_arithmetic_type(cx, lhs_rvalue.ty) && is_arithmetic_type(cx, rhs.ty) {
                usual_arithmetic_conversions(lhs_rvalue.ty, rhs.ty, &mut cx.types)
            } else if is_pointer_type(cx, lhs_rvalue.ty) && is_integer(&cx.types.get(rhs.ty).kind) {
                lhs_rvalue.ty
            } else {
                return emit_type_mismatch(
                    cx,
                    span,
                    "'+=' requires arithmetic operands or pointer+integer",
                );
            }
        }
        AstAssignOp::SubAssign => {
            if is_arithmetic_type(cx, lhs_rvalue.ty) && is_arithmetic_type(cx, rhs.ty) {
                usual_arithmetic_conversions(lhs_rvalue.ty, rhs.ty, &mut cx.types)
            } else if is_pointer_type(cx, lhs_rvalue.ty) && is_integer(&cx.types.get(rhs.ty).kind) {
                lhs_rvalue.ty
            } else {
                return emit_type_mismatch(
                    cx,
                    span,
                    "'-=' requires arithmetic operands or pointer-integer",
                );
            }
        }
        AstAssignOp::MulAssign | AstAssignOp::DivAssign => {
            if !is_arithmetic_type(cx, lhs_rvalue.ty) || !is_arithmetic_type(cx, rhs.ty) {
                return emit_type_mismatch(
                    cx,
                    span,
                    "compound assignment requires arithmetic operands",
                );
            }
            usual_arithmetic_conversions(lhs_rvalue.ty, rhs.ty, &mut cx.types)
        }
        AstAssignOp::ModAssign => {
            if !is_integer(&cx.types.get(lhs_rvalue.ty).kind)
                || !is_integer(&cx.types.get(rhs.ty).kind)
            {
                return emit_type_mismatch(cx, span, "'%=' requires integer operands");
            }
            usual_arithmetic_conversions(lhs_rvalue.ty, rhs.ty, &mut cx.types)
        }
        AstAssignOp::ShlAssign | AstAssignOp::ShrAssign => {
            if !is_integer(&cx.types.get(lhs_rvalue.ty).kind)
                || !is_integer(&cx.types.get(rhs.ty).kind)
            {
                return emit_type_mismatch(
                    cx,
                    span,
                    "shift compound assignment requires integer operands",
                );
            }
            let lhs_promoted = integer_promotion(lhs_rvalue.ty, &mut cx.types);
            let rhs_promoted = integer_promotion(rhs.ty, &mut cx.types);
            rhs = cast_if_needed(rhs, rhs_promoted);
            lhs_promoted
        }
        AstAssignOp::BitAndAssign | AstAssignOp::BitXorAssign | AstAssignOp::BitOrAssign => {
            if !is_integer(&cx.types.get(lhs_rvalue.ty).kind)
                || !is_integer(&cx.types.get(rhs.ty).kind)
            {
                return emit_type_mismatch(
                    cx,
                    span,
                    "bitwise compound assignment requires integer operands",
                );
            }
            usual_arithmetic_conversions(lhs_rvalue.ty, rhs.ty, &mut cx.types)
        }
        AstAssignOp::Assign => unreachable!(),
    };

    if !assignment_compatible_with_const(compound_result_ty, None, lhs_ty, &cx.types) {
        return emit_type_mismatch(
            cx,
            span,
            "compound assignment result is not assignable to left operand",
        );
    }

    TypedExpr {
        kind: TypedExprKind::Assign {
            op: map_assign_op(op),
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        ty: lhs_rvalue.ty,
        value_category: ValueCategory::RValue,
        const_value: None,
        span,
    }
}

/// Lowers the conditional operator (`cond ? then : else`) and derives the
/// composite result type for the two branch expressions.
fn lower_conditional_expr(
    cx: &mut SemaContext<'_>,
    cond_ast: &Expr,
    then_ast: &Expr,
    else_ast: &Expr,
    span: SourceSpan,
) -> TypedExpr {
    let cond = lower_and_convert(cx, cond_ast, ConversionOptions::STANDARD);
    if is_error_type(cx, cond.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }
    if !is_scalar(cond.ty, &cx.types) {
        return emit_type_mismatch(cx, span, "conditional expression requires scalar condition");
    }

    let mut then_expr = lower_and_convert(cx, then_ast, ConversionOptions::STANDARD);
    let mut else_expr = lower_and_convert(cx, else_ast, ConversionOptions::STANDARD);
    if is_error_type(cx, then_expr.ty) || is_error_type(cx, else_expr.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }

    let result_ty = if is_arithmetic_type(cx, then_expr.ty) && is_arithmetic_type(cx, else_expr.ty)
    {
        let common = usual_arithmetic_conversions(then_expr.ty, else_expr.ty, &mut cx.types);
        then_expr = cast_if_needed(then_expr, common);
        else_expr = cast_if_needed(else_expr, common);
        common
    } else if types_compatible(then_expr.ty, else_expr.ty, &cx.types) {
        then_expr.ty
    } else if is_pointer_type(cx, then_expr.ty) && is_pointer_type(cx, else_expr.ty) {
        if !pointer_comparison_compatible(then_expr.ty, else_expr.ty, &cx.types, true) {
            return emit_type_mismatch(
                cx,
                span,
                "conditional pointer operands must have compatible pointee types",
            );
        }

        if assignment_compatible_with_const(then_expr.ty, None, else_expr.ty, &cx.types) {
            else_expr.ty
        } else {
            then_expr.ty
        }
    } else if is_pointer_type(cx, then_expr.ty) && is_null_pointer_constant(&else_expr) {
        then_expr.ty
    } else if is_pointer_type(cx, else_expr.ty) && is_null_pointer_constant(&then_expr) {
        else_expr.ty
    } else {
        return emit_type_mismatch(cx, span, "conditional branches have incompatible types");
    };

    then_expr = cast_if_needed(then_expr, result_ty);
    else_expr = cast_if_needed(else_expr, result_ty);

    TypedExpr {
        kind: TypedExprKind::Conditional {
            cond: Box::new(cond),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
        },
        ty: result_ty,
        value_category: ValueCategory::RValue,
        const_value: None,
        span,
    }
}

/// Lowers a function call expression and validates argument compatibility.
fn lower_call_expr(
    cx: &mut SemaContext<'_>,
    callee_ast: &Expr,
    args_ast: &[Expr],
    span: SourceSpan,
) -> TypedExpr {
    let callee_raw = lower_expr(cx, callee_ast);
    let callee = apply_standard_conversions(cx, callee_raw, ConversionOptions::STANDARD);
    if is_error_type(cx, callee.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }

    let function_ty = match &cx.types.get(callee.ty).kind {
        TypeKind::Function(func) => func.clone(),
        TypeKind::Pointer { pointee } => match &cx.types.get(*pointee).kind {
            TypeKind::Function(func) => func.clone(),
            _ => {
                return emit_type_mismatch(
                    cx,
                    span,
                    "call expression requires function or pointer-to-function callee",
                );
            }
        },
        _ => {
            return emit_type_mismatch(
                cx,
                span,
                "call expression requires function or pointer-to-function callee",
            );
        }
    };

    let mut args = Vec::with_capacity(args_ast.len());
    for arg in args_ast {
        args.push(lower_and_convert(cx, arg, ConversionOptions::STANDARD));
    }

    if matches!(function_ty.style, FunctionStyle::Prototype) {
        if !function_ty.variadic && args.len() != function_ty.params.len() {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                format!(
                    "function expects {} arguments but {} provided",
                    function_ty.params.len(),
                    args.len()
                ),
                span,
            ));
        } else if function_ty.variadic && args.len() < function_ty.params.len() {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                format!(
                    "function expects at least {} arguments but {} provided",
                    function_ty.params.len(),
                    args.len()
                ),
                span,
            ));
        }

        for (idx, param_ty) in function_ty.params.iter().copied().enumerate() {
            if idx >= args.len() {
                break;
            }
            if is_error_type(cx, args[idx].ty) {
                continue;
            }
            let arg_const = const_int_value(args[idx].const_value);
            if !assignment_compatible_with_const(args[idx].ty, arg_const, param_ty, &cx.types) {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    format!("argument {} has incompatible type", idx + 1),
                    args[idx].span,
                ));
                args[idx] = TypedExpr::opaque(args[idx].span, cx.error_type());
            } else {
                args[idx] = cast_if_needed(args[idx].clone(), param_ty);
            }
        }

        if function_ty.variadic {
            for arg in args.iter_mut().skip(function_ty.params.len()) {
                *arg = default_argument_promotion(cx, arg.clone());
            }
        }
    } else {
        for arg in &mut args {
            *arg = default_argument_promotion(cx, arg.clone());
        }
    }

    TypedExpr {
        kind: TypedExprKind::Call {
            func: Box::new(callee),
            args,
        },
        ty: function_ty.ret,
        value_category: ValueCategory::RValue,
        const_value: None,
        span,
    }
}

/// Lowers record member access via `.` and `->`.
fn lower_member_expr(
    cx: &mut SemaContext<'_>,
    base_ast: &Expr,
    field_name: &str,
    deref: bool,
    span: SourceSpan,
) -> TypedExpr {
    let base = lower_expr(cx, base_ast);
    if is_error_type(cx, base.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }

    let (record_id, base_expr, base_is_lvalue) = if deref {
        let base_converted = apply_standard_conversions(cx, base, ConversionOptions::STANDARD);
        let TypeKind::Pointer { pointee } = cx.types.get(base_converted.ty).kind else {
            return emit_type_mismatch(cx, span, "'->' requires pointer-to-record operand");
        };
        let TypeKind::Record(record_id) = cx.types.get(pointee).kind else {
            return emit_type_mismatch(cx, span, "'->' requires pointer-to-record operand");
        };
        (record_id, base_converted, true)
    } else {
        let TypeKind::Record(record_id) = cx.types.get(base.ty).kind else {
            return emit_type_mismatch(cx, span, "'.' requires record operand");
        };
        (
            record_id,
            base.clone(),
            matches!(
                base.value_category,
                ValueCategory::LValue | ValueCategory::ArrayDesignator
            ),
        )
    };

    let record = cx.records.get(record_id);
    if !record.is_complete {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::IncompleteType,
            "member access requires complete record type",
            span,
        ));
        return TypedExpr::opaque(span, cx.error_type());
    }
    let Some((field_idx, field)) = record
        .fields
        .iter()
        .enumerate()
        .find(|(_, field)| field.name.as_deref() == Some(field_name))
    else {
        return emit_type_mismatch(
            cx,
            span,
            format!("record has no member named '{field_name}'"),
        );
    };

    let value_category = if base_is_lvalue {
        value_category_for_designator_type(cx, field.ty)
    } else {
        ValueCategory::RValue
    };

    TypedExpr {
        kind: TypedExprKind::MemberAccess {
            base: Box::new(base_expr),
            field: crate::frontend::sema::types::FieldId(field_idx as u32),
            deref,
        },
        ty: field.ty,
        value_category,
        const_value: None,
        span,
    }
}

/// Lowers array subscripting (`base[index]`), accepting both `ptr + int` and
/// the commuted `int + ptr` form.
fn lower_index_expr(
    cx: &mut SemaContext<'_>,
    base_ast: &Expr,
    index_ast: &Expr,
    span: SourceSpan,
) -> TypedExpr {
    let base = lower_and_convert(cx, base_ast, ConversionOptions::STANDARD);
    let index = lower_and_convert(cx, index_ast, ConversionOptions::STANDARD);
    if is_error_type(cx, base.ty) || is_error_type(cx, index.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }

    let pointee = if let Some(pointee) = pointee_of_pointer(cx, base.ty) {
        if !is_integer(&cx.types.get(index.ty).kind) {
            return emit_type_mismatch(cx, span, "array index must have integer type");
        }
        pointee
    } else if let Some(pointee) = pointee_of_pointer(cx, index.ty) {
        if !is_integer(&cx.types.get(base.ty).kind) {
            return emit_type_mismatch(cx, span, "array index must have integer type");
        }
        pointee
    } else {
        return emit_type_mismatch(
            cx,
            span,
            "subscripted value must be pointer and index must be integer",
        );
    };

    let const_value = index_expr_const_address(cx, &base, &index);

    TypedExpr {
        kind: TypedExprKind::Index {
            base: Box::new(base),
            index: Box::new(index),
        },
        ty: pointee,
        value_category: value_category_for_designator_type(cx, pointee),
        const_value,
        span,
    }
}

fn index_expr_const_address(
    cx: &SemaContext<'_>,
    base: &TypedExpr,
    index: &TypedExpr,
) -> Option<ConstValue> {
    let (ptr_expr, int_expr, pointee) = if let Some(pointee) = pointee_of_pointer(cx, base.ty) {
        (base, index, pointee)
    } else if let Some(pointee) = pointee_of_pointer(cx, index.ty) {
        (index, base, pointee)
    } else {
        return None;
    };

    let (symbol, base_offset) = pointer_const_address(cx, ptr_expr)?;
    let index_value = integer_constant_value(cx, int_expr)?;
    let elem_size = i64::try_from(type_size_of(pointee, &cx.types, &cx.records)?).ok()?;
    let index_offset = index_value.checked_mul(elem_size)?;
    let offset = base_offset.checked_add(index_offset)?;
    Some(ConstValue::Addr { symbol, offset })
}

/// Lowers explicit cast expressions and validates allowed cast categories.
fn lower_cast_expr(
    cx: &mut SemaContext<'_>,
    ty_name: &TypeName,
    inner: &Expr,
    span: SourceSpan,
) -> TypedExpr {
    let to = decl::build_type_from_type_name(cx, ty_name, span);
    let expr = lower_and_convert(cx, inner, ConversionOptions::STANDARD);
    if is_error_type(cx, to) || is_error_type(cx, expr.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }

    if !is_valid_cast(cx, expr.ty, to) {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidCast,
            "invalid cast between operand and target type",
            span,
        ));
        return TypedExpr::opaque(span, cx.error_type());
    }

    TypedExpr {
        kind: TypedExprKind::Cast {
            expr: Box::new(expr),
            to,
        },
        ty: to,
        value_category: ValueCategory::RValue,
        const_value: None,
        span,
    }
}

fn address_of_operand_const(cx: &SemaContext<'_>, operand: &TypedExpr) -> Option<ConstValue> {
    let (symbol, offset) = address_of_operand_symbol_offset(cx, operand)?;
    Some(ConstValue::Addr { symbol, offset })
}

fn address_of_operand_symbol_offset(
    cx: &SemaContext<'_>,
    operand: &TypedExpr,
) -> Option<(SymbolId, i64)> {
    if let Some(ConstValue::Addr { symbol, offset }) = operand.const_value {
        return Some((symbol, offset));
    }

    match &operand.kind {
        TypedExprKind::SymbolRef(symbol) => Some((*symbol, 0)),
        TypedExprKind::ImplicitCast { expr, .. } | TypedExprKind::Cast { expr, .. } => {
            address_of_operand_symbol_offset(cx, expr)
        }
        TypedExprKind::Unary {
            op: UnaryOp::Deref,
            operand: inner,
        } => pointer_const_address(cx, inner),
        TypedExprKind::Index { base, index } => {
            let (ptr_expr, int_expr, pointee) =
                if let Some(pointee) = pointee_of_pointer(cx, base.ty) {
                    (base.as_ref(), index.as_ref(), pointee)
                } else if let Some(pointee) = pointee_of_pointer(cx, index.ty) {
                    (index.as_ref(), base.as_ref(), pointee)
                } else {
                    return None;
                };
            let (symbol, base_offset) = pointer_const_address(cx, ptr_expr)?;
            let index_value = integer_constant_value(cx, int_expr)?;
            let elem_size = i64::try_from(type_size_of(pointee, &cx.types, &cx.records)?).ok()?;
            let index_offset = index_value.checked_mul(elem_size)?;
            let offset = base_offset.checked_add(index_offset)?;
            Some((symbol, offset))
        }
        TypedExprKind::MemberAccess { base, field, deref } => {
            let (symbol, base_offset, record_id) = if *deref {
                let TypeKind::Pointer { pointee } = cx.types.get(base.ty).kind else {
                    return None;
                };
                let TypeKind::Record(record_id) = cx.types.get(pointee).kind else {
                    return None;
                };
                let (symbol, offset) = pointer_const_address(cx, base)?;
                (symbol, offset, record_id)
            } else {
                let TypeKind::Record(record_id) = cx.types.get(base.ty).kind else {
                    return None;
                };
                let (symbol, offset) = address_of_operand_symbol_offset(cx, base)?;
                (symbol, offset, record_id)
            };
            let field_offset = record_field_offset(cx, record_id, *field)?;
            let offset = base_offset.checked_add(field_offset)?;
            Some((symbol, offset))
        }
        _ => None,
    }
}

fn pointer_const_address(cx: &SemaContext<'_>, expr: &TypedExpr) -> Option<(SymbolId, i64)> {
    if let Some(ConstValue::Addr { symbol, offset }) = expr.const_value {
        return Some((symbol, offset));
    }

    match &expr.kind {
        TypedExprKind::Unary {
            op: UnaryOp::AddrOf,
            operand,
        } => address_of_operand_symbol_offset(cx, operand),
        TypedExprKind::ImplicitCast { expr: inner, .. }
        | TypedExprKind::Cast { expr: inner, .. } => {
            if !is_pointer_type(cx, expr.ty) {
                return None;
            }
            pointer_const_address(cx, inner)
        }
        TypedExprKind::Binary {
            op: BinaryOp::Add,
            left,
            right,
        } => pointer_integer_offset_const_address(cx, left, right, false)
            .or_else(|| pointer_integer_offset_const_address(cx, right, left, false)),
        TypedExprKind::Binary {
            op: BinaryOp::Sub,
            left,
            right,
        } => pointer_integer_offset_const_address(cx, left, right, true),
        TypedExprKind::MemberAccess { .. }
            if matches!(
                expr.value_category,
                ValueCategory::LValue
                    | ValueCategory::ArrayDesignator
                    | ValueCategory::FunctionDesignator
            ) =>
        {
            address_of_operand_symbol_offset(cx, expr)
        }
        TypedExprKind::SymbolRef(symbol)
            if matches!(
                expr.value_category,
                ValueCategory::ArrayDesignator | ValueCategory::FunctionDesignator
            ) =>
        {
            Some((*symbol, 0))
        }
        _ => None,
    }
}

fn pointer_integer_offset_const_address(
    cx: &SemaContext<'_>,
    ptr_expr: &TypedExpr,
    int_expr: &TypedExpr,
    negate_index: bool,
) -> Option<(SymbolId, i64)> {
    let pointee = pointee_of_pointer(cx, ptr_expr.ty)?;
    let (symbol, base_offset) = pointer_const_address(cx, ptr_expr)?;
    let mut index = integer_constant_value(cx, int_expr)?;
    if negate_index {
        index = index.checked_neg()?;
    }
    let elem_size = i64::try_from(type_size_of(pointee, &cx.types, &cx.records)?).ok()?;
    let delta = index.checked_mul(elem_size)?;
    Some((symbol, base_offset.checked_add(delta)?))
}

fn integer_constant_value(cx: &SemaContext<'_>, expr: &TypedExpr) -> Option<i64> {
    if let Some(v) = const_int_value(expr.const_value) {
        return Some(v);
    }

    let env = ConstEvalEnv {
        types: &cx.types,
        records: &cx.records,
    };
    match const_eval::eval_const_expr(expr, ConstExprContext::IntegerConstant, &env).ok()? {
        ConstValue::Int(v) => Some(v),
        ConstValue::UInt(v) => i64::try_from(v).ok(),
        _ => None,
    }
}

/// Returns `true` when an expression is representable as a C address constant.
pub(crate) fn is_address_constant_expr(cx: &SemaContext<'_>, expr: &TypedExpr) -> bool {
    if !is_pointer_type(cx, expr.ty) {
        return false;
    }

    if matches!(
        expr.const_value,
        Some(ConstValue::Addr { .. }) | Some(ConstValue::NullPtr)
    ) {
        return true;
    }

    if pointer_const_address(cx, expr).is_some() {
        return true;
    }

    // String literal decay produces an implicit cast from an opaque const-char array.
    match &expr.kind {
        TypedExprKind::ImplicitCast { expr: inner, .. }
        | TypedExprKind::Cast { expr: inner, .. } => {
            if !is_pointer_type(cx, expr.ty) {
                return false;
            }
            if integer_constant_value(cx, inner).is_some() {
                return true;
            }
            if matches!(inner.kind, TypedExprKind::Opaque)
                && matches!(inner.value_category, ValueCategory::ArrayDesignator)
            {
                if let TypeKind::Array { elem, .. } = cx.types.get(inner.ty).kind {
                    let elem_ty = cx.types.get(elem);
                    if elem_ty.quals.is_const
                        && matches!(
                            elem_ty.kind,
                            TypeKind::Char | TypeKind::SignedChar | TypeKind::UnsignedChar
                        )
                    {
                        return true;
                    }
                }
            }
            // File-scope compound literal with array type decays to a pointer —
            // that pointer is an address constant (C99 6.5.2.5p5).
            if matches!(
                inner.kind,
                TypedExprKind::CompoundLiteral {
                    is_file_scope: true,
                    ..
                }
            ) {
                return true;
            }
            is_address_constant_expr(cx, inner)
        }
        // C99 6.6p9: a conditional with a constant condition and address-constant
        // branches is an address constant expression.
        TypedExprKind::Conditional {
            cond,
            then_expr,
            else_expr,
        } => {
            integer_constant_value(cx, cond).is_some()
                && is_address_constant_expr(cx, then_expr)
                && is_address_constant_expr(cx, else_expr)
        }
        _ => false,
    }
}

fn record_field_offset(
    cx: &SemaContext<'_>,
    record_id: crate::frontend::sema::types::RecordId,
    field_id: crate::frontend::sema::types::FieldId,
) -> Option<i64> {
    let record = cx.records.get(record_id);
    let field_index = field_id.0 as usize;
    if field_index >= record.fields.len() {
        return None;
    }

    match record.kind {
        crate::frontend::parser::ast::RecordKind::Struct => {
            let mut offset = 0i64;
            for field in record.fields.iter().take(field_index) {
                let size = i64::try_from(type_size_of(field.ty, &cx.types, &cx.records)?).ok()?;
                offset = offset.checked_add(size)?;
            }
            Some(offset)
        }
        crate::frontend::parser::ast::RecordKind::Union => Some(0),
    }
}

/// Lowers `sizeof(type-name)` expressions.
fn lower_sizeof_type_expr(
    cx: &mut SemaContext<'_>,
    ty_name: &TypeName,
    span: SourceSpan,
) -> TypedExpr {
    let ty = decl::build_type_from_type_name(cx, ty_name, span);
    lower_sizeof_ty(cx, ty, span)
}

/// Lowers `sizeof(expr)` expressions without applying decay conversions to the
/// operand, matching C semantics.
fn lower_sizeof_expr(cx: &mut SemaContext<'_>, inner: &Expr, span: SourceSpan) -> TypedExpr {
    let operand = lower_and_convert(cx, inner, ConversionOptions::SIZEOF_OPERAND);
    if is_error_type(cx, operand.ty) {
        return TypedExpr::opaque(span, cx.error_type());
    }
    lower_sizeof_ty(cx, operand.ty, span)
}

/// Builds a typed `sizeof` expression from a resolved operand type.
fn lower_sizeof_ty(cx: &mut SemaContext<'_>, ty: TypeId, span: SourceSpan) -> TypedExpr {
    let Some(size) = type_size_of(ty, &cx.types, &cx.records) else {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::IncompleteType,
            "operand of sizeof has incomplete or unsupported type",
            span,
        ));
        return TypedExpr::opaque(span, cx.error_type());
    };

    TypedExpr {
        kind: TypedExprKind::SizeofType { ty },
        ty: size_t_type(cx),
        value_category: ValueCategory::RValue,
        const_value: Some(ConstValue::UInt(size)),
        span,
    }
}

/// Lowers comma expressions, preserving only the right operand's type/value.
fn lower_comma_expr(
    cx: &mut SemaContext<'_>,
    left_ast: &Expr,
    right_ast: &Expr,
    span: SourceSpan,
) -> TypedExpr {
    let left = lower_and_convert(cx, left_ast, ConversionOptions::STANDARD);
    let right = lower_and_convert(cx, right_ast, ConversionOptions::STANDARD);
    TypedExpr {
        kind: TypedExprKind::Comma {
            left: Box::new(left),
            right: Box::new(right.clone()),
        },
        ty: right.ty,
        value_category: ValueCategory::RValue,
        const_value: right.const_value,
        span,
    }
}

fn lower_compound_literal_expr(
    cx: &mut SemaContext<'_>,
    ty_name: &TypeName,
    init: &crate::frontend::parser::ast::Initializer,
    span: SourceSpan,
) -> TypedExpr {
    let ty = decl::build_type_from_type_name(cx, ty_name, span);
    let lowered = init::lower_initializer(cx, ty, init);
    let ty = lowered.resulting_ty;
    let typed_init = lowered.init;
    let is_file_scope = cx.scope_level() == crate::frontend::sema::symbols::ScopeLevel::File;

    TypedExpr {
        kind: TypedExprKind::CompoundLiteral {
            ty,
            init: Box::new(typed_init),
            is_file_scope,
        },
        ty,
        value_category: value_category_for_designator_type(cx, ty),
        const_value: None,
        span,
    }
}

/// Lowers a variable reference expression.
fn lower_variable_expr(cx: &mut SemaContext<'_>, name: &str, span: SourceSpan) -> TypedExpr {
    if let Some(symbol_id) = cx.resolve_ordinary(name, span) {
        let symbol = cx.symbol(symbol_id);
        let mut typed = TypedExpr::symbol(symbol_id, span, symbol.ty());
        typed.value_category = match symbol.kind() {
            SymbolKind::EnumConst => ValueCategory::RValue,
            _ => value_category_for_designator_type(cx, symbol.ty()),
        };

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

/// Lowers a literal expression with basic C type inference.
fn lower_literal_expr(cx: &mut SemaContext<'_>, lit: &Literal, span: SourceSpan) -> TypedExpr {
    match lit {
        Literal::Int { value, base } => {
            let (ty, value) = infer_int_literal_type_and_value(cx, *value, *base);
            TypedExpr::literal(value, span, ty)
        }
        Literal::Char(ch) => TypedExpr::literal(ConstValue::Int(*ch as i64), span, int_type(cx)),
        Literal::Float(value) => {
            let ty = double_type(cx);
            TypedExpr::literal(ConstValue::FloatBits(value.to_bits()), span, ty)
        }
        Literal::String(value) => {
            let char_ty = cx.types.intern(Type {
                kind: TypeKind::Char,
                quals: Qualifiers {
                    is_const: true,
                    is_volatile: false,
                    is_restrict: false,
                },
            });
            let arr_ty = cx.types.intern(Type {
                kind: TypeKind::Array {
                    elem: char_ty,
                    len: ArrayLen::Known(value.len() as u64 + 1),
                },
                quals: Qualifiers::default(),
            });
            TypedExpr {
                kind: TypedExprKind::Opaque,
                ty: arr_ty,
                value_category: ValueCategory::ArrayDesignator,
                const_value: None,
                span,
            }
        }
    }
}

/// Infers integer-literal type/value pairs under the simplified LP64 model used
/// by this semantic pass.
fn infer_int_literal_type_and_value(
    cx: &mut SemaContext<'_>,
    value: u64,
    suffix: IntLiteralSuffix,
) -> (TypeId, ConstValue) {
    match suffix {
        IntLiteralSuffix::Int => {
            if value <= i32::MAX as u64 {
                (int_type(cx), ConstValue::Int(value as i64))
            } else if value <= i64::MAX as u64 {
                (long_type(cx, true), ConstValue::Int(value as i64))
            } else {
                (long_long_type(cx, false), ConstValue::UInt(value))
            }
        }
        IntLiteralSuffix::UInt => {
            if value <= u32::MAX as u64 {
                (int_type_unsigned(cx), ConstValue::UInt(value))
            } else {
                (long_type(cx, false), ConstValue::UInt(value))
            }
        }
        IntLiteralSuffix::Long => {
            if value <= i64::MAX as u64 {
                (long_type(cx, true), ConstValue::Int(value as i64))
            } else {
                (long_type(cx, false), ConstValue::UInt(value))
            }
        }
        IntLiteralSuffix::ULong => (long_type(cx, false), ConstValue::UInt(value)),
        IntLiteralSuffix::LongLong => {
            if value <= i64::MAX as u64 {
                (long_long_type(cx, true), ConstValue::Int(value as i64))
            } else {
                (long_long_type(cx, false), ConstValue::UInt(value))
            }
        }
        IntLiteralSuffix::ULongLong => (long_long_type(cx, false), ConstValue::UInt(value)),
    }
}

/// Applies context-dependent standard conversions:
/// array-to-pointer, function-to-pointer, and lvalue-to-rvalue.
fn apply_standard_conversions(
    cx: &mut SemaContext<'_>,
    mut expr: TypedExpr,
    options: ConversionOptions,
) -> TypedExpr {
    if options.decay_arrays && matches!(expr.value_category, ValueCategory::ArrayDesignator) {
        if let TypeKind::Array { elem, .. } = cx.types.get(expr.ty).kind {
            let ptr_ty = cx.types.intern(Type {
                kind: TypeKind::Pointer { pointee: elem },
                quals: Qualifiers::default(),
            });
            let span = expr.span;
            expr = TypedExpr::implicit_cast(expr, ptr_ty, span);
        }
    }

    if options.decay_functions && matches!(expr.value_category, ValueCategory::FunctionDesignator) {
        let ptr_ty = cx.types.intern(Type {
            kind: TypeKind::Pointer { pointee: expr.ty },
            quals: Qualifiers::default(),
        });
        let span = expr.span;
        expr = TypedExpr::implicit_cast(expr, ptr_ty, span);
    }

    if options.lvalue_to_rvalue && matches!(expr.value_category, ValueCategory::LValue) {
        let to = unqualified(expr.ty, &mut cx.types);
        let span = expr.span;
        expr = TypedExpr::implicit_cast(expr, to, span);
    }

    expr
}

/// Inserts an implicit cast only when source and destination types differ.
fn cast_if_needed(expr: TypedExpr, to: TypeId) -> TypedExpr {
    if expr.ty == to {
        expr
    } else {
        let span = expr.span;
        TypedExpr::implicit_cast(expr, to, span)
    }
}

/// Maps parser binary operators to typed-AST binary operators.
fn map_binary_op(op: AstBinaryOp) -> BinaryOp {
    match op {
        AstBinaryOp::Mul => BinaryOp::Mul,
        AstBinaryOp::Div => BinaryOp::Div,
        AstBinaryOp::Mod => BinaryOp::Mod,
        AstBinaryOp::Add => BinaryOp::Add,
        AstBinaryOp::Sub => BinaryOp::Sub,
        AstBinaryOp::Shl => BinaryOp::Shl,
        AstBinaryOp::Shr => BinaryOp::Shr,
        AstBinaryOp::Lt => BinaryOp::Lt,
        AstBinaryOp::Le => BinaryOp::Le,
        AstBinaryOp::Gt => BinaryOp::Gt,
        AstBinaryOp::Ge => BinaryOp::Ge,
        AstBinaryOp::Eq => BinaryOp::Eq,
        AstBinaryOp::Ne => BinaryOp::Ne,
        AstBinaryOp::BitAnd => BinaryOp::BitwiseAnd,
        AstBinaryOp::BitXor => BinaryOp::BitwiseXor,
        AstBinaryOp::BitOr => BinaryOp::BitwiseOr,
        AstBinaryOp::LogicalAnd => BinaryOp::LogicalAnd,
        AstBinaryOp::LogicalOr => BinaryOp::LogicalOr,
    }
}

/// Maps parser assignment operators to typed-AST assignment operators.
fn map_assign_op(op: AstAssignOp) -> AssignOp {
    match op {
        AstAssignOp::Assign => AssignOp::Assign,
        AstAssignOp::AddAssign => AssignOp::AddAssign,
        AstAssignOp::SubAssign => AssignOp::SubAssign,
        AstAssignOp::MulAssign => AssignOp::MulAssign,
        AstAssignOp::DivAssign => AssignOp::DivAssign,
        AstAssignOp::ModAssign => AssignOp::ModAssign,
        AstAssignOp::ShlAssign => AssignOp::ShlAssign,
        AstAssignOp::ShrAssign => AssignOp::ShrAssign,
        AstAssignOp::BitAndAssign => AssignOp::AndAssign,
        AstAssignOp::BitXorAssign => AssignOp::XorAssign,
        AstAssignOp::BitOrAssign => AssignOp::OrAssign,
    }
}

/// Checks whether an explicit cast is allowed in this implementation level.
fn is_valid_cast(cx: &SemaContext<'_>, from: TypeId, to: TypeId) -> bool {
    let from_kind = &cx.types.get(from).kind;
    let to_kind = &cx.types.get(to).kind;

    if matches!(from_kind, TypeKind::Error) || matches!(to_kind, TypeKind::Error) {
        return true;
    }

    if matches!(to_kind, TypeKind::Void) {
        return true;
    }

    if is_arithmetic(from_kind) && is_arithmetic(to_kind) {
        return true;
    }

    if matches!(from_kind, TypeKind::Pointer { .. }) && matches!(to_kind, TypeKind::Pointer { .. })
    {
        // Strict C99 mode: function-pointer and `void*` interconversion is rejected.
        if (is_void_pointer(from, &cx.types) && is_function_pointer_type(cx, to))
            || (is_void_pointer(to, &cx.types) && is_function_pointer_type(cx, from))
        {
            return false;
        }
        return true;
    }

    if matches!(from_kind, TypeKind::Pointer { .. }) && is_integer(to_kind) {
        return true;
    }

    if is_integer(from_kind) && matches!(to_kind, TypeKind::Pointer { .. }) {
        return true;
    }

    false
}

/// Emits a `TypeMismatch` diagnostic and returns an error-typed placeholder.
fn emit_type_mismatch(
    cx: &mut SemaContext<'_>,
    span: SourceSpan,
    message: impl Into<String>,
) -> TypedExpr {
    cx.emit(SemaDiagnostic::new(
        SemaDiagnosticCode::TypeMismatch,
        message,
        span,
    ));
    TypedExpr::opaque(span, cx.error_type())
}

/// Returns whether an expression is a modifiable lvalue.
fn is_modifiable_lvalue(cx: &SemaContext<'_>, expr: &TypedExpr) -> bool {
    if !matches!(expr.value_category, ValueCategory::LValue) {
        return false;
    }
    let ty = cx.types.get(expr.ty);
    if ty.quals.is_const {
        return false;
    }
    !matches!(
        ty.kind,
        TypeKind::Array { .. } | TypeKind::Function(_) | TypeKind::Void
    )
}

/// Derives value category from a type for symbol/member/index designators.
fn value_category_for_designator_type(cx: &SemaContext<'_>, ty: TypeId) -> ValueCategory {
    match cx.types.get(ty).kind {
        TypeKind::Function(_) => ValueCategory::FunctionDesignator,
        TypeKind::Array { .. } => ValueCategory::ArrayDesignator,
        _ => ValueCategory::LValue,
    }
}

/// Returns true when `ty` is a pointer type.
fn is_pointer_type(cx: &SemaContext<'_>, ty: TypeId) -> bool {
    matches!(cx.types.get(ty).kind, TypeKind::Pointer { .. })
}

/// Returns the pointee type if `ty` is a pointer.
fn pointee_of_pointer(cx: &SemaContext<'_>, ty: TypeId) -> Option<TypeId> {
    match cx.types.get(ty).kind {
        TypeKind::Pointer { pointee } => Some(pointee),
        _ => None,
    }
}

fn is_function_pointer_type(cx: &SemaContext<'_>, ty: TypeId) -> bool {
    let Some(pointee) = pointee_of_pointer(cx, ty) else {
        return false;
    };
    matches!(cx.types.get(pointee).kind, TypeKind::Function(_))
}

/// Returns true when `ty` is arithmetic.
fn is_arithmetic_type(cx: &SemaContext<'_>, ty: TypeId) -> bool {
    is_arithmetic(&cx.types.get(ty).kind)
}

fn is_error_type(cx: &SemaContext<'_>, ty: TypeId) -> bool {
    matches!(cx.types.get(ty).kind, TypeKind::Error)
}

/// Returns true when the typed expression is an integer zero constant.
fn is_null_pointer_constant(expr: &TypedExpr) -> bool {
    matches!(
        expr.const_value,
        Some(ConstValue::Int(0)) | Some(ConstValue::UInt(0))
    )
}

/// Extracts an `i64` integer value from const-value payloads when representable.
fn const_int_value(value: Option<ConstValue>) -> Option<i64> {
    match value {
        Some(ConstValue::Int(v)) => Some(v),
        Some(ConstValue::UInt(v)) => i64::try_from(v).ok(),
        _ => None,
    }
}

/// Applies default argument promotions used by variadic and non-prototype calls.
fn default_argument_promotion(cx: &mut SemaContext<'_>, expr: TypedExpr) -> TypedExpr {
    match cx.types.get(expr.ty).kind {
        TypeKind::Float => cast_if_needed(expr, double_type(cx)),
        _ if is_integer(&cx.types.get(expr.ty).kind) => {
            let promoted = integer_promotion(expr.ty, &mut cx.types);
            cast_if_needed(expr, promoted)
        }
        _ => expr,
    }
}

/// Returns canonical `int`.
fn int_type(cx: &mut SemaContext<'_>) -> TypeId {
    cx.types.intern(Type {
        kind: TypeKind::Int { signed: true },
        quals: Qualifiers::default(),
    })
}

/// Returns canonical `unsigned int`.
fn int_type_unsigned(cx: &mut SemaContext<'_>) -> TypeId {
    cx.types.intern(Type {
        kind: TypeKind::Int { signed: false },
        quals: Qualifiers::default(),
    })
}

/// Returns canonical `long` / `unsigned long`.
fn long_type(cx: &mut SemaContext<'_>, signed: bool) -> TypeId {
    cx.types.intern(Type {
        kind: TypeKind::Long { signed },
        quals: Qualifiers::default(),
    })
}

/// Returns canonical `long long` / `unsigned long long`.
fn long_long_type(cx: &mut SemaContext<'_>, signed: bool) -> TypeId {
    cx.types.intern(Type {
        kind: TypeKind::LongLong { signed },
        quals: Qualifiers::default(),
    })
}

/// Returns canonical `double`.
fn double_type(cx: &mut SemaContext<'_>) -> TypeId {
    cx.types.intern(Type {
        kind: TypeKind::Double,
        quals: Qualifiers::default(),
    })
}

/// Returns canonical `size_t` (modeled as `unsigned long` in LP64).
fn size_t_type(cx: &mut SemaContext<'_>) -> TypeId {
    long_type(cx, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::span::SourceSpan;
    use crate::frontend::parser::ast::{
        AssignOp as AstAssignOp, BinaryOp as AstBinaryOp, Expr, IntLiteralSuffix, RecordKind,
    };
    use crate::frontend::sema::symbols::{DefinitionStatus, Linkage, Symbol, SymbolId};
    use crate::frontend::sema::types::{FieldDef, FunctionType, RecordDef, RecordId};

    fn new_cx() -> SemaContext<'static> {
        SemaContext::new("test.c", "")
    }

    fn bind_symbol(cx: &mut SemaContext<'_>, name: &str, ty: TypeId) -> SymbolId {
        let sym = Symbol::new(
            name.to_string(),
            SymbolKind::Object,
            ty,
            Linkage::None,
            DefinitionStatus::Defined,
            SourceSpan::dummy(),
        );
        let id = cx.insert_symbol(sym);
        let _ = cx.insert_ordinary(name.to_string(), id);
        id
    }

    fn int_ty(cx: &mut SemaContext<'_>) -> TypeId {
        cx.types.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        })
    }

    fn unsigned_char_ty(cx: &mut SemaContext<'_>) -> TypeId {
        cx.types.intern(Type {
            kind: TypeKind::UnsignedChar,
            quals: Qualifiers::default(),
        })
    }

    fn record_ty_with_field(
        cx: &mut SemaContext<'_>,
        field_name: &str,
        field_ty: TypeId,
    ) -> TypeId {
        let rec_id: RecordId = cx.records.insert(RecordDef {
            tag: Some("S".to_string()),
            kind: RecordKind::Struct,
            fields: vec![FieldDef {
                name: Some(field_name.to_string()),
                ty: field_ty,
                bit_width: None,
            }],
            is_complete: true,
        });
        cx.types.intern(Type {
            kind: TypeKind::Record(rec_id),
            quals: Qualifiers::default(),
        })
    }

    #[test]
    fn infers_uint_literal_type() {
        let mut cx = new_cx();
        let expr = Expr::int_with_base_and_span(7, IntLiteralSuffix::UInt, SourceSpan::dummy());
        let typed = lower_expr(&mut cx, &expr);
        assert!(matches!(
            cx.types.get(typed.ty).kind,
            TypeKind::Int { signed: false }
        ));
        assert_eq!(typed.const_value, Some(ConstValue::UInt(7)));
    }

    #[test]
    fn propagates_unary_constant_values() {
        let mut cx = new_cx();

        let minus = Expr::unary(
            crate::frontend::parser::ast::UnaryOp::Minus,
            Expr::int_with_span(1, SourceSpan::dummy()),
        );
        let logical_not = Expr::unary(
            crate::frontend::parser::ast::UnaryOp::LogicalNot,
            Expr::int_with_span(0, SourceSpan::dummy()),
        );
        let bit_not = Expr::unary(
            crate::frontend::parser::ast::UnaryOp::BitNot,
            Expr::int_with_span(1, SourceSpan::dummy()),
        );

        assert_eq!(
            lower_expr(&mut cx, &minus).const_value,
            Some(ConstValue::Int(-1))
        );
        assert_eq!(
            lower_expr(&mut cx, &logical_not).const_value,
            Some(ConstValue::Int(1))
        );
        assert_eq!(
            lower_expr(&mut cx, &bit_not).const_value,
            Some(ConstValue::Int(-2))
        );
    }

    #[test]
    fn index_expression_yields_element_lvalue() {
        let mut cx = new_cx();
        let int_ty = int_ty(&mut cx);
        let arr_ty = cx.types.intern(Type {
            kind: TypeKind::Array {
                elem: int_ty,
                len: ArrayLen::Known(4),
            },
            quals: Qualifiers::default(),
        });
        bind_symbol(&mut cx, "arr", arr_ty);

        let expr = Expr::index(
            Expr::var_with_span("arr".to_string(), SourceSpan::dummy()),
            Expr::int_with_span(1, SourceSpan::dummy()),
        );
        let typed = lower_expr(&mut cx, &expr);
        assert_eq!(typed.ty, int_ty);
        assert_eq!(typed.value_category, ValueCategory::LValue);
    }

    #[test]
    fn shift_compound_assignment_promotes_rhs_independently() {
        let mut cx = new_cx();
        let lhs_ty = long_long_type(&mut cx, true);
        let rhs_ty = unsigned_char_ty(&mut cx);
        bind_symbol(&mut cx, "lhs", lhs_ty);
        bind_symbol(&mut cx, "rhs", rhs_ty);

        let expr = Expr::assign(
            Expr::var_with_span("lhs".to_string(), SourceSpan::dummy()),
            AstAssignOp::ShlAssign,
            Expr::var_with_span("rhs".to_string(), SourceSpan::dummy()),
        );
        let typed = lower_expr(&mut cx, &expr);

        let TypedExprKind::Assign { rhs, .. } = typed.kind else {
            panic!("expected assignment expression");
        };
        assert!(matches!(
            cx.types.get(rhs.ty).kind,
            TypeKind::Int { signed: true }
        ));
    }

    #[test]
    fn sizeof_array_operand_does_not_decay() {
        let mut cx = new_cx();
        let int_ty = int_ty(&mut cx);
        let arr_ty = cx.types.intern(Type {
            kind: TypeKind::Array {
                elem: int_ty,
                len: ArrayLen::Known(4),
            },
            quals: Qualifiers::default(),
        });
        bind_symbol(&mut cx, "arr", arr_ty);

        let expr = Expr::sizeof_expr(Expr::var_with_span("arr".to_string(), SourceSpan::dummy()));
        let typed = lower_expr(&mut cx, &expr);
        assert_eq!(typed.const_value, Some(ConstValue::UInt(16)));
    }

    #[test]
    fn member_access_resolves_field_type() {
        let mut cx = new_cx();
        let int_ty = int_ty(&mut cx);
        let rec_ty = record_ty_with_field(&mut cx, "x", int_ty);
        bind_symbol(&mut cx, "s", rec_ty);

        let expr = Expr::member(
            Expr::var_with_span("s".to_string(), SourceSpan::dummy()),
            "x".to_string(),
            false,
        );
        let typed = lower_expr(&mut cx, &expr);
        assert_eq!(typed.ty, int_ty);
        assert_eq!(typed.value_category, ValueCategory::LValue);
    }

    #[test]
    fn function_call_argument_mismatch_reports_error() {
        let mut cx = new_cx();
        let int_ty = int_ty(&mut cx);
        let fn_ty = cx.types.intern(Type {
            kind: TypeKind::Function(FunctionType {
                ret: int_ty,
                params: vec![int_ty],
                variadic: false,
                style: FunctionStyle::Prototype,
            }),
            quals: Qualifiers::default(),
        });
        let sym = Symbol::new(
            "f".to_string(),
            SymbolKind::Function,
            fn_ty,
            Linkage::External,
            DefinitionStatus::Declared,
            SourceSpan::dummy(),
        );
        let sym_id = cx.insert_symbol(sym);
        let _ = cx.insert_ordinary("f".to_string(), sym_id);

        let expr = Expr::call(
            Expr::var_with_span("f".to_string(), SourceSpan::dummy()),
            Vec::new(),
        );
        let _ = lower_expr(&mut cx, &expr);
        let diags = cx.take_diagnostics();
        assert!(
            diags
                .iter()
                .any(|d| d.code == SemaDiagnosticCode::TypeMismatch),
            "expected TypeMismatch diagnostics, got: {diags:?}"
        );
    }

    #[test]
    fn variadic_call_requires_fixed_arguments() {
        let mut cx = new_cx();
        let int_ty = int_ty(&mut cx);
        let fn_ty = cx.types.intern(Type {
            kind: TypeKind::Function(FunctionType {
                ret: int_ty,
                params: vec![int_ty],
                variadic: true,
                style: FunctionStyle::Prototype,
            }),
            quals: Qualifiers::default(),
        });
        let sym = Symbol::new(
            "printf_like".to_string(),
            SymbolKind::Function,
            fn_ty,
            Linkage::External,
            DefinitionStatus::Declared,
            SourceSpan::dummy(),
        );
        let sym_id = cx.insert_symbol(sym);
        let _ = cx.insert_ordinary("printf_like".to_string(), sym_id);

        let expr = Expr::call(
            Expr::var_with_span("printf_like".to_string(), SourceSpan::dummy()),
            Vec::new(),
        );
        let _ = lower_expr(&mut cx, &expr);
        let diags = cx.take_diagnostics();
        assert!(
            diags
                .iter()
                .any(|d| d.code == SemaDiagnosticCode::TypeMismatch),
            "expected TypeMismatch diagnostics, got: {diags:?}"
        );
    }

    #[test]
    fn invalid_pointer_addition_reports_error() {
        let mut cx = new_cx();
        let int_ty = int_ty(&mut cx);
        let ptr_ty = cx.types.intern(Type {
            kind: TypeKind::Pointer { pointee: int_ty },
            quals: Qualifiers::default(),
        });
        bind_symbol(&mut cx, "p", ptr_ty);
        bind_symbol(&mut cx, "q", ptr_ty);

        let expr = Expr::binary(
            Expr::var_with_span("p".to_string(), SourceSpan::dummy()),
            AstBinaryOp::Add,
            Expr::var_with_span("q".to_string(), SourceSpan::dummy()),
        );
        let _ = lower_expr(&mut cx, &expr);

        let diags = cx.take_diagnostics();
        assert!(
            diags
                .iter()
                .any(|d| d.code == SemaDiagnosticCode::TypeMismatch),
            "expected TypeMismatch diagnostics, got: {diags:?}"
        );
    }

    #[test]
    fn pointer_assignment_accepts_null_pointer_constant() {
        let mut cx = new_cx();
        let int_ty = int_ty(&mut cx);
        let ptr_ty = cx.types.intern(Type {
            kind: TypeKind::Pointer { pointee: int_ty },
            quals: Qualifiers::default(),
        });
        bind_symbol(&mut cx, "p", ptr_ty);

        let expr = Expr::assign(
            Expr::var_with_span("p".to_string(), SourceSpan::dummy()),
            AstAssignOp::Assign,
            Expr::int_with_span(0, SourceSpan::dummy()),
        );
        let typed = lower_expr(&mut cx, &expr);
        assert_eq!(typed.ty, ptr_ty);

        let diags = cx.take_diagnostics();
        assert!(
            !diags
                .iter()
                .any(|d| d.code == SemaDiagnosticCode::TypeMismatch),
            "did not expect TypeMismatch diagnostics, got: {diags:?}"
        );
    }

    #[test]
    fn prototype_pointer_param_accepts_null_pointer_constant() {
        let mut cx = new_cx();
        let int_ty = int_ty(&mut cx);
        let ptr_ty = cx.types.intern(Type {
            kind: TypeKind::Pointer { pointee: int_ty },
            quals: Qualifiers::default(),
        });
        let fn_ty = cx.types.intern(Type {
            kind: TypeKind::Function(FunctionType {
                ret: int_ty,
                params: vec![ptr_ty],
                variadic: false,
                style: FunctionStyle::Prototype,
            }),
            quals: Qualifiers::default(),
        });
        let sym = Symbol::new(
            "f".to_string(),
            SymbolKind::Function,
            fn_ty,
            Linkage::External,
            DefinitionStatus::Declared,
            SourceSpan::dummy(),
        );
        let sym_id = cx.insert_symbol(sym);
        let _ = cx.insert_ordinary("f".to_string(), sym_id);

        let expr = Expr::call(
            Expr::var_with_span("f".to_string(), SourceSpan::dummy()),
            vec![Expr::int_with_span(0, SourceSpan::dummy())],
        );
        let _ = lower_expr(&mut cx, &expr);

        let diags = cx.take_diagnostics();
        assert!(
            !diags
                .iter()
                .any(|d| d.code == SemaDiagnosticCode::TypeMismatch),
            "did not expect TypeMismatch diagnostics, got: {diags:?}"
        );
    }

    #[test]
    fn nonprototype_call_applies_default_argument_promotions() {
        let mut cx = new_cx();
        let int_ty = int_ty(&mut cx);
        let uchar_ty = unsigned_char_ty(&mut cx);
        bind_symbol(&mut cx, "c", uchar_ty);

        let fn_ty = cx.types.intern(Type {
            kind: TypeKind::Function(FunctionType {
                ret: int_ty,
                params: Vec::new(),
                variadic: false,
                style: FunctionStyle::NonPrototype,
            }),
            quals: Qualifiers::default(),
        });
        let sym = Symbol::new(
            "f".to_string(),
            SymbolKind::Function,
            fn_ty,
            Linkage::External,
            DefinitionStatus::Declared,
            SourceSpan::dummy(),
        );
        let sym_id = cx.insert_symbol(sym);
        let _ = cx.insert_ordinary("f".to_string(), sym_id);

        let expr = Expr::call(
            Expr::var_with_span("f".to_string(), SourceSpan::dummy()),
            vec![Expr::var_with_span("c".to_string(), SourceSpan::dummy())],
        );
        let typed = lower_expr(&mut cx, &expr);

        match typed.kind {
            TypedExprKind::Call { args, .. } => {
                assert_eq!(args.len(), 1);
                assert!(matches!(
                    cx.types.get(args[0].ty).kind,
                    TypeKind::Int { .. }
                ));
            }
            other => panic!("expected call expression, got {other:?}"),
        }
    }
}
