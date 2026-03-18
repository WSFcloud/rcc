use crate::common::span::SourceSpan;
use crate::frontend::parser::ast::{
    BlockItem, CompoundStmt, DirectDeclarator, DirectDeclaratorKind, ForInit, FunctionDef,
    FunctionParams, Stmt, StmtKind,
};
use crate::frontend::sema::check::{decl, expr};
use crate::frontend::sema::context::SemaContext;
use crate::frontend::sema::diagnostic::{SemaDiagnostic, SemaDiagnosticCode};
use crate::frontend::sema::symbols::{DefinitionStatus, Linkage, Symbol, SymbolId, SymbolKind};
use crate::frontend::sema::typed_ast::{
    CaseValue, ConstValue, LabelId, TypedBlockItem, TypedForInit, TypedFunctionDef, TypedStmt,
    TypedStmtKind,
};
use crate::frontend::sema::types::{
    FunctionStyle, FunctionType, Qualifiers, Type, TypeId, TypeKind,
    assignment_compatible_with_const, integer_promotion, is_integer, is_scalar,
};
use std::collections::HashMap;

/// Pass 2 entry point: lower a function definition into a typed AST node.
pub fn lower_function_definition(cx: &mut SemaContext<'_>, func: &FunctionDef) -> TypedFunctionDef {
    let symbol = resolve_function_symbol(cx, func);
    let return_ty = function_return_type(cx, symbol, func.span);

    let mut labels = LabelTracker {
        return_ty: Some(return_ty),
        ..LabelTracker::default()
    };

    // Parameters and outermost function block share one scope level.
    cx.enter_scope();
    labels.enter_scope();

    declare_function_parameters(cx, func);
    let body = lower_compound_stmt(cx, &func.body, &mut labels, false);

    labels.leave_scope();
    labels.finish(cx);
    cx.leave_scope();

    TypedFunctionDef {
        symbol,
        body,
        span: func.span,
    }
}

/// Lower a compound statement (block) into a typed AST node.
fn lower_compound_stmt(
    cx: &mut SemaContext<'_>,
    compound: &CompoundStmt,
    labels: &mut LabelTracker,
    introduce_scope: bool,
) -> TypedStmt {
    if introduce_scope {
        cx.enter_scope();
        labels.enter_scope();
    }

    let mut items = Vec::with_capacity(compound.items.len());
    for item in &compound.items {
        match item {
            BlockItem::Decl(decl_ast) => {
                let typed_decl = decl::lower_local_declaration(cx, decl_ast);
                let initialized = count_initialized_declarators(decl_ast);
                if initialized > 0 {
                    labels.note_initialized_decl(initialized);
                }
                items.push(TypedBlockItem::Declaration(typed_decl));
            }
            BlockItem::Stmt(stmt_ast) => {
                let typed_stmt = lower_stmt(cx, stmt_ast, labels);
                items.push(TypedBlockItem::Stmt(typed_stmt));
            }
        }
    }

    if introduce_scope {
        labels.leave_scope();
        cx.leave_scope();
    }

    TypedStmt {
        kind: TypedStmtKind::Compound(items),
        span: compound.span,
    }
}

/// Lower a single statement into a typed AST node.
fn lower_stmt(cx: &mut SemaContext<'_>, stmt: &Stmt, labels: &mut LabelTracker) -> TypedStmt {
    let kind = match &stmt.kind {
        StmtKind::Empty => TypedStmtKind::Expr(None),
        StmtKind::Expr(expr_ast) => TypedStmtKind::Expr(Some(expr::lower_expr(cx, expr_ast))),
        StmtKind::Compound(compound) => lower_compound_stmt(cx, compound, labels, true).kind,
        StmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            let cond = expr::lower_expr(cx, cond);
            if !is_scalar(cond.ty, &cx.types) {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "if condition must have scalar type",
                    cond.span,
                ));
            }
            let then_branch = Box::new(lower_stmt(cx, then_branch, labels));
            let else_branch = else_branch
                .as_ref()
                .map(|branch| Box::new(lower_stmt(cx, branch, labels)));
            TypedStmtKind::If {
                cond,
                then_branch,
                else_branch,
            }
        }
        StmtKind::Switch {
            expr: switch_expr,
            body,
        } => {
            let typed_expr = expr::lower_expr(cx, switch_expr);
            let switch_control_ty = if is_integer(&cx.types.get(typed_expr.ty).kind) {
                integer_promotion(typed_expr.ty, &mut cx.types)
            } else {
                cx.error_type()
            };
            if !is_integer(&cx.types.get(typed_expr.ty).kind) {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "switch expression must have integer type",
                    typed_expr.span,
                ));
            }

            labels.enter_switch(switch_control_ty);
            let body = Box::new(lower_stmt(cx, body, labels));
            labels.leave_switch();

            TypedStmtKind::Switch {
                expr: typed_expr,
                body,
            }
        }
        StmtKind::While { cond, body } => {
            let cond = expr::lower_expr(cx, cond);
            if !is_scalar(cond.ty, &cx.types) {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "while condition must have scalar type",
                    cond.span,
                ));
            }

            labels.enter_loop();
            let body = Box::new(lower_stmt(cx, body, labels));
            labels.leave_loop();

            TypedStmtKind::While { cond, body }
        }
        StmtKind::DoWhile { body, cond } => {
            labels.enter_loop();
            let body = Box::new(lower_stmt(cx, body, labels));
            labels.leave_loop();

            let cond = expr::lower_expr(cx, cond);
            if !is_scalar(cond.ty, &cx.types) {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "do-while condition must have scalar type",
                    cond.span,
                ));
            }

            TypedStmtKind::DoWhile { body, cond }
        }
        StmtKind::For {
            init,
            cond,
            step,
            body,
        } => {
            cx.enter_scope();
            labels.enter_scope();

            let init = match init {
                Some(ForInit::Expr(e)) => Some(TypedForInit::Expr(expr::lower_expr(cx, e))),
                Some(ForInit::Decl(d)) => {
                    let typed_decl = decl::lower_local_declaration(cx, d);
                    let initialized = count_initialized_declarators(d);
                    if initialized > 0 {
                        labels.note_initialized_decl(initialized);
                    }
                    Some(TypedForInit::Decl(typed_decl))
                }
                None => None,
            };

            let cond = cond.as_ref().map(|c| {
                let lowered = expr::lower_expr(cx, c);
                if !is_scalar(lowered.ty, &cx.types) {
                    cx.emit(SemaDiagnostic::new(
                        SemaDiagnosticCode::TypeMismatch,
                        "for condition must have scalar type",
                        lowered.span,
                    ));
                }
                lowered
            });
            let step = step.as_ref().map(|s| expr::lower_expr(cx, s));

            labels.enter_loop();
            let body = Box::new(lower_stmt(cx, body, labels));
            labels.leave_loop();

            labels.leave_scope();
            cx.leave_scope();

            TypedStmtKind::For {
                init,
                cond,
                step,
                body,
            }
        }
        StmtKind::Return(value) => {
            let return_ty = labels.return_ty.unwrap_or_else(|| cx.error_type());
            let return_is_void = matches!(cx.types.get(return_ty).kind, TypeKind::Void);
            let lowered = value
                .as_ref()
                .map(|expr_ast| expr::lower_expr(cx, expr_ast));

            match lowered.as_ref() {
                Some(expr) if return_is_void => {
                    cx.emit(SemaDiagnostic::new(
                        SemaDiagnosticCode::TypeMismatch,
                        "void function should not return a value",
                        expr.span,
                    ));
                }
                Some(expr) => {
                    if !assignment_compatible_with_const(
                        expr.ty,
                        const_int_value(expr.const_value),
                        return_ty,
                        &cx.types,
                    ) {
                        cx.emit(SemaDiagnostic::new(
                            SemaDiagnosticCode::TypeMismatch,
                            "returned expression has incompatible type",
                            expr.span,
                        ));
                    }
                }
                None if !return_is_void
                    && !matches!(cx.types.get(return_ty).kind, TypeKind::Error) =>
                {
                    cx.emit(SemaDiagnostic::new(
                        SemaDiagnosticCode::TypeMismatch,
                        "non-void function must return a value",
                        stmt.span,
                    ));
                }
                None => {}
            }

            TypedStmtKind::Return(lowered)
        }
        StmtKind::Break => {
            if !labels.can_break() {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "break statement not within loop or switch",
                    stmt.span,
                ));
            }
            TypedStmtKind::Break
        }
        StmtKind::Continue => {
            if !labels.in_loop() {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "continue statement not within a loop",
                    stmt.span,
                ));
            }
            TypedStmtKind::Continue
        }
        StmtKind::Goto(name) => {
            let label = labels.reference(name, stmt.span);
            TypedStmtKind::Goto(label)
        }
        StmtKind::Label { label, stmt: inner } => {
            let label_id = labels.define(cx, label, stmt.span);
            let lowered = Box::new(lower_stmt(cx, inner, labels));
            TypedStmtKind::Label {
                label: label_id,
                stmt: lowered,
            }
        }
        StmtKind::Case {
            expr: case_expr,
            stmt: inner,
        } => {
            if !labels.in_switch() {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "case label is only valid inside switch",
                    stmt.span,
                ));
            }

            let value = match decl::evaluate_required_integer_constant_expr(
                cx,
                case_expr,
                "case label is not a constant expression",
            ) {
                Some(v) => {
                    let converted = labels.note_case(cx, v, stmt.span);
                    CaseValue::Resolved(converted)
                }
                None => CaseValue::Unresolved(expr::lower_expr(cx, case_expr)),
            };
            let lowered = Box::new(lower_stmt(cx, inner, labels));

            TypedStmtKind::Case {
                value,
                stmt: lowered,
            }
        }
        StmtKind::Default { stmt: inner } => {
            if !labels.in_switch() {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "default label is only valid inside switch",
                    stmt.span,
                ));
            } else {
                labels.note_default(cx, stmt.span);
            }

            let lowered = Box::new(lower_stmt(cx, inner, labels));
            TypedStmtKind::Default { stmt: lowered }
        }
    };

    TypedStmt {
        kind,
        span: stmt.span,
    }
}

/// Register function parameters into the current block scope.
fn declare_function_parameters(cx: &mut SemaContext<'_>, func: &FunctionDef) {
    let Some(params) = function_params_from_declarator(&func.declarator.direct) else {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::TypeMismatch,
            "function definition declarator is not a function",
            func.declarator.direct.span,
        ));
        return;
    };

    let FunctionParams::Prototype {
        params: param_list, ..
    } = params
    else {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::UnsupportedKnrDefinition,
            "K&R-style function definitions are not supported in sema V1",
            func.span,
        ));
        return;
    };

    // `int f(void)` declares no parameters.
    if param_list.len() == 1 && param_list[0].declarator.is_none() {
        let ty = decl::build_parameter_type(cx, &param_list[0]);
        if matches!(cx.types.get(ty).kind, TypeKind::Void) {
            return;
        }
    }

    for param in param_list {
        let Some(declarator) = &param.declarator else {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "function definition parameter requires an identifier",
                param.span,
            ));
            continue;
        };
        let Some(name) = decl::declarator_ident(declarator) else {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "function definition parameter requires an identifier",
                param.span,
            ));
            continue;
        };

        if let Some(previous_id) = cx.lookup_ordinary_in_current_scope(name) {
            let previous = cx.symbol(previous_id);
            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::RedeclarationConflict,
                    format!("duplicate parameter name '{name}'"),
                    param.span,
                )
                .with_secondary(
                    previous.decl_span(),
                    "previous parameter declaration is here",
                ),
            );
            continue;
        }

        let ty = decl::build_parameter_type(cx, param);
        if matches!(cx.types.get(ty).kind, TypeKind::Void) {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "parameter cannot have type 'void'",
                param.span,
            ));
            continue;
        }
        let ty = decl::normalize_function_parameter_type(cx, ty);

        let sym = Symbol::new(
            name.to_string(),
            SymbolKind::Object,
            ty,
            Linkage::None,
            DefinitionStatus::Defined,
            param.span,
        );
        let sym_id = cx.insert_symbol(sym);
        if let Err((dup_name, _)) = cx.insert_ordinary(name.to_string(), sym_id) {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::RedeclarationConflict,
                format!("duplicate parameter name '{dup_name}'"),
                param.span,
            ));
        }
    }
}

struct SwitchState {
    control_ty: TypeId,
    cases: HashMap<i64, SourceSpan>,
    default_span: Option<SourceSpan>,
}

/// Tracks label definitions, goto references, and jump-over-initializer
/// violations within a single function body.
#[derive(Default)]
struct LabelTracker {
    next_id: u32,
    ids: HashMap<String, LabelId>,
    defined: HashMap<String, (SourceSpan, usize)>,
    gotos: Vec<(String, SourceSpan, usize)>,
    active_initializer_depth: usize,
    scope_initializer_counts: Vec<usize>,
    return_ty: Option<TypeId>,
    loop_depth: usize,
    switch_stack: Vec<SwitchState>,
}

impl LabelTracker {
    /// Push a new scope level onto the initializer tracking stack.
    fn enter_scope(&mut self) {
        self.scope_initializer_counts.push(0);
    }

    /// Pop the current scope level and subtract its initializer count
    /// from `active_initializer_depth`.
    fn leave_scope(&mut self) {
        if let Some(count) = self.scope_initializer_counts.pop() {
            self.active_initializer_depth = self.active_initializer_depth.saturating_sub(count);
        }
    }

    /// Record that `count` initialized declarations were encountered in
    /// the current scope.
    fn note_initialized_decl(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        if let Some(current_scope_count) = self.scope_initializer_counts.last_mut() {
            *current_scope_count += count;
        }
        self.active_initializer_depth += count;
    }

    /// Record a goto reference to `name`.
    fn reference(&mut self, name: &str, span: SourceSpan) -> LabelId {
        let id = self.ensure_id(name);
        self.gotos
            .push((name.to_string(), span, self.active_initializer_depth));
        id
    }

    /// Record a label definition.
    fn define(&mut self, cx: &mut SemaContext<'_>, name: &str, span: SourceSpan) -> LabelId {
        let id = self.ensure_id(name);
        if let Some((previous_span, _)) = self.defined.get(name) {
            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::DuplicateLabel,
                    format!("duplicate label '{name}'"),
                    span,
                )
                .with_secondary(*previous_span, "previous label is here"),
            );
            return id;
        }
        self.defined
            .insert(name.to_string(), (span, self.active_initializer_depth));
        id
    }

    /// Finalize label tracking at the end of a function body.
    fn finish(self, cx: &mut SemaContext<'_>) {
        for (name, goto_span, goto_depth) in self.gotos {
            let Some((label_span, label_depth)) = self.defined.get(&name) else {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::UndefinedLabel,
                    format!("goto to undefined label '{name}'"),
                    goto_span,
                ));
                continue;
            };

            if goto_depth < *label_depth {
                cx.emit(
                    SemaDiagnostic::new(
                        SemaDiagnosticCode::JumpOverInitializer,
                        format!("goto to '{name}' jumps over initialized declaration(s)"),
                        goto_span,
                    )
                    .with_secondary(*label_span, "label target is here"),
                );
            }
        }
    }

    /// Return or create a stable `LabelId` for the given label name.
    fn ensure_id(&mut self, name: &str) -> LabelId {
        if let Some(id) = self.ids.get(name).copied() {
            return id;
        }
        let id = LabelId(self.next_id);
        self.next_id += 1;
        self.ids.insert(name.to_string(), id);
        id
    }

    fn enter_loop(&mut self) {
        self.loop_depth += 1;
    }

    fn leave_loop(&mut self) {
        self.loop_depth = self.loop_depth.saturating_sub(1);
    }

    fn in_loop(&self) -> bool {
        self.loop_depth > 0
    }

    fn enter_switch(&mut self, control_ty: TypeId) {
        self.switch_stack.push(SwitchState {
            control_ty,
            cases: HashMap::new(),
            default_span: None,
        });
    }

    fn leave_switch(&mut self) {
        let _ = self.switch_stack.pop();
    }

    fn in_switch(&self) -> bool {
        !self.switch_stack.is_empty()
    }

    fn can_break(&self) -> bool {
        self.in_loop() || self.in_switch()
    }

    fn note_case(&mut self, cx: &mut SemaContext<'_>, value: i64, span: SourceSpan) -> i64 {
        let Some(current_switch) = self.switch_stack.last_mut() else {
            return value;
        };
        let converted = decl::cast_ice_integer_value(cx, value, current_switch.control_ty);
        if let Some(previous_span) = current_switch.cases.get(&converted).copied() {
            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::RedeclarationConflict,
                    format!("duplicate case value '{converted}'"),
                    span,
                )
                .with_secondary(previous_span, "previous case value is here"),
            );
            return converted;
        }
        current_switch.cases.insert(converted, span);
        converted
    }

    fn note_default(&mut self, cx: &mut SemaContext<'_>, span: SourceSpan) {
        let Some(current_switch) = self.switch_stack.last_mut() else {
            return;
        };
        if let Some(previous_span) = current_switch.default_span {
            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::RedeclarationConflict,
                    "multiple default labels in one switch",
                    span,
                )
                .with_secondary(previous_span, "previous default label is here"),
            );
            return;
        }
        current_switch.default_span = Some(span);
    }
}

fn resolve_function_symbol(cx: &mut SemaContext<'_>, func: &FunctionDef) -> SymbolId {
    let Some(name) = decl::function_name(func) else {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::TypeMismatch,
            "function definition requires an identifier",
            func.span,
        ));
        return make_fallback_function_symbol(cx, func.span);
    };

    let Some(symbol_id) = cx.lookup_ordinary(name) else {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::UndefinedSymbol,
            format!("missing pass1 symbol for function '{name}'"),
            func.span,
        ));
        return make_fallback_function_symbol(cx, func.span);
    };

    if cx.symbol(symbol_id).kind() != SymbolKind::Function {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::TypeMismatch,
            format!("'{name}' is not a function symbol"),
            func.span,
        ));
        return make_fallback_function_symbol(cx, func.span);
    }

    symbol_id
}

fn make_fallback_function_symbol(cx: &mut SemaContext<'_>, span: SourceSpan) -> SymbolId {
    let fallback_ty = cx.types.intern(Type {
        kind: TypeKind::Function(FunctionType {
            ret: cx.error_type(),
            params: Vec::new(),
            variadic: false,
            style: FunctionStyle::Prototype,
        }),
        quals: Qualifiers::default(),
    });
    cx.insert_symbol(Symbol::new(
        "<invalid-function>".to_string(),
        SymbolKind::Function,
        fallback_ty,
        Linkage::None,
        DefinitionStatus::Defined,
        span,
    ))
}

fn function_return_type(cx: &mut SemaContext<'_>, symbol: SymbolId, span: SourceSpan) -> TypeId {
    let ty = cx.symbol(symbol).ty();
    let kind = cx.types.get(ty).kind.clone();
    match kind {
        TypeKind::Function(function_ty) => function_ty.ret,
        TypeKind::Pointer { pointee } => match cx.types.get(pointee).kind {
            TypeKind::Function(ref function_ty) => function_ty.ret,
            _ => {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "function symbol does not have function type",
                    span,
                ));
                cx.error_type()
            }
        },
        _ => {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "function symbol does not have function type",
                span,
            ));
            cx.error_type()
        }
    }
}

fn function_params_from_declarator(direct: &DirectDeclarator) -> Option<&FunctionParams> {
    match &direct.kind {
        DirectDeclaratorKind::Function { params, .. } => Some(params),
        DirectDeclaratorKind::Array { inner, .. } => function_params_from_declarator(inner),
        DirectDeclaratorKind::Grouped(inner) => function_params_from_declarator(&inner.direct),
        DirectDeclaratorKind::Ident(_) | DirectDeclaratorKind::Abstract => None,
    }
}

fn const_int_value(value: Option<ConstValue>) -> Option<i64> {
    match value {
        Some(ConstValue::Int(v)) => Some(v),
        Some(ConstValue::UInt(v)) => i64::try_from(v).ok(),
        _ => None,
    }
}

/// Count the number of declarators with initializers in a declaration.
fn count_initialized_declarators(decl: &crate::frontend::parser::ast::Declaration) -> usize {
    decl.declarators.iter().filter(|d| d.init.is_some()).count()
}
