use crate::common::span::SourceSpan;
use crate::frontend::sema::diagnostic::{SemaDiagnostic, SemaDiagnosticCode};
use crate::frontend::sema::typed_ast::{BinaryOp, ConstValue, TypedExpr, TypedExprKind, UnaryOp};
use crate::frontend::sema::types::{RecordArena, TypeArena, TypeId, TypeKind, type_size_of};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstExprContext {
    IntegerConstant,
    ArithmeticConstant,
    AddressConstant,
}

pub struct ConstEvalEnv<'a> {
    pub types: &'a TypeArena,
    pub records: &'a RecordArena,
}

/// Evaluate constant expressions used by ICE/address-constant contexts.
pub fn eval_const_expr(
    expr: &TypedExpr,
    context: ConstExprContext,
    env: &ConstEvalEnv<'_>,
) -> Result<ConstValue, SemaDiagnostic> {
    let value = eval_const_expr_inner(expr, env)?;
    match context {
        ConstExprContext::IntegerConstant => match value {
            ConstValue::Int(_) | ConstValue::UInt(_) => Ok(value),
            _ => Err(non_constant_diag(
                expr.span,
                "expression is not an integer constant expression",
            )),
        },
        ConstExprContext::ArithmeticConstant => match value {
            ConstValue::Int(_) | ConstValue::UInt(_) | ConstValue::FloatBits(_) => Ok(value),
            _ => Err(non_constant_diag(
                expr.span,
                "expression is not an arithmetic constant expression",
            )),
        },
        ConstExprContext::AddressConstant => match value {
            ConstValue::NullPtr | ConstValue::Addr { .. } => Ok(value),
            _ => Err(non_constant_diag(
                expr.span,
                "expression is not an address constant",
            )),
        },
    }
}

fn eval_const_expr_inner(
    expr: &TypedExpr,
    env: &ConstEvalEnv<'_>,
) -> Result<ConstValue, SemaDiagnostic> {
    match &expr.kind {
        TypedExprKind::Literal(value) => Ok(*value),
        TypedExprKind::Opaque | TypedExprKind::SymbolRef(_) => expr
            .const_value
            .ok_or_else(|| non_constant_diag(expr.span, "expression is not a constant")),
        TypedExprKind::Unary { op, operand } => eval_unary_expr(*op, operand, expr.span, env),
        TypedExprKind::Binary { op, left, right } => {
            eval_binary_expr(*op, left, right, expr.span, env)
        }
        TypedExprKind::Conditional {
            cond,
            then_expr,
            else_expr,
        } => {
            let cond_value = const_to_i64(eval_const_expr_inner(cond, env)?, cond.span)?;
            if cond_value != 0 {
                eval_const_expr_inner(then_expr, env)
            } else {
                eval_const_expr_inner(else_expr, env)
            }
        }
        TypedExprKind::Cast { expr: inner, to }
        | TypedExprKind::ImplicitCast { expr: inner, to } => {
            let value = eval_const_expr_inner(inner, env)?;
            cast_const_value(value, *to, expr.span, env)
        }
        TypedExprKind::SizeofType { ty } => {
            let size = type_size_of(*ty, env.types, env.records).ok_or_else(|| {
                non_constant_diag(expr.span, "operand of sizeof has incomplete type")
            })?;
            Ok(ConstValue::UInt(size))
        }
        _ => Err(non_constant_diag(
            expr.span,
            "expression is not a constant expression",
        )),
    }
}

fn eval_unary_expr(
    op: UnaryOp,
    operand: &TypedExpr,
    span: SourceSpan,
    env: &ConstEvalEnv<'_>,
) -> Result<ConstValue, SemaDiagnostic> {
    let operand_value = eval_const_expr_inner(operand, env)?;
    match op {
        UnaryOp::Plus => match operand_value {
            ConstValue::Int(_) | ConstValue::UInt(_) => Ok(operand_value),
            _ => {
                let integer = const_to_i64(operand_value, span)?;
                Ok(ConstValue::Int(integer))
            }
        },
        UnaryOp::Minus => {
            let integer = const_to_i64(operand_value, span)?;
            integer
                .checked_neg()
                .map(ConstValue::Int)
                .ok_or_else(|| signed_overflow_diag(span))
        }
        UnaryOp::LogicalNot => {
            let integer = const_to_i64(operand_value, span)?;
            Ok(ConstValue::Int(i64::from(integer == 0)))
        }
        UnaryOp::BitwiseNot => {
            let integer = const_to_i64(operand_value, span)?;
            Ok(ConstValue::Int(!integer))
        }
        _ => Err(non_constant_diag(
            span,
            "unsupported unary operator in constant expression",
        )),
    }
}

fn eval_binary_expr(
    op: BinaryOp,
    left: &TypedExpr,
    right: &TypedExpr,
    span: SourceSpan,
    env: &ConstEvalEnv<'_>,
) -> Result<ConstValue, SemaDiagnostic> {
    if matches!(op, BinaryOp::LogicalAnd) {
        let lhs = const_to_i64(eval_const_expr_inner(left, env)?, left.span)?;
        if lhs == 0 {
            return Ok(ConstValue::Int(0));
        }
        let rhs = const_to_i64(eval_const_expr_inner(right, env)?, right.span)?;
        return Ok(ConstValue::Int(i64::from(rhs != 0)));
    }
    if matches!(op, BinaryOp::LogicalOr) {
        let lhs = const_to_i64(eval_const_expr_inner(left, env)?, left.span)?;
        if lhs != 0 {
            return Ok(ConstValue::Int(1));
        }
        let rhs = const_to_i64(eval_const_expr_inner(right, env)?, right.span)?;
        return Ok(ConstValue::Int(i64::from(rhs != 0)));
    }

    let lhs = const_to_i64(eval_const_expr_inner(left, env)?, left.span)?;
    let rhs = const_to_i64(eval_const_expr_inner(right, env)?, right.span)?;

    let value = match op {
        BinaryOp::Add => lhs
            .checked_add(rhs)
            .ok_or_else(|| signed_overflow_diag(span))?,
        BinaryOp::Sub => lhs
            .checked_sub(rhs)
            .ok_or_else(|| signed_overflow_diag(span))?,
        BinaryOp::Mul => lhs
            .checked_mul(rhs)
            .ok_or_else(|| signed_overflow_diag(span))?,
        BinaryOp::Div => {
            if rhs == 0 {
                return Err(division_by_zero_diag(right.span));
            }
            lhs.checked_div(rhs)
                .ok_or_else(|| signed_overflow_diag(span))?
        }
        BinaryOp::Mod => {
            if rhs == 0 {
                return Err(division_by_zero_diag(right.span));
            }
            lhs.checked_rem(rhs)
                .ok_or_else(|| signed_overflow_diag(span))?
        }
        BinaryOp::Shl => {
            if !(0..64).contains(&rhs) {
                return Err(signed_overflow_diag(span));
            }
            let shifted = (lhs as i128) << (rhs as u32);
            if shifted < i64::MIN as i128 || shifted > i64::MAX as i128 {
                return Err(signed_overflow_diag(span));
            }
            shifted as i64
        }
        BinaryOp::Shr => {
            if !(0..64).contains(&rhs) {
                return Err(signed_overflow_diag(span));
            }
            lhs >> (rhs as u32)
        }
        BinaryOp::BitwiseAnd => lhs & rhs,
        BinaryOp::BitwiseOr => lhs | rhs,
        BinaryOp::BitwiseXor => lhs ^ rhs,
        BinaryOp::Eq => i64::from(lhs == rhs),
        BinaryOp::Ne => i64::from(lhs != rhs),
        BinaryOp::Lt => i64::from(lhs < rhs),
        BinaryOp::Le => i64::from(lhs <= rhs),
        BinaryOp::Gt => i64::from(lhs > rhs),
        BinaryOp::Ge => i64::from(lhs >= rhs),
        BinaryOp::LogicalAnd | BinaryOp::LogicalOr => unreachable!(),
    };

    Ok(ConstValue::Int(value))
}

fn cast_const_value(
    value: ConstValue,
    to: TypeId,
    span: SourceSpan,
    env: &ConstEvalEnv<'_>,
) -> Result<ConstValue, SemaDiagnostic> {
    match &env.types.get(to).kind {
        TypeKind::Bool => {
            let bits = integer_bits(value, span)?;
            Ok(ConstValue::Int(i64::from(bits != 0)))
        }
        TypeKind::Char | TypeKind::SignedChar => {
            let bits = integer_bits(value, span)?;
            Ok(ConstValue::Int((bits as u8 as i8) as i64))
        }
        TypeKind::UnsignedChar => {
            let bits = integer_bits(value, span)?;
            Ok(ConstValue::UInt(bits as u8 as u64))
        }
        TypeKind::Short { signed: true } => {
            let bits = integer_bits(value, span)?;
            Ok(ConstValue::Int((bits as u16 as i16) as i64))
        }
        TypeKind::Short { signed: false } => {
            let bits = integer_bits(value, span)?;
            Ok(ConstValue::UInt(bits as u16 as u64))
        }
        TypeKind::Int { signed: true } | TypeKind::Enum(_) => {
            let bits = integer_bits(value, span)?;
            Ok(ConstValue::Int((bits as u32 as i32) as i64))
        }
        TypeKind::Int { signed: false } => {
            let bits = integer_bits(value, span)?;
            Ok(ConstValue::UInt(bits as u32 as u64))
        }
        TypeKind::Long { signed: true } | TypeKind::LongLong { signed: true } => {
            let bits = integer_bits(value, span)?;
            Ok(ConstValue::Int(bits as i64))
        }
        TypeKind::Long { signed: false } | TypeKind::LongLong { signed: false } => {
            let bits = integer_bits(value, span)?;
            Ok(ConstValue::UInt(bits))
        }
        TypeKind::Pointer { .. } => match value {
            ConstValue::NullPtr | ConstValue::Addr { .. } => Ok(value),
            ConstValue::Int(0) | ConstValue::UInt(0) => Ok(ConstValue::NullPtr),
            _ => Err(non_constant_diag(
                span,
                "cast result is not a representable pointer constant",
            )),
        },
        _ => Err(non_constant_diag(
            span,
            "cast target is not supported in constant expressions",
        )),
    }
}

fn integer_bits(value: ConstValue, span: SourceSpan) -> Result<u64, SemaDiagnostic> {
    match value {
        ConstValue::Int(v) => Ok(v as u64),
        ConstValue::UInt(v) => Ok(v),
        ConstValue::NullPtr => Ok(0),
        _ => Err(non_constant_diag(
            span,
            "operand is not an integer constant expression",
        )),
    }
}

fn const_to_i64(value: ConstValue, span: SourceSpan) -> Result<i64, SemaDiagnostic> {
    match value {
        ConstValue::Int(v) => Ok(v),
        ConstValue::UInt(v) => i64::try_from(v).map_err(|_| signed_overflow_diag(span)),
        ConstValue::NullPtr => Ok(0),
        _ => Err(non_constant_diag(
            span,
            "operand is not an integer constant expression",
        )),
    }
}

fn non_constant_diag(span: SourceSpan, message: &str) -> SemaDiagnostic {
    SemaDiagnostic::new(
        SemaDiagnosticCode::NonConstantInRequiredContext,
        message,
        span,
    )
}

fn division_by_zero_diag(span: SourceSpan) -> SemaDiagnostic {
    SemaDiagnostic::new(
        SemaDiagnosticCode::ConstantDivisionByZero,
        "division by zero in constant expression",
        span,
    )
}

fn signed_overflow_diag(span: SourceSpan) -> SemaDiagnostic {
    SemaDiagnostic::new(
        SemaDiagnosticCode::ConstantSignedOverflow,
        "signed integer overflow in constant expression",
        span,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::parser::ast::RecordKind;
    use crate::frontend::sema::typed_ast::ValueCategory;
    use crate::frontend::sema::types::{FieldDef, Qualifiers, RecordDef, Type, TypeKind};

    fn typed_expr(kind: TypedExprKind, ty: TypeId) -> TypedExpr {
        TypedExpr {
            kind,
            ty,
            value_category: ValueCategory::RValue,
            const_value: None,
            span: SourceSpan::dummy(),
        }
    }

    fn int_literal(value: i64, ty: TypeId) -> TypedExpr {
        TypedExpr::literal(ConstValue::Int(value), SourceSpan::dummy(), ty)
    }

    #[test]
    fn evaluates_nested_integer_arithmetic() {
        let mut types = TypeArena::new();
        let records = RecordArena::default();
        let int_ty = types.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let env = ConstEvalEnv {
            types: &types,
            records: &records,
        };

        let expr = typed_expr(
            TypedExprKind::Binary {
                op: BinaryOp::Mul,
                left: Box::new(typed_expr(
                    TypedExprKind::Binary {
                        op: BinaryOp::Add,
                        left: Box::new(int_literal(2, int_ty)),
                        right: Box::new(int_literal(3, int_ty)),
                    },
                    int_ty,
                )),
                right: Box::new(int_literal(4, int_ty)),
            },
            int_ty,
        );

        let value = eval_const_expr(&expr, ConstExprContext::IntegerConstant, &env)
            .expect("constant expression should evaluate");
        assert_eq!(value, ConstValue::Int(20));
    }

    #[test]
    fn preserves_short_circuit_behavior() {
        let mut types = TypeArena::new();
        let records = RecordArena::default();
        let int_ty = types.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let env = ConstEvalEnv {
            types: &types,
            records: &records,
        };

        let expr = typed_expr(
            TypedExprKind::Binary {
                op: BinaryOp::LogicalAnd,
                left: Box::new(int_literal(0, int_ty)),
                right: Box::new(typed_expr(
                    TypedExprKind::Binary {
                        op: BinaryOp::Div,
                        left: Box::new(int_literal(1, int_ty)),
                        right: Box::new(int_literal(0, int_ty)),
                    },
                    int_ty,
                )),
            },
            int_ty,
        );

        let value = eval_const_expr(&expr, ConstExprContext::IntegerConstant, &env)
            .expect("short-circuit should skip division");
        assert_eq!(value, ConstValue::Int(0));
    }

    #[test]
    fn reports_division_by_zero() {
        let mut types = TypeArena::new();
        let records = RecordArena::default();
        let int_ty = types.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let env = ConstEvalEnv {
            types: &types,
            records: &records,
        };

        let expr = typed_expr(
            TypedExprKind::Binary {
                op: BinaryOp::Div,
                left: Box::new(int_literal(1, int_ty)),
                right: Box::new(int_literal(0, int_ty)),
            },
            int_ty,
        );

        let err = eval_const_expr(&expr, ConstExprContext::IntegerConstant, &env)
            .expect_err("division by zero must fail");
        assert_eq!(err.code, SemaDiagnosticCode::ConstantDivisionByZero);
    }

    #[test]
    fn casts_to_unsigned_char_with_truncation() {
        let mut types = TypeArena::new();
        let records = RecordArena::default();
        let int_ty = types.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let uchar_ty = types.intern(Type {
            kind: TypeKind::UnsignedChar,
            quals: Qualifiers::default(),
        });
        let env = ConstEvalEnv {
            types: &types,
            records: &records,
        };

        let expr = typed_expr(
            TypedExprKind::Cast {
                expr: Box::new(int_literal(0x1ff, int_ty)),
                to: uchar_ty,
            },
            uchar_ty,
        );

        let value = eval_const_expr(&expr, ConstExprContext::IntegerConstant, &env)
            .expect("cast should evaluate");
        assert_eq!(value, ConstValue::UInt(0xff));
    }

    #[test]
    fn unary_plus_preserves_unsigned_constants() {
        let mut types = TypeArena::new();
        let records = RecordArena::default();
        let uint_ty = types.intern(Type {
            kind: TypeKind::Int { signed: false },
            quals: Qualifiers::default(),
        });
        let env = ConstEvalEnv {
            types: &types,
            records: &records,
        };

        let expr = typed_expr(
            TypedExprKind::Unary {
                op: UnaryOp::Plus,
                operand: Box::new(TypedExpr::literal(
                    ConstValue::UInt(7),
                    SourceSpan::dummy(),
                    uint_ty,
                )),
            },
            uint_ty,
        );

        let value = eval_const_expr(&expr, ConstExprContext::IntegerConstant, &env)
            .expect("unary plus should evaluate");
        assert_eq!(value, ConstValue::UInt(7));
    }

    #[test]
    fn computes_sizeof_complete_struct() {
        let mut types = TypeArena::new();
        let mut records = RecordArena::default();
        let int_ty = types.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let char_ty = types.intern(Type {
            kind: TypeKind::Char,
            quals: Qualifiers::default(),
        });
        let size_ty = types.intern(Type {
            kind: TypeKind::Long { signed: false },
            quals: Qualifiers::default(),
        });
        let record_id = records.insert(RecordDef {
            tag: Some("S".to_string()),
            kind: RecordKind::Struct,
            fields: vec![
                FieldDef {
                    name: Some("a".to_string()),
                    ty: int_ty,
                    bit_width: None,
                },
                FieldDef {
                    name: Some("b".to_string()),
                    ty: char_ty,
                    bit_width: None,
                },
            ],
            is_complete: true,
        });
        let record_ty = types.intern(Type {
            kind: TypeKind::Record(record_id),
            quals: Qualifiers::default(),
        });
        let env = ConstEvalEnv {
            types: &types,
            records: &records,
        };

        let expr = typed_expr(TypedExprKind::SizeofType { ty: record_ty }, size_ty);
        let value = eval_const_expr(&expr, ConstExprContext::IntegerConstant, &env)
            .expect("sizeof(struct) should evaluate");
        assert_eq!(value, ConstValue::UInt(5));
    }
}
