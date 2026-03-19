use chumsky::prelude::todo;

use crate::common::span::SourceSpan;
use crate::frontend::parser::ast::{
    Designator, DesignatorKind, ExprKind, Initializer, InitializerItem, InitializerKind, Literal,
};
use crate::frontend::sema::check::decl;
use crate::frontend::sema::check::expr;
use crate::frontend::sema::const_eval::{self, ConstEvalEnv, ConstExprContext};
use crate::frontend::sema::context::SemaContext;
use crate::frontend::sema::diagnostic::{SemaDiagnostic, SemaDiagnosticCode};
use crate::frontend::sema::typed_ast::{
    ConstValue, TypedExpr, TypedExprKind, TypedInitItem, TypedInitializer, ValueCategory,
};
use crate::frontend::sema::types::{
    ArrayLen, FieldId, Type, TypeId, TypeKind, assignment_compatible_with_const,
};

/// An element in the initialization path.
///
/// This represents one step in the path to a subobject being initialized.
/// For example, in `struct S s = { .a[5].b = 42 }`, the path is:
/// - `StructField(a)`
/// - `ArrayIndex(5)`
/// - `StructField(b)`
pub enum InitPathElem {
    /// Index into an array element.
    ArrayIndex(usize),
    /// Access to a struct field.
    StructField(FieldId),
    /// Access to a union field.
    UnionField(FieldId),
}

/// Cursor for tracking the current position during initialization.
///
/// This structure maintains the state while walking through an initializer,
/// tracking which subobject is currently being initialized.
pub struct InitCursor {
    /// The type of the object being initialized.
    pub object_ty: TypeId,
    /// The path from the root object to the current subobject.
    pub path: Vec<InitPathElem>,
    /// The type of the current subobject.
    pub current_subobject_ty: TypeId,
    /// The next implicit array index (for brace elision).
    pub next_implicit_index: usize,
}

/// Result of lowering one initializer against a target type.
pub struct LoweredInitializer {
    pub init: TypedInitializer,
    pub resulting_ty: TypeId,
}

fn zero_span() -> SourceSpan {
    SourceSpan::new(0, 0)
}

/// Entry point for initializer semantic checking.
///
/// This function type-checks an initializer and produces a typed initializer.
/// In the framework stage, only simple expression initializers are supported.
///
/// # TODO
/// - Implement aggregate initialization with designators
/// - Implement brace elision and implicit zero-initialization
/// - Validate initializer compatibility with target type
pub fn lower_initializer(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    init: &Initializer,
) -> LoweredInitializer {
    match &cx.types.get(target_ty).kind {
        TypeKind::Array { .. } | TypeKind::Record(_) => {
            lower_aggregate_or_string_initializer(cx, target_ty, init)
        }
        _ => LoweredInitializer {
            init: lower_scalar_initializer(cx, target_ty, init),
            resulting_ty: target_ty,
        },
    }
}

/// Checks whether an initializer tree is a constant initializer.
pub fn is_constant_initializer(cx: &SemaContext<'_>, init: &TypedInitializer) -> bool {
    match init {
        TypedInitializer::Expr(expr) => {
            // C99 6.6: the comma operator is never a constant expression,
            // even if both operands are constant.
            if matches!(expr.kind, TypedExprKind::Comma { .. }) {
                return false;
            }
            if expr::is_address_constant_expr(cx, expr) {
                return true;
            }
            if matches!(
                expr.const_value,
                Some(ConstValue::Int(_))
                    | Some(ConstValue::UInt(_))
                    | Some(ConstValue::FloatBits(_))
                    | Some(ConstValue::NullPtr)
                    | Some(ConstValue::Addr { .. })
            ) {
                return true;
            }
            let env = ConstEvalEnv {
                types: &cx.types,
                records: &cx.records,
            };
            const_eval::eval_const_expr(expr, ConstExprContext::ArithmeticConstant, &env).is_ok()
                || const_eval::eval_const_expr(expr, ConstExprContext::AddressConstant, &env)
                    .is_ok()
        }
        TypedInitializer::Aggregate(items) => items
            .iter()
            .all(|item| is_constant_initializer(cx, &item.init)),
        TypedInitializer::SparseArray { entries, .. } => entries
            .values()
            .all(|item| is_constant_initializer(cx, &item.init)),
        TypedInitializer::ZeroInit { .. } => true,
    }
}

fn lower_aggregate_or_string_initializer(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    init: &Initializer,
) -> LoweredInitializer {
    if let Some(string_lowered) = try_lower_char_array_string_initializer(cx, target_ty, init) {
        return string_lowered;
    }

    match &init.kind {
        InitializerKind::Aggregate(items) => {
            let lowered = lower_aggregate_from_items(cx, target_ty, items, false, init.span);
            LoweredInitializer {
                init: lowered.init,
                resulting_ty: lowered.resulting_ty,
            }
        }
        InitializerKind::Expr(_) => {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::InvalidInitializer,
                "aggregate object requires brace-enclosed initializer",
                init.span,
            ));
            LoweredInitializer {
                init: TypedInitializer::ZeroInit { ty: target_ty },
                resulting_ty: target_ty,
            }
        }
    }
}

fn lower_scalar_initializer(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    init: &Initializer,
) -> TypedInitializer {
    match &init.kind {
        InitializerKind::Expr(expr_node) => lower_scalar_expr_initializer(cx, target_ty, expr_node),
        InitializerKind::Aggregate(items) => {
            let Some(first) = items.first() else {
                return TypedInitializer::ZeroInit { ty: target_ty };
            };

            if !first.designators.is_empty() {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::InvalidInitializer,
                    "designator cannot target a scalar object",
                    first.span,
                ));
            }
            if items.len() > 1 {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::InvalidInitializer,
                    "too many initializer elements for scalar object",
                    init.span,
                ));
            }

            lower_scalar_initializer(cx, target_ty, &first.init)
        }
    }
}

fn lower_scalar_expr_initializer(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    expr_node: &crate::frontend::parser::ast::Expr,
) -> TypedInitializer {
    let value = expr::lower_expr_with_standard_conversions(cx, expr_node);
    let compatible = assignment_compatible_with_const(
        value.ty,
        const_int_value(value.const_value),
        target_ty,
        &cx.types,
    ) || (is_string_literal_expr(expr_node)
        && is_char_pointer_type(cx, target_ty));
    if !compatible {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::TypeMismatch,
            "initializer expression has incompatible type",
            value.span,
        ));
    }
    TypedInitializer::Expr(value)
}

fn try_lower_char_array_string_initializer(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    init: &Initializer,
) -> Option<LoweredInitializer> {
    let (elem_ty, len_kind) = match &cx.types.get(target_ty).kind {
        TypeKind::Array { elem, len } => (*elem, len.clone()),
        _ => return None,
    };

    if !matches!(
        cx.types.get(elem_ty).kind,
        TypeKind::Char | TypeKind::SignedChar | TypeKind::UnsignedChar
    ) {
        return None;
    }

    let text = extract_string_initializer_text(init)?;

    let bytes = text.as_bytes();
    let mut values: Vec<i64> = bytes.iter().map(|b| i64::from(*b)).collect();
    values.push(0);

    let (final_len, write_len) = match len_kind {
        ArrayLen::Known(n) => {
            let bound = n as usize;
            if bound < bytes.len() {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::InvalidInitializer,
                    "string literal is too long for array initializer",
                    init.span,
                ));
            }
            (bound, bound.min(values.len()))
        }
        ArrayLen::Incomplete => (values.len(), values.len()),
        _ => {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::InvalidInitializer,
                "string literal cannot initialize variable-length or flexible array",
                init.span,
            ));
            return Some(LoweredInitializer {
                init: TypedInitializer::ZeroInit { ty: target_ty },
                resulting_ty: target_ty,
            });
        }
    };

    let mut slots: Vec<Option<(TypedInitializer, SourceSpan)>> = vec![None; final_len];
    for (index, value) in values.into_iter().take(write_len).enumerate() {
        let literal = TypedExpr {
            kind: TypedExprKind::Literal(ConstValue::Int(value)),
            ty: elem_ty,
            value_category: ValueCategory::RValue,
            const_value: Some(ConstValue::Int(value)),
            span: init.span,
        };
        slots[index] = Some((TypedInitializer::Expr(literal), init.span));
    }

    let resulting_ty = match &cx.types.get(target_ty).kind {
        TypeKind::Array {
            elem,
            len: ArrayLen::Incomplete,
        } => cx.types.intern(Type {
            kind: TypeKind::Array {
                elem: *elem,
                len: ArrayLen::Known(final_len as u64),
            },
            quals: cx.types.get(target_ty).quals,
        }),
        _ => target_ty,
    };

    Some(LoweredInitializer {
        init: build_aggregate_initializer_from_dense_slots(slots, elem_ty),
        resulting_ty,
    })
}

fn extract_string_initializer_text<'a>(init: &'a Initializer) -> Option<&'a str> {
    match &init.kind {
        InitializerKind::Expr(expr_node) => {
            let ExprKind::Literal(Literal::String(text)) = &expr_node.kind else {
                return None;
            };
            Some(text.as_str())
        }
        // C allows one level of brace-wrapped string initializer for character arrays:
        // `char s[] = { "abc" };`
        InitializerKind::Aggregate(items)
            if items.len() == 1 && items[0].designators.is_empty() =>
        {
            extract_string_initializer_text(&items[0].init)
        }
        _ => None,
    }
}

struct AggregateLowering {
    init: TypedInitializer,
    resulting_ty: TypeId,
    consumed: usize,
}

fn lower_subobject_from_items(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    items: &[InitializerItem],
    stop_at_complete: bool,
) -> AggregateLowering {
    if matches!(
        cx.types.get(target_ty).kind,
        TypeKind::Array { .. } | TypeKind::Record(_)
    ) {
        // Before entering aggregate lowering, check if the first non-designated item
        // is a string literal targeting a char array — this must be handled as a
        // single initializer rather than brace-elided aggregate elements.
        if let Some(first) = items.first()
            && first.designators.is_empty()
        {
            if let Some(lowered) =
                try_lower_char_array_string_initializer(cx, target_ty, &first.init)
            {
                return AggregateLowering {
                    init: lowered.init,
                    resulting_ty: lowered.resulting_ty,
                    consumed: 1,
                };
            }
        }
        let diag_span = items.first().map_or(zero_span(), |i| i.span);
        return lower_aggregate_from_items(cx, target_ty, items, stop_at_complete, diag_span);
    }

    let Some(first) = items.first() else {
        return AggregateLowering {
            init: TypedInitializer::ZeroInit { ty: target_ty },
            resulting_ty: target_ty,
            consumed: 0,
        };
    };

    if !first.designators.is_empty() {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidInitializer,
            "designator cannot target a scalar object",
            first.span,
        ));
    }

    AggregateLowering {
        init: lower_scalar_initializer(cx, target_ty, &first.init),
        resulting_ty: target_ty,
        consumed: 1,
    }
}

fn lower_aggregate_from_items(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    items: &[InitializerItem],
    stop_at_complete: bool,
    diag_span: SourceSpan,
) -> AggregateLowering {
    match &cx.types.get(target_ty).kind {
        TypeKind::Array { elem, len } => {
            lower_array_from_items(cx, target_ty, *elem, len.clone(), items, stop_at_complete)
        }
        TypeKind::Record(record_id) => {
            let record = cx.records.get(*record_id).clone();
            match record.kind {
                crate::frontend::parser::ast::RecordKind::Struct => lower_struct_from_items(
                    cx,
                    target_ty,
                    *record_id,
                    items,
                    stop_at_complete,
                    diag_span,
                ),
                crate::frontend::parser::ast::RecordKind::Union => lower_union_from_items(
                    cx,
                    target_ty,
                    *record_id,
                    items,
                    stop_at_complete,
                    diag_span,
                ),
            }
        }
        _ => AggregateLowering {
            init: TypedInitializer::ZeroInit { ty: target_ty },
            resulting_ty: target_ty,
            consumed: 0,
        },
    }
}

fn lower_array_from_items(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    elem_ty: TypeId,
    len: ArrayLen,
    items: &[InitializerItem],
    stop_at_complete: bool,
) -> AggregateLowering {
    let fixed_len = match len {
        ArrayLen::Known(n) => Some(n as usize),
        ArrayLen::Incomplete => None,
        ArrayLen::FlexibleMember => None,
    };

    let mut slots: std::collections::BTreeMap<usize, (TypedInitializer, SourceSpan)> =
        std::collections::BTreeMap::new();
    let mut max_index_seen: usize = 0;
    let mut cursor = InitCursor {
        object_ty: target_ty,
        path: Vec::new(),
        current_subobject_ty: elem_ty,
        next_implicit_index: 0,
    };
    let mut consumed = 0usize;

    while consumed < items.len() {
        let item = &items[consumed];

        if !item.designators.is_empty() {
            let Some((index, remaining)) =
                resolve_array_designator(cx, &item.designators, item.span, fixed_len)
            else {
                consumed += 1;
                continue;
            };

            let child = lower_designated_child(
                cx,
                elem_ty,
                remaining,
                &item.init,
                item.span,
                &items[consumed + 1..],
            );
            slots.insert(index, (child.init, item.span));
            if index >= max_index_seen {
                max_index_seen = index + 1;
            }
            cursor.next_implicit_index = index + 1;
            consumed += child.consumed;
            continue;
        }

        let target_index = cursor.next_implicit_index;
        if let Some(bound) = fixed_len
            && target_index >= bound
        {
            if stop_at_complete {
                break;
            }
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::InvalidInitializer,
                "too many initializer elements for array object",
                item.span,
            ));
            consumed += 1;
            continue;
        }

        let (child_init, child_ty, used) = if is_aggregate_type(cx, elem_ty)
            && !matches!(item.init.kind, InitializerKind::Aggregate(_))
        {
            let lowered = lower_subobject_from_items(cx, elem_ty, &items[consumed..], true);
            (lowered.init, lowered.resulting_ty, lowered.consumed.max(1))
        } else {
            let lowered = lower_initializer(cx, elem_ty, &item.init);
            (lowered.init, lowered.resulting_ty, 1)
        };

        if child_ty != elem_ty {
            // Nested incomplete-array inference in array elements is not modeled.
            todo!("need implement");
        }

        slots.insert(target_index, (child_init, item.span));
        if target_index >= max_index_seen {
            max_index_seen = target_index + 1;
        }
        cursor.next_implicit_index = target_index + 1;
        consumed += used;
    }

    let final_len = match fixed_len {
        Some(n) => n,
        None => max_index_seen,
    };

    let resulting_ty = if fixed_len.is_none() {
        cx.types.intern(Type {
            kind: TypeKind::Array {
                elem: elem_ty,
                len: ArrayLen::Known(final_len as u64),
            },
            quals: cx.types.get(target_ty).quals,
        })
    } else {
        target_ty
    };

    // Use sparse representation when the array is significantly larger than
    // the number of explicitly initialized elements.
    let init = if final_len > slots.len() * 2 + 16 {
        let entries = slots
            .into_iter()
            .map(|(idx, (init, span))| (idx, TypedInitItem { init, span }))
            .collect();
        TypedInitializer::SparseArray {
            elem_ty,
            total_len: final_len,
            entries,
        }
    } else {
        build_aggregate_initializer_from_slots_map(slots, final_len, elem_ty)
    };

    AggregateLowering {
        init,
        resulting_ty,
        consumed,
    }
}

fn lower_struct_from_items(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    record_id: crate::frontend::sema::types::RecordId,
    items: &[InitializerItem],
    stop_at_complete: bool,
    diag_span: SourceSpan,
) -> AggregateLowering {
    let record = cx.records.get(record_id).clone();
    if !record.is_complete {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::IncompleteType,
            "initializer requires complete struct type",
            diag_span,
        ));
        return AggregateLowering {
            init: TypedInitializer::ZeroInit { ty: target_ty },
            resulting_ty: target_ty,
            consumed: 0,
        };
    }

    let mut slots: Vec<Option<(TypedInitializer, SourceSpan)>> = vec![None; record.fields.len()];
    let mut cursor = InitCursor {
        object_ty: target_ty,
        path: Vec::new(),
        current_subobject_ty: target_ty,
        next_implicit_index: 0,
    };
    let mut consumed = 0usize;

    while consumed < items.len() {
        let item = &items[consumed];

        if !item.designators.is_empty() {
            let Some((field_index, remaining)) =
                resolve_struct_designator(cx, &record, &item.designators, item.span)
            else {
                consumed += 1;
                continue;
            };

            let field_ty = record.fields[field_index].ty;
            let child = lower_designated_child(
                cx,
                field_ty,
                remaining,
                &item.init,
                item.span,
                &items[consumed + 1..],
            );
            slots[field_index] = Some((child.init, item.span));
            cursor.next_implicit_index = field_index + 1;
            consumed += child.consumed;
            continue;
        }

        let target_index = cursor.next_implicit_index;
        if target_index >= record.fields.len() {
            if stop_at_complete {
                break;
            }
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::InvalidInitializer,
                "too many initializer elements for struct object",
                item.span,
            ));
            consumed += 1;
            continue;
        }

        let field_ty = record.fields[target_index].ty;
        let (child_init, used) = if is_aggregate_type(cx, field_ty)
            && !matches!(item.init.kind, InitializerKind::Aggregate(_))
        {
            let lowered = lower_subobject_from_items(cx, field_ty, &items[consumed..], true);
            (lowered.init, lowered.consumed.max(1))
        } else {
            (lower_initializer(cx, field_ty, &item.init).init, 1)
        };

        slots[target_index] = Some((child_init, item.span));
        cursor.next_implicit_index = target_index + 1;
        consumed += used;
    }

    let mut lowered_items = Vec::with_capacity(record.fields.len());
    for (index, field) in record.fields.iter().enumerate() {
        let (init, span) = slots[index]
            .take()
            .unwrap_or((TypedInitializer::ZeroInit { ty: field.ty }, zero_span()));
        lowered_items.push(TypedInitItem { init, span });
    }

    AggregateLowering {
        init: TypedInitializer::Aggregate(lowered_items),
        resulting_ty: target_ty,
        consumed,
    }
}

fn lower_union_from_items(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    record_id: crate::frontend::sema::types::RecordId,
    items: &[InitializerItem],
    stop_at_complete: bool,
    diag_span: SourceSpan,
) -> AggregateLowering {
    let record = cx.records.get(record_id).clone();
    if !record.is_complete || record.fields.is_empty() {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::IncompleteType,
            "initializer requires complete union type",
            diag_span,
        ));
        return AggregateLowering {
            init: TypedInitializer::ZeroInit { ty: target_ty },
            resulting_ty: target_ty,
            consumed: 0,
        };
    }

    let mut active: Option<(usize, TypedInitializer, SourceSpan)> = None;
    let mut consumed = 0usize;

    while consumed < items.len() {
        let item = &items[consumed];
        let (field_index, remaining) = if !item.designators.is_empty() {
            let Some((field_index, remaining)) =
                resolve_union_designator(cx, &record, &item.designators, item.span)
            else {
                consumed += 1;
                continue;
            };
            (field_index, remaining)
        } else {
            (0usize, &[][..])
        };

        if active.is_some() && item.designators.is_empty() {
            if stop_at_complete {
                break;
            }
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::InvalidInitializer,
                "too many initializer elements for union object",
                item.span,
            ));
            consumed += 1;
            continue;
        }

        let field_ty = record.fields[field_index].ty;
        let lowered = if !remaining.is_empty()
            || (is_aggregate_type(cx, field_ty)
                && !matches!(item.init.kind, InitializerKind::Aggregate(_)))
        {
            lower_designated_child(
                cx,
                field_ty,
                remaining,
                &item.init,
                item.span,
                &items[consumed + 1..],
            )
        } else {
            let lowered = lower_initializer(cx, field_ty, &item.init);
            AggregateLowering {
                init: lowered.init,
                resulting_ty: lowered.resulting_ty,
                consumed: 1,
            }
        };

        active = Some((field_index, lowered.init, item.span));
        consumed += lowered.consumed;

        if stop_at_complete {
            break;
        }
    }

    let (field_index, init, span) = active.unwrap_or((
        0,
        TypedInitializer::ZeroInit {
            ty: record.fields[0].ty,
        },
        zero_span(),
    ));

    let mut init_opt = Some(init);
    let lowered_items = record
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            if index == field_index {
                TypedInitItem {
                    init: init_opt
                        .take()
                        .unwrap_or(TypedInitializer::ZeroInit { ty: field.ty }),
                    span,
                }
            } else {
                TypedInitItem {
                    init: TypedInitializer::ZeroInit { ty: field.ty },
                    span: zero_span(),
                }
            }
        })
        .collect();

    AggregateLowering {
        init: TypedInitializer::Aggregate(lowered_items),
        resulting_ty: target_ty,
        consumed,
    }
}

fn lower_designated_child(
    cx: &mut SemaContext<'_>,
    child_ty: TypeId,
    remaining_designators: &[Designator],
    init: &Initializer,
    span: SourceSpan,
    tail_items: &[InitializerItem],
) -> AggregateLowering {
    if is_aggregate_type(cx, child_ty) && !matches!(init.kind, InitializerKind::Aggregate(_)) {
        // If the remaining designators are empty and this is a string literal
        // targeting a char array, handle it directly instead of brace-eliding.
        if remaining_designators.is_empty() {
            if let Some(lowered) = try_lower_char_array_string_initializer(cx, child_ty, init) {
                return AggregateLowering {
                    init: lowered.init,
                    resulting_ty: lowered.resulting_ty,
                    consumed: 1,
                };
            }
        }

        let mut items = Vec::with_capacity(1 + tail_items.len());
        items.push(InitializerItem {
            designators: remaining_designators.to_vec(),
            init: init.clone(),
            span,
        });
        for item in tail_items {
            if !item.designators.is_empty() {
                break;
            }
            items.push(item.clone());
        }
        let lowered = lower_subobject_from_items(cx, child_ty, &items, true);
        return AggregateLowering {
            init: lowered.init,
            resulting_ty: lowered.resulting_ty,
            consumed: lowered.consumed.max(1),
        };
    }

    if remaining_designators.is_empty() {
        let lowered = lower_initializer(cx, child_ty, init);
        return AggregateLowering {
            init: lowered.init,
            resulting_ty: lowered.resulting_ty,
            consumed: 1,
        };
    }

    let nested_item = InitializerItem {
        designators: remaining_designators.to_vec(),
        init: init.clone(),
        span,
    };
    let nested_init = Initializer {
        kind: InitializerKind::Aggregate(vec![nested_item]),
        span,
    };
    let lowered = lower_initializer(cx, child_ty, &nested_init);
    AggregateLowering {
        init: lowered.init,
        resulting_ty: lowered.resulting_ty,
        consumed: 1,
    }
}

fn resolve_array_designator<'a>(
    cx: &mut SemaContext<'_>,
    designators: &'a [Designator],
    span: SourceSpan,
    fixed_len: Option<usize>,
) -> Option<(usize, &'a [Designator])> {
    let Some((first, remaining)) = designators.split_first() else {
        return None;
    };
    let DesignatorKind::Index(index_expr) = &first.kind else {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidInitializer,
            "array initializer designator must start with '[index]'",
            first.span,
        ));
        return None;
    };

    let Some(value) = decl::evaluate_required_integer_constant_expr(
        cx,
        index_expr,
        "array designator index must be an integer constant expression",
    ) else {
        return None;
    };
    if value < 0 {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidInitializer,
            "array designator index must be non-negative",
            span,
        ));
        return None;
    }
    let index = value as usize;
    if let Some(bound) = fixed_len
        && index >= bound
    {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidInitializer,
            "array designator index is out of bounds",
            first.span,
        ));
        return None;
    }
    Some((index, remaining))
}

fn resolve_struct_designator<'a>(
    cx: &mut SemaContext<'_>,
    record: &crate::frontend::sema::types::RecordDef,
    designators: &'a [Designator],
    span: SourceSpan,
) -> Option<(usize, &'a [Designator])> {
    let Some((first, remaining)) = designators.split_first() else {
        return None;
    };
    let DesignatorKind::Field(field_name) = &first.kind else {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidInitializer,
            "struct initializer designator must start with '.field'",
            first.span,
        ));
        return None;
    };

    let Some(index) = record
        .fields
        .iter()
        .position(|field| field.name.as_deref() == Some(field_name.as_str()))
    else {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidInitializer,
            format!("struct has no field named '{field_name}'"),
            span,
        ));
        return None;
    };
    Some((index, remaining))
}

fn resolve_union_designator<'a>(
    cx: &mut SemaContext<'_>,
    record: &crate::frontend::sema::types::RecordDef,
    designators: &'a [Designator],
    span: SourceSpan,
) -> Option<(usize, &'a [Designator])> {
    let Some((first, remaining)) = designators.split_first() else {
        return None;
    };
    let DesignatorKind::Field(field_name) = &first.kind else {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidInitializer,
            "union initializer designator must start with '.field'",
            first.span,
        ));
        return None;
    };

    let Some(index) = record
        .fields
        .iter()
        .position(|field| field.name.as_deref() == Some(field_name.as_str()))
    else {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidInitializer,
            format!("union has no field named '{field_name}'"),
            span,
        ));
        return None;
    };
    Some((index, remaining))
}

fn build_aggregate_initializer_from_dense_slots(
    mut slots: Vec<Option<(TypedInitializer, SourceSpan)>>,
    elem_ty: TypeId,
) -> TypedInitializer {
    let mut items = Vec::with_capacity(slots.len());
    for slot in &mut slots {
        let (init, span) = slot
            .take()
            .unwrap_or((TypedInitializer::ZeroInit { ty: elem_ty }, zero_span()));
        items.push(TypedInitItem { init, span });
    }
    TypedInitializer::Aggregate(items)
}

fn build_aggregate_initializer_from_slots_map(
    slots: std::collections::BTreeMap<usize, (TypedInitializer, SourceSpan)>,
    total_len: usize,
    elem_ty: TypeId,
) -> TypedInitializer {
    let mut items = Vec::with_capacity(total_len);
    for i in 0..total_len {
        let (init, span) = slots
            .get(&i)
            .cloned()
            .unwrap_or((TypedInitializer::ZeroInit { ty: elem_ty }, zero_span()));
        items.push(TypedInitItem { init, span });
    }
    TypedInitializer::Aggregate(items)
}

fn const_int_value(value: Option<ConstValue>) -> Option<i64> {
    match value {
        Some(ConstValue::Int(v)) => Some(v),
        Some(ConstValue::UInt(v)) => i64::try_from(v).ok(),
        _ => None,
    }
}

fn is_aggregate_type(cx: &SemaContext<'_>, ty: TypeId) -> bool {
    matches!(
        cx.types.get(ty).kind,
        TypeKind::Array { .. } | TypeKind::Record(_)
    )
}

fn is_string_literal_expr(expr: &crate::frontend::parser::ast::Expr) -> bool {
    matches!(expr.kind, ExprKind::Literal(Literal::String(_)))
}

fn is_char_pointer_type(cx: &SemaContext<'_>, ty: TypeId) -> bool {
    let TypeKind::Pointer { pointee } = cx.types.get(ty).kind else {
        return false;
    };
    matches!(
        cx.types.get(pointee).kind,
        TypeKind::Char | TypeKind::SignedChar | TypeKind::UnsignedChar
    )
}
