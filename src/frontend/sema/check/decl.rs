use crate::common::span::SourceSpan;
use crate::frontend::parser::ast::{
    ArraySize, DeclSpec, Declaration, Declarator, DirectDeclarator, DirectDeclaratorKind,
    EnumSpecifier, ExternalDecl, FunctionDef, FunctionParams, FunctionSpecifier, ParameterDecl,
    Pointer, RecordKind, RecordMemberDecl, RecordSpecifier, StorageClass, TranslationUnit,
    TypeName, TypeQualifier, TypeSpecifier,
};
use crate::frontend::sema::context::SemaContext;
use crate::frontend::sema::diagnostic::{SemaDiagnostic, SemaDiagnosticCode};
use crate::frontend::sema::init;
use crate::frontend::sema::symbols::{
    DefinitionStatus, Linkage, LinkageError, Symbol, SymbolId, SymbolKind, infer_linkage,
};
use crate::frontend::sema::typed_ast::{TypedDeclInit, TypedDeclaration, TypedInitializer};
use crate::frontend::sema::types::{
    ArrayLen, EnumConstant, EnumDef, FieldDef, Qualifiers, RecordDef, TagId, Type, TypeId,
    TypeKind, type_size_of,
};

/// Pass 1 entry for declaration processing at translation-unit scope.
///
/// This keeps only framework traversal so later semantic passes can be plugged in.
pub fn pass1_translation_unit(cx: &mut SemaContext<'_>, tu: &TranslationUnit) {
    for item in &tu.items {
        match item {
            ExternalDecl::Declaration(decl) => declare_file_scope_declaration(cx, decl),
            ExternalDecl::FunctionDef(func) => {
                if !func.declarations.is_empty() {
                    cx.emit(SemaDiagnostic::new(
                        SemaDiagnosticCode::UnsupportedKnrDefinition,
                        "K&R-style function definitions are not supported in sema V1",
                        func.span,
                    ));
                }
                let _ = ensure_function_symbol(cx, func);
            }
        }
    }
}

/// Build a typed declaration node by referencing symbols registered in pass 1.
pub fn lower_external_declaration(
    cx: &mut SemaContext<'_>,
    decl: &Declaration,
) -> TypedDeclaration {
    let mut symbols = Vec::new();
    let mut initializers = Vec::new();

    for init_decl in &decl.declarators {
        let Some(name) = declarator_ident(&init_decl.declarator) else {
            continue;
        };
        if let Some(symbol_id) = cx.lookup_ordinary(name) {
            symbols.push(symbol_id);
            if let Some(init_ast) = &init_decl.init {
                let decl_span = init_decl.declarator.direct.span.join(init_ast.span);
                if has_prior_initializer_diagnostic(cx, decl_span) {
                    continue;
                }
                let target_ty = cx.symbol(symbol_id).ty();
                let lowered = init::lower_initializer(cx, target_ty, init_ast);
                initializers.push(TypedDeclInit {
                    symbol: symbol_id,
                    init: lowered.init,
                });
            }
        }
    }

    TypedDeclaration {
        symbols,
        initializers,
        span: declaration_span(decl),
    }
}

/// Finalize tentative definitions at end of translation unit.
pub fn finalize_tentative_definitions(cx: &mut SemaContext<'_>) -> Vec<TypedDeclaration> {
    let mut synthesized = Vec::new();

    for idx in 0..cx.symbols.len() {
        let symbol_id = SymbolId(idx as u32);
        let (is_tentative_object, ty, span, name) = {
            let symbol = cx.symbol(symbol_id);
            (
                symbol.kind() == SymbolKind::Object
                    && symbol.status() == DefinitionStatus::Tentative,
                symbol.ty(),
                symbol.decl_span(),
                symbol.name().to_string(),
            )
        };

        if !is_tentative_object {
            continue;
        }

        if !is_complete_type(ty, cx) {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::IncompleteType,
                format!("tentative definition of '{name}' has incomplete type"),
                span,
            ));
            continue;
        }

        cx.symbol_mut(symbol_id)
            .set_status(DefinitionStatus::Defined);
        synthesized.push(TypedDeclaration {
            symbols: vec![symbol_id],
            initializers: vec![TypedDeclInit {
                symbol: symbol_id,
                init: TypedInitializer::ZeroInit { ty },
            }],
            span,
        });
    }

    synthesized
}

/// Ensure one function symbol exists in the current scope and return its symbol id.
pub fn ensure_function_symbol(cx: &mut SemaContext<'_>, func: &FunctionDef) -> Option<SymbolId> {
    let Some(name) = function_name(func) else {
        return None;
    };

    let ty = build_decl_type(
        cx,
        &func.specifiers,
        &func.declarator,
        func.declarator.direct.span,
    );

    let storage = match normalize_storage(&func.specifiers, func.span) {
        Ok(s) => s,
        Err(diag) => {
            cx.emit(diag);
            return None;
        }
    };

    let existing_id = cx.lookup_ordinary(name);
    let linkage = match infer_linkage(
        SymbolKind::Function,
        cx.scope_level(),
        storage,
        existing_id.map(|id| cx.symbol(id)),
    ) {
        Ok(l) => l,
        Err(err) => {
            cx.emit(linkage_error_to_diag(err, func.span));
            return None;
        }
    };

    // Check for existing symbol and merge.
    if let Some(existing_id) = existing_id {
        let decl_info = crate::frontend::sema::symbols::DeclInfo {
            name,
            kind: SymbolKind::Function,
            ty,
            linkage,
            status: DefinitionStatus::Defined,
            span: func.declarator.direct.span,
        };
        if let Err(diag) = crate::frontend::sema::symbols::merge_declarations(
            cx.symbols.get_mut(existing_id),
            &decl_info,
            &mut cx.types,
        ) {
            cx.emit(diag);
        }
        Some(existing_id)
    } else {
        let sym = Symbol::new(
            name.to_string(),
            SymbolKind::Function,
            ty,
            linkage,
            DefinitionStatus::Defined,
            func.declarator.direct.span,
        );
        let sym_id = cx.insert_symbol(sym);
        let _ = cx.insert_ordinary(name.to_string(), sym_id);
        Some(sym_id)
    }
}

/// Lower one local declaration inside a block scope.
pub fn lower_local_declaration(cx: &mut SemaContext<'_>, decl: &Declaration) -> TypedDeclaration {
    let mut symbols = Vec::new();
    let mut initializers = Vec::new();
    let span = declaration_span(decl);

    let storage = match normalize_storage(&decl.specifiers, span) {
        Ok(s) => s,
        Err(diag) => {
            cx.emit(diag);
            return TypedDeclaration {
                symbols,
                initializers,
                span,
            };
        }
    };
    let is_typedef = decl.specifiers.storage.contains(&StorageClass::Typedef);
    let has_inline = decl
        .specifiers
        .function
        .contains(&FunctionSpecifier::Inline);

    // Handle declarations without declarators (e.g., block-scope tag declarations).
    if decl.declarators.is_empty() {
        if is_typedef {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "typedef declaration requires at least one declarator",
                span,
            ));
        }
        if has_inline {
            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "'inline' can only be applied to function declarations",
                    span,
                )
                .with_note("remove 'inline' or declare at least one function declarator"),
            );
        }
        let _ = resolve_base_type(cx, &decl.specifiers, span, false);
        return TypedDeclaration {
            symbols,
            initializers,
            span,
        };
    }

    let base_ty = resolve_base_type(cx, &decl.specifiers, span, false);

    if is_typedef && has_inline {
        cx.emit(
            SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "'inline' cannot be combined with 'typedef'",
                span,
            )
            .with_note("only function declarations/definitions may use 'inline'"),
        );
    }

    for init_decl in &decl.declarators {
        let Some(name) = declarator_ident(&init_decl.declarator) else {
            continue;
        };
        let mut ty = apply_declarator_with_base(cx, base_ty, &init_decl.declarator);

        if is_typedef {
            if init_decl.init.is_some() {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::InvalidInitializer,
                    "typedef declaration cannot have an initializer",
                    init_decl.declarator.direct.span,
                ));
            }

            if let Some(existing_id) = cx.lookup_ordinary_in_current_scope(name) {
                let existing = cx.symbol(existing_id);
                if existing.kind() == SymbolKind::Typedef && existing.ty() == ty {
                    symbols.push(existing_id);
                    continue;
                }
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::RedeclarationConflict,
                    format!("redefinition of '{name}'"),
                    init_decl.declarator.direct.span,
                ));
                continue;
            }

            let sym = Symbol::new(
                name.to_string(),
                SymbolKind::Typedef,
                ty,
                Linkage::None,
                DefinitionStatus::Defined,
                init_decl.declarator.direct.span,
            );
            let sym_id = cx.insert_symbol(sym);
            if let Err((dup_name, _)) = cx.insert_ordinary(name.to_string(), sym_id) {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::RedeclarationConflict,
                    format!("redeclaration of '{dup_name}'"),
                    init_decl.declarator.direct.span,
                ));
                continue;
            }
            symbols.push(sym_id);
            continue;
        }

        let mut kind = if matches!(cx.types.get(ty).kind, TypeKind::Function(_)) {
            SymbolKind::Function
        } else {
            SymbolKind::Object
        };
        let mut typed_initializer = None;

        if let Some(init) = &init_decl.init {
            if kind == SymbolKind::Function {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::InvalidInitializer,
                    "function declaration cannot have an initializer",
                    init.span,
                ));
            }

            if storage == Some(StorageClass::Extern) {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::InvalidInitializer,
                    "extern declaration cannot have an initializer",
                    init.span,
                ));
            }

            if kind != SymbolKind::Function && storage != Some(StorageClass::Extern) {
                let lowered = init::lower_initializer(cx, ty, init);
                if storage == Some(StorageClass::Static)
                    && !init::is_constant_initializer(cx, &lowered.init)
                {
                    cx.emit(SemaDiagnostic::new(
                        SemaDiagnosticCode::NonConstantInRequiredContext,
                        format!("initializer for '{name}' is not a constant expression"),
                        init.span,
                    ));
                }
                ty = lowered.resulting_ty;
                typed_initializer = Some(lowered.init);
            }
        }

        kind = if matches!(cx.types.get(ty).kind, TypeKind::Function(_)) {
            SymbolKind::Function
        } else {
            SymbolKind::Object
        };
        enforce_inline_function_only(cx, has_inline, kind, init_decl.declarator.direct.span);

        if kind == SymbolKind::Object
            && storage != Some(StorageClass::Extern)
            && !is_complete_type(ty, cx)
        {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::IncompleteType,
                format!("declaration of '{name}' has incomplete type"),
                init_decl.declarator.direct.span,
            ));
        }

        let existing_in_current = cx.lookup_ordinary_in_current_scope(name);
        if let Some(existing_id) = existing_in_current {
            let existing = cx.symbol(existing_id);
            if kind == SymbolKind::Object && existing.linkage() == Linkage::None {
                cx.emit(
                    SemaDiagnostic::new(
                        SemaDiagnosticCode::RedeclarationConflict,
                        format!("redeclaration of '{name}' in the same block scope"),
                        init_decl.declarator.direct.span,
                    )
                    .with_secondary(existing.decl_span(), "previous declaration is here"),
                );
                continue;
            }
        }

        let visible_linkage_entity = cx.lookup_ordinary(name).filter(|id| {
            let sym = cx.symbol(*id);
            if sym.linkage() == Linkage::None {
                return false;
            }
            if kind == SymbolKind::Function {
                sym.kind() == SymbolKind::Function
            } else {
                true
            }
        });

        let merge_target = if let Some(existing_id) = existing_in_current {
            Some(existing_id)
        } else if kind == SymbolKind::Function || storage == Some(StorageClass::Extern) {
            visible_linkage_entity
        } else {
            None
        };

        let linkage = match infer_linkage(
            kind,
            cx.scope_level(),
            storage,
            merge_target.map(|id| cx.symbol(id)),
        ) {
            Ok(l) => l,
            Err(err) => {
                cx.emit(linkage_error_to_diag(err, init_decl.declarator.direct.span));
                continue;
            }
        };

        let status = if init_decl.init.is_some() {
            DefinitionStatus::Defined
        } else {
            DefinitionStatus::Declared
        };

        if let Some(existing_id) = merge_target {
            let decl_info = crate::frontend::sema::symbols::DeclInfo {
                name,
                kind,
                ty,
                linkage,
                status,
                span: init_decl.declarator.direct.span,
            };
            if let Err(diag) = crate::frontend::sema::symbols::merge_declarations(
                cx.symbols.get_mut(existing_id),
                &decl_info,
                &mut cx.types,
            ) {
                cx.emit(diag);
                continue;
            }
            symbols.push(existing_id);
            if let Some(init) = typed_initializer.take() {
                initializers.push(TypedDeclInit {
                    symbol: existing_id,
                    init,
                });
            }
            continue;
        }

        let sym = Symbol::new(
            name.to_string(),
            kind,
            ty,
            linkage,
            status,
            init_decl.declarator.direct.span,
        );
        let sym_id = cx.insert_symbol(sym);
        if let Err((dup_name, _)) = cx.insert_ordinary(name.to_string(), sym_id) {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::RedeclarationConflict,
                format!("redeclaration of '{dup_name}'"),
                init_decl.declarator.direct.span,
            ));
            continue;
        }
        symbols.push(sym_id);
        if let Some(init) = typed_initializer.take() {
            initializers.push(TypedDeclInit {
                symbol: sym_id,
                init,
            });
        }
    }

    TypedDeclaration {
        symbols,
        initializers,
        span,
    }
}

/// Extract the identifier from a declarator.
///
/// Returns `None` for abstract declarators.
pub fn declarator_ident(declarator: &Declarator) -> Option<&str> {
    direct_declarator_ident(declarator.direct.as_ref())
}

/// Extract the function name from a function definition declarator.
pub fn function_name(func: &FunctionDef) -> Option<&str> {
    declarator_ident(&func.declarator)
}

/// Build semantic parameter type from parser parameter declaration.
pub(crate) fn build_parameter_type(cx: &mut SemaContext<'_>, parameter: &ParameterDecl) -> TypeId {
    let base_ty = resolve_base_type(cx, &parameter.specifiers, parameter.span, true);
    if let Some(declarator) = &parameter.declarator {
        apply_declarator_with_base(cx, base_ty, declarator)
    } else {
        base_ty
    }
}

/// Extract the identifier from a direct declarator node.
fn direct_declarator_ident(declarator: &DirectDeclarator) -> Option<&str> {
    match &declarator.kind {
        DirectDeclaratorKind::Ident(name) => Some(name.as_str()),
        DirectDeclaratorKind::Grouped(inner) => declarator_ident(inner),
        DirectDeclaratorKind::Array { inner, .. }
        | DirectDeclaratorKind::Function { inner, .. } => direct_declarator_ident(inner),
        DirectDeclaratorKind::Abstract => None,
    }
}

/// Register one file-scope declaration in pass 1.
fn declare_file_scope_declaration(cx: &mut SemaContext<'_>, decl: &Declaration) {
    let storage = match normalize_storage(&decl.specifiers, declaration_span(decl)) {
        Ok(s) => s,
        Err(diag) => {
            cx.emit(diag);
            return;
        }
    };

    let is_typedef = decl.specifiers.storage.contains(&StorageClass::Typedef);
    let has_inline = decl
        .specifiers
        .function
        .contains(&FunctionSpecifier::Inline);

    // Handle declarations without declarators (e.g., `struct S {};` or `enum E { A };`).
    if decl.declarators.is_empty() {
        if is_typedef {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "typedef declaration requires at least one declarator",
                declaration_span(decl),
            ));
            return;
        }
        if has_inline {
            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "'inline' can only be applied to function declarations",
                    declaration_span(decl),
                )
                .with_note("remove 'inline' or declare at least one function declarator"),
            );
        }
        // Trigger type resolution to register tags.
        let _ = resolve_base_type(cx, &decl.specifiers, declaration_span(decl), false);
        return;
    }

    // Resolve base type once to avoid re-entering struct/enum definitions.
    let base_ty = resolve_base_type(cx, &decl.specifiers, declaration_span(decl), true);

    if is_typedef && has_inline {
        cx.emit(
            SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "'inline' cannot be combined with 'typedef'",
                declaration_span(decl),
            )
            .with_note("only function declarations/definitions may use 'inline'"),
        );
    }

    for init_decl in &decl.declarators {
        let Some(name) = declarator_ident(&init_decl.declarator) else {
            continue;
        };

        let mut ty = apply_declarator_with_base(cx, base_ty, &init_decl.declarator);

        if is_typedef {
            // Typedef redeclaration: C11 6.7p3 allows identical typedef redeclaration.
            if let Some(existing_id) = cx.lookup_ordinary(name) {
                let existing = cx.symbol(existing_id);
                if existing.kind() == SymbolKind::Typedef && existing.ty() == ty {
                    // Identical typedef redeclaration — silently accept.
                    continue;
                }
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::RedeclarationConflict,
                    format!("redefinition of '{name}'"),
                    init_decl.declarator.direct.span,
                ));
                continue;
            }
            // First declaration — insert symbol then register in namespace.
            let sym = Symbol::new(
                name.to_string(),
                SymbolKind::Typedef,
                ty,
                Linkage::None,
                DefinitionStatus::Defined,
                init_decl.declarator.direct.span,
            );
            let sym_id = cx.insert_symbol(sym);
            let _ = cx.insert_ordinary(name.to_string(), sym_id);
            continue;
        }

        let mut kind = if matches!(cx.types.get(ty).kind, TypeKind::Function(_)) {
            SymbolKind::Function
        } else {
            SymbolKind::Object
        };

        if let Some(init) = &init_decl.init {
            if kind == SymbolKind::Function {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::InvalidInitializer,
                    "function declaration cannot have an initializer",
                    init.span,
                ));
                continue;
            }

            let lowered = init::lower_initializer(cx, ty, init);
            if !init::is_constant_initializer(cx, &lowered.init) {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::NonConstantInRequiredContext,
                    format!("initializer for '{name}' is not a constant expression"),
                    init.span,
                ));
            }
            ty = lowered.resulting_ty;
        }

        kind = if matches!(cx.types.get(ty).kind, TypeKind::Function(_)) {
            SymbolKind::Function
        } else {
            SymbolKind::Object
        };
        enforce_inline_function_only(cx, has_inline, kind, init_decl.declarator.direct.span);

        let existing_id = cx.lookup_ordinary(name);
        let linkage = match infer_linkage(
            kind,
            cx.scope_level(),
            storage,
            existing_id.map(|id| cx.symbol(id)),
        ) {
            Ok(l) => l,
            Err(err) => {
                cx.emit(linkage_error_to_diag(err, init_decl.declarator.direct.span));
                continue;
            }
        };

        let status = if init_decl.init.is_some() {
            DefinitionStatus::Defined
        } else if kind == SymbolKind::Object
            && linkage != Linkage::None
            && storage != Some(StorageClass::Extern)
        {
            DefinitionStatus::Tentative
        } else {
            DefinitionStatus::Declared
        };

        // Check for existing symbol and merge if compatible.
        if let Some(existing_id) = existing_id {
            let decl_info = crate::frontend::sema::symbols::DeclInfo {
                name,
                kind,
                ty,
                linkage,
                status,
                span: init_decl.declarator.direct.span,
            };
            if let Err(diag) = crate::frontend::sema::symbols::merge_declarations(
                cx.symbols.get_mut(existing_id),
                &decl_info,
                &mut cx.types,
            ) {
                cx.emit(diag);
            }
        } else {
            let sym = Symbol::new(
                name.to_string(),
                kind,
                ty,
                linkage,
                status,
                init_decl.declarator.direct.span,
            );
            let sym_id = cx.insert_symbol(sym);
            let _ = cx.insert_ordinary(name.to_string(), sym_id);
        }
    }
}

/// Validate and normalize storage-class specifiers.
///
/// `typedef` is syntactically a storage class but semantically not one;
/// it is filtered out here so callers can treat the result as a true storage class.
fn normalize_storage(
    specifiers: &DeclSpec,
    span: SourceSpan,
) -> Result<Option<StorageClass>, SemaDiagnostic> {
    let real_storage: Vec<_> = specifiers
        .storage
        .iter()
        .filter(|s| !matches!(s, StorageClass::Typedef))
        .collect();
    if real_storage.len() > 1 {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidLinkageMerge,
            "multiple storage-class specifiers are not yet normalized",
            span,
        ));
    }
    Ok(real_storage.first().map(|s| **s))
}

fn validate_array_element_type(
    cx: &mut SemaContext<'_>,
    elem_ty: TypeId,
    span: SourceSpan,
) -> TypeId {
    let kind = &cx.types.get(elem_ty).kind;
    if matches!(kind, TypeKind::Error) {
        return elem_ty;
    }

    if matches!(kind, TypeKind::Function(_)) {
        cx.emit(
            SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "array element type cannot be a function type",
                span,
            )
            .with_note("declare an array of function pointers instead"),
        );
        return cx.error_type();
    }

    if !is_complete_type(elem_ty, cx) {
        cx.emit(
            SemaDiagnostic::new(
                SemaDiagnosticCode::IncompleteType,
                "array element type must be complete",
                span,
            )
            .with_note("complete the element type or use a pointer element type"),
        );
        return cx.error_type();
    }

    elem_ty
}

fn validate_function_return_type(
    cx: &mut SemaContext<'_>,
    ret_ty: TypeId,
    span: SourceSpan,
) -> TypeId {
    match cx.types.get(ret_ty).kind {
        TypeKind::Error => ret_ty,
        TypeKind::Array { .. } => {
            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "function return type cannot be an array type",
                    span,
                )
                .with_note("return a pointer to array instead"),
            );
            cx.error_type()
        }
        TypeKind::Function(_) => {
            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    "function return type cannot be a function type",
                    span,
                )
                .with_note("return a function pointer instead"),
            );
            cx.error_type()
        }
        _ => ret_ty,
    }
}

/// Build full declaration type from specifiers and declarator.
fn build_decl_type(
    cx: &mut SemaContext<'_>,
    specifiers: &DeclSpec,
    declarator: &Declarator,
    span: SourceSpan,
) -> TypeId {
    let base_ty = resolve_base_type(cx, specifiers, span, true);
    apply_declarator_with_base(cx, base_ty, declarator)
}

/// Apply declarator modifiers over a computed base type.
///
/// Apply by declarator precedence:
/// - First apply pointer layers on this declarator node.
/// - Then thread direct-declarator suffixes (`[]`, `()`) through `Grouped`.
fn apply_declarator_with_base(
    cx: &mut SemaContext<'_>,
    base_ty: TypeId,
    declarator: &Declarator,
) -> TypeId {
    let base_with_pointers = apply_pointer_layers(cx, base_ty, &declarator.pointers);
    apply_direct_declarator(cx, base_with_pointers, &declarator.direct)
}

/// Apply pointer layers (`*`) over a type, from innermost to outermost.
fn apply_pointer_layers(cx: &mut SemaContext<'_>, mut ty: TypeId, pointers: &[Pointer]) -> TypeId {
    for ptr in pointers {
        let quals = qualifiers_from_slice(&ptr.qualifiers);
        ty = cx.types.intern(Type {
            kind: TypeKind::Pointer { pointee: ty },
            quals,
        });
    }
    ty
}

/// Apply one direct declarator chain (array/function/grouped/identifier).
fn apply_direct_declarator(
    cx: &mut SemaContext<'_>,
    base_ty: TypeId,
    direct: &DirectDeclarator,
) -> TypeId {
    match &direct.kind {
        DirectDeclaratorKind::Ident(_) | DirectDeclaratorKind::Abstract => base_ty,
        DirectDeclaratorKind::Grouped(inner) => apply_declarator_with_base(cx, base_ty, inner),
        DirectDeclaratorKind::Array { inner, size, .. } => {
            let len = array_len_from_size_expr(cx, size, direct.span);
            let elem_ty = validate_array_element_type(cx, base_ty, direct.span);
            let array_ty = cx.types.intern(Type {
                kind: TypeKind::Array { elem: elem_ty, len },
                quals: Qualifiers::default(),
            });
            apply_direct_declarator(cx, array_ty, inner)
        }
        DirectDeclaratorKind::Function { inner, params } => {
            let function_ty = build_function_type(cx, base_ty, params, direct.span);
            apply_direct_declarator(cx, function_ty, inner)
        }
    }
}

/// Build function type from return type and parsed parameter list.
fn build_function_type(
    cx: &mut SemaContext<'_>,
    mut ret_ty: TypeId,
    params: &FunctionParams,
    span: SourceSpan,
) -> TypeId {
    use crate::frontend::sema::types::{FunctionStyle, FunctionType};

    ret_ty = validate_function_return_type(cx, ret_ty, span);

    match params {
        FunctionParams::NonPrototype => cx.types.intern(Type {
            kind: TypeKind::Function(FunctionType {
                ret: ret_ty,
                params: Vec::new(),
                variadic: false,
                style: FunctionStyle::NonPrototype,
            }),
            quals: Qualifiers::default(),
        }),
        FunctionParams::Prototype { params, variadic } => {
            // Special case: single void parameter means zero parameters.
            let param_types = if params.len() == 1 && params[0].declarator.is_none() {
                let base_ty = resolve_base_type(cx, &params[0].specifiers, params[0].span, true);
                if is_void_type(cx, base_ty) {
                    Vec::new()
                } else {
                    vec![normalize_function_parameter_type(cx, base_ty)]
                }
            } else {
                let mut lowered = Vec::with_capacity(params.len());
                for param in params {
                    let ty = build_parameter_type(cx, param);
                    if is_void_type(cx, ty) {
                        cx.emit(SemaDiagnostic::new(
                            SemaDiagnosticCode::TypeMismatch,
                            "parameter list with 'void' must contain exactly one unnamed parameter",
                            param.span,
                        ));
                        lowered.push(cx.error_type());
                    } else {
                        lowered.push(normalize_function_parameter_type(cx, ty));
                    }
                }
                lowered
            };

            cx.types.intern(Type {
                kind: TypeKind::Function(FunctionType {
                    ret: ret_ty,
                    params: param_types,
                    variadic: *variadic,
                    style: FunctionStyle::Prototype,
                }),
                quals: Qualifiers::default(),
            })
        }
    }
}

/// Resolve declaration specifiers into a semantic base type.
fn resolve_base_type(
    cx: &mut SemaContext<'_>,
    specifiers: &DeclSpec,
    span: SourceSpan,
    allow_outer_tag_reference: bool,
) -> TypeId {
    match try_build_base_type(cx, specifiers, span, allow_outer_tag_reference) {
        Ok(ty) => ty,
        Err(diag) => {
            cx.emit(diag);
            cx.error_type()
        }
    }
}

/// Normalize parameter types according to C decay rules.
///
/// Arrays decay to pointers, functions decay to function pointers.
pub(crate) fn normalize_function_parameter_type(
    cx: &mut SemaContext<'_>,
    param_ty: TypeId,
) -> TypeId {
    let ty = cx.types.get(param_ty);
    match &ty.kind {
        TypeKind::Array { elem, .. } => cx.types.intern(Type {
            kind: TypeKind::Pointer { pointee: *elem },
            quals: ty.quals,
        }),
        TypeKind::Function(_) => cx.types.intern(Type {
            kind: TypeKind::Pointer { pointee: param_ty },
            quals: Qualifiers::default(),
        }),
        _ => param_ty,
    }
}

fn is_void_type(cx: &SemaContext<'_>, ty: TypeId) -> bool {
    matches!(cx.types.get(ty).kind, TypeKind::Void)
}

/// Convert array-size syntax node into semantic array length.
fn array_len_from_size_expr(
    cx: &mut SemaContext<'_>,
    size: &ArraySize,
    span: SourceSpan,
) -> ArrayLen {
    match size {
        ArraySize::Unspecified => ArrayLen::Incomplete,
        ArraySize::Variable => {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::UnsupportedVmType,
                "variable-length arrays are not supported in V1",
                span,
            ));
            ArrayLen::Incomplete
        }
        ArraySize::Expr(expr) => match evaluate_integer_constant_expr(cx, expr) {
            Ok(n) if n > 0 => ArrayLen::Known(n as u64),
            Ok(_) => {
                cx.emit(SemaDiagnostic::new(
                    SemaDiagnosticCode::NonConstantInRequiredContext,
                    "array size must be a positive integer",
                    span,
                ));
                ArrayLen::Incomplete
            }
            Err(err) => {
                emit_ice_eval_error(cx, err, "array size is not a constant expression");
                ArrayLen::Incomplete
            }
        },
    }
}

/// Build builtin/typedef/record/enum base type from declaration specifiers.
///
/// Uses a counter-based approach: count occurrences of each type keyword,
/// then match against the set of legal C99 combinations (C99 6.7.2).
fn try_build_base_type(
    cx: &mut SemaContext<'_>,
    specifiers: &DeclSpec,
    span: SourceSpan,
    allow_outer_tag_reference: bool,
) -> Result<TypeId, SemaDiagnostic> {
    use crate::frontend::parser::ast::TypeSpecifierKind as TSK;

    // Separate structured specifiers (struct/union/enum/typedef) from keywords.
    let mut structured: Option<&TypeSpecifier> = None;
    let mut void_count = 0u8;
    let mut char_count = 0u8;
    let mut short_count = 0u8;
    let mut int_count = 0u8;
    let mut long_count = 0u8;
    let mut float_count = 0u8;
    let mut double_count = 0u8;
    let mut signed_count = 0u8;
    let mut unsigned_count = 0u8;
    let mut bool_count = 0u8;

    for spec in &specifiers.ty {
        match &spec.kind {
            TSK::StructOrUnion(_) | TSK::Enum(_) | TSK::TypedefName(_) => {
                if structured.is_some() {
                    return Err(SemaDiagnostic::new(
                        SemaDiagnosticCode::TypeMismatch,
                        "multiple struct/union/enum/typedef specifiers",
                        spec.span,
                    ));
                }
                structured = Some(spec);
            }
            TSK::Void => void_count += 1,
            TSK::Char => char_count += 1,
            TSK::Short => short_count += 1,
            TSK::Int => int_count += 1,
            TSK::Long => long_count += 1,
            TSK::Float => float_count += 1,
            TSK::Double => double_count += 1,
            TSK::Signed => signed_count += 1,
            TSK::Unsigned => unsigned_count += 1,
            TSK::Bool => bool_count += 1,
        }
    }

    let keyword_total = void_count
        + char_count
        + short_count
        + int_count
        + long_count
        + float_count
        + double_count
        + signed_count
        + unsigned_count
        + bool_count;

    // Structured specifier must appear alone (no keyword mix).
    if let Some(s) = structured {
        if keyword_total > 0 {
            return Err(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "type specifier keywords cannot combine with struct/union/enum/typedef",
                span,
            ));
        }
        let base_ty = match &s.kind {
            TSK::StructOrUnion(rec) => {
                resolve_record_specifier_type(cx, rec, s.span, allow_outer_tag_reference)?
            }
            TSK::Enum(en) => resolve_enum_specifier_type(cx, en, s.span)?,
            TSK::TypedefName(name) => resolve_typedef_name_type(cx, name, s.span)?,
            _ => unreachable!(),
        };
        // Apply top-level qualifiers.
        let quals = qualifiers_from_slice(&specifiers.qualifiers);
        return Ok(apply_type_qualifiers(cx, base_ty, quals));
    }

    // Empty specifier list: C99 forbids implicit int.
    if keyword_total == 0 {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::TypeMismatch,
            "missing type specifier (implicit int is not allowed in C99)",
            span,
        ));
    }

    // Reject duplicates of non-combinable keywords.
    if void_count > 1
        || char_count > 1
        || short_count > 1
        || int_count > 1
        || float_count > 1
        || double_count > 1
        || signed_count > 1
        || unsigned_count > 1
        || bool_count > 1
    {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::TypeMismatch,
            "duplicate type specifier",
            span,
        ));
    }
    // long can appear at most twice (long long).
    if long_count > 2 {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::TypeMismatch,
            "too many 'long' specifiers",
            span,
        ));
    }

    // signed/unsigned are mutually exclusive.
    if signed_count > 0 && unsigned_count > 0 {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::TypeMismatch,
            "'signed' and 'unsigned' cannot be combined",
            span,
        ));
    }
    let is_unsigned = unsigned_count > 0;

    // Match legal combinations (C99 6.7.2).
    let kind = match (
        void_count,
        bool_count,
        char_count,
        short_count,
        int_count,
        long_count,
        float_count,
        double_count,
    ) {
        // void
        (1, 0, 0, 0, 0, 0, 0, 0) if !is_unsigned && signed_count == 0 => TypeKind::Void,
        // _Bool
        (0, 1, 0, 0, 0, 0, 0, 0) if !is_unsigned && signed_count == 0 => TypeKind::Bool,
        // char, signed char, unsigned char
        (0, 0, 1, 0, 0, 0, 0, 0) if signed_count == 0 && !is_unsigned => TypeKind::Char,
        (0, 0, 1, 0, 0, 0, 0, 0) if signed_count > 0 => TypeKind::SignedChar,
        (0, 0, 1, 0, 0, 0, 0, 0) if is_unsigned => TypeKind::UnsignedChar,
        // short, short int, signed short, signed short int, unsigned short, unsigned short int
        (0, 0, 0, 1, i, 0, 0, 0) if i <= 1 => TypeKind::Short {
            signed: !is_unsigned,
        },
        // int, signed, signed int, unsigned, unsigned int
        (0, 0, 0, 0, i, 0, 0, 0) if i <= 1 && (i > 0 || signed_count > 0 || is_unsigned) => {
            TypeKind::Int {
                signed: !is_unsigned,
            }
        }
        // long, long int, signed long, signed long int, unsigned long, unsigned long int
        (0, 0, 0, 0, i, 1, 0, 0) if i <= 1 => TypeKind::Long {
            signed: !is_unsigned,
        },
        // long long, long long int, signed long long, etc.
        (0, 0, 0, 0, i, 2, 0, 0) if i <= 1 => TypeKind::LongLong {
            signed: !is_unsigned,
        },
        // float
        (0, 0, 0, 0, 0, 0, 1, 0) if signed_count == 0 && !is_unsigned => TypeKind::Float,
        // double
        (0, 0, 0, 0, 0, 0, 0, 1) if signed_count == 0 && !is_unsigned && long_count == 0 => {
            TypeKind::Double
        }
        // long double — V1 不支持
        (0, 0, 0, 0, 0, 1, 0, 1) if signed_count == 0 && !is_unsigned => {
            return Err(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "'long double' is not supported in V1",
                span,
            ));
        }
        _ => {
            return Err(SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                "invalid combination of type specifiers",
                span,
            ));
        }
    };

    let quals = qualifiers_from_slice(&specifiers.qualifiers);
    Ok(cx.types.intern(Type { kind, quals }))
}

/// Evaluate an integer constant expression for contexts like enum/array length.
///
/// Supports: integer literals, enum constant references, unary +/-/~/!,
/// and binary arithmetic/bitwise/relational/logical operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IceEvalErrorKind {
    NonConstant,
    DivisionByZero,
    SignedOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IceEvalError {
    kind: IceEvalErrorKind,
    span: SourceSpan,
}

impl IceEvalError {
    fn non_constant(span: SourceSpan) -> Self {
        Self {
            kind: IceEvalErrorKind::NonConstant,
            span,
        }
    }

    fn division_by_zero(span: SourceSpan) -> Self {
        Self {
            kind: IceEvalErrorKind::DivisionByZero,
            span,
        }
    }

    fn signed_overflow(span: SourceSpan) -> Self {
        Self {
            kind: IceEvalErrorKind::SignedOverflow,
            span,
        }
    }
}

fn emit_ice_eval_error(cx: &mut SemaContext<'_>, err: IceEvalError, non_constant_message: &str) {
    match err.kind {
        IceEvalErrorKind::NonConstant => {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::NonConstantInRequiredContext,
                non_constant_message,
                err.span,
            ));
        }
        IceEvalErrorKind::DivisionByZero => {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::ConstantDivisionByZero,
                "division by zero in constant expression",
                err.span,
            ));
        }
        IceEvalErrorKind::SignedOverflow => {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::ConstantSignedOverflow,
                "signed integer overflow in constant expression",
                err.span,
            ));
        }
    }
}

/// Evaluate an integer constant expression and emit diagnostics on failure.
pub(crate) fn evaluate_required_integer_constant_expr(
    cx: &mut SemaContext<'_>,
    expr: &crate::frontend::parser::ast::Expr,
    non_constant_message: &str,
) -> Option<i64> {
    match evaluate_integer_constant_expr(cx, expr) {
        Ok(v) => Some(v),
        Err(err) => {
            emit_ice_eval_error(cx, err, non_constant_message);
            None
        }
    }
}

fn evaluate_integer_constant_expr(
    cx: &mut SemaContext<'_>,
    expr: &crate::frontend::parser::ast::Expr,
) -> Result<i64, IceEvalError> {
    use crate::frontend::parser::ast::{BinaryOp, ExprKind, IntLiteralSuffix, Literal, UnaryOp};

    match &expr.kind {
        ExprKind::Literal(Literal::Int { value, base }) => match base {
            IntLiteralSuffix::Int | IntLiteralSuffix::Long | IntLiteralSuffix::LongLong => {
                i64::try_from(*value).map_err(|_| IceEvalError::signed_overflow(expr.span))
            }
            IntLiteralSuffix::UInt | IntLiteralSuffix::ULong | IntLiteralSuffix::ULongLong => {
                Ok(*value as i64)
            }
        },
        ExprKind::Literal(Literal::Char(c)) => Ok(*c as i64),
        ExprKind::Var(name) => {
            // Look up enum constants by name.
            let sym_id = cx
                .resolve_ordinary(name, expr.span)
                .ok_or_else(|| IceEvalError::non_constant(expr.span))?;
            let sym = cx.symbol(sym_id);
            if sym.kind() != SymbolKind::EnumConst {
                return Err(IceEvalError::non_constant(expr.span));
            }
            cx.lookup_enum_const_value(sym_id)
                .ok_or_else(|| IceEvalError::non_constant(expr.span))
        }
        ExprKind::Unary { op, expr: inner } => {
            let val = evaluate_integer_constant_expr(cx, inner)?;
            Ok(match op {
                UnaryOp::Plus => val,
                UnaryOp::Minus => val
                    .checked_neg()
                    .ok_or_else(|| IceEvalError::signed_overflow(expr.span))?,
                UnaryOp::BitNot => !val,
                UnaryOp::LogicalNot => i64::from(val == 0),
                _ => return Err(IceEvalError::non_constant(expr.span)),
            })
        }
        ExprKind::Binary { left, op, right } => {
            // Preserve C short-circuit behavior for logical operators.
            if matches!(op, BinaryOp::LogicalAnd) {
                let lhs = evaluate_integer_constant_expr(cx, left)?;
                if lhs == 0 {
                    return Ok(0);
                }
                let rhs = evaluate_integer_constant_expr(cx, right)?;
                return Ok(i64::from(rhs != 0));
            }
            if matches!(op, BinaryOp::LogicalOr) {
                let lhs = evaluate_integer_constant_expr(cx, left)?;
                if lhs != 0 {
                    return Ok(1);
                }
                let rhs = evaluate_integer_constant_expr(cx, right)?;
                return Ok(i64::from(rhs != 0));
            }

            let lhs = evaluate_integer_constant_expr(cx, left)?;
            let rhs = evaluate_integer_constant_expr(cx, right)?;
            Ok(match op {
                BinaryOp::Add => lhs
                    .checked_add(rhs)
                    .ok_or_else(|| IceEvalError::signed_overflow(expr.span))?,
                BinaryOp::Sub => lhs
                    .checked_sub(rhs)
                    .ok_or_else(|| IceEvalError::signed_overflow(expr.span))?,
                BinaryOp::Mul => lhs
                    .checked_mul(rhs)
                    .ok_or_else(|| IceEvalError::signed_overflow(expr.span))?,
                BinaryOp::Div => {
                    if rhs == 0 {
                        return Err(IceEvalError::division_by_zero(right.span));
                    }
                    lhs.checked_div(rhs)
                        .ok_or_else(|| IceEvalError::signed_overflow(expr.span))?
                }
                BinaryOp::Mod => {
                    if rhs == 0 {
                        return Err(IceEvalError::division_by_zero(right.span));
                    }
                    lhs.checked_rem(rhs)
                        .ok_or_else(|| IceEvalError::signed_overflow(expr.span))?
                }
                BinaryOp::Shl => {
                    if !(0..64).contains(&rhs) {
                        return Err(IceEvalError::signed_overflow(expr.span));
                    }
                    let shifted = (lhs as i128) << (rhs as u32);
                    if shifted < i64::MIN as i128 || shifted > i64::MAX as i128 {
                        return Err(IceEvalError::signed_overflow(expr.span));
                    }
                    shifted as i64
                }
                BinaryOp::Shr => {
                    if !(0..64).contains(&rhs) {
                        return Err(IceEvalError::signed_overflow(expr.span));
                    }
                    lhs >> (rhs as u32)
                }
                BinaryOp::BitAnd => lhs & rhs,
                BinaryOp::BitOr => lhs | rhs,
                BinaryOp::BitXor => lhs ^ rhs,
                BinaryOp::Lt => i64::from(lhs < rhs),
                BinaryOp::Le => i64::from(lhs <= rhs),
                BinaryOp::Gt => i64::from(lhs > rhs),
                BinaryOp::Ge => i64::from(lhs >= rhs),
                BinaryOp::Eq => i64::from(lhs == rhs),
                BinaryOp::Ne => i64::from(lhs != rhs),
                BinaryOp::LogicalAnd | BinaryOp::LogicalOr => unreachable!(),
            })
        }
        ExprKind::Conditional {
            cond,
            then_expr,
            else_expr,
        } => {
            let c = evaluate_integer_constant_expr(cx, cond)?;
            if c != 0 {
                evaluate_integer_constant_expr(cx, then_expr)
            } else {
                evaluate_integer_constant_expr(cx, else_expr)
            }
        }
        ExprKind::Cast { ty, expr: inner } => {
            let value = evaluate_integer_constant_expr(cx, inner)?;
            let cast_ty = build_type_from_type_name(cx, ty, expr.span);
            if is_integer_ice_type(cx, cast_ty) {
                Ok(cast_ice_integer_value(cx, value, cast_ty))
            } else {
                Err(IceEvalError::non_constant(expr.span))
            }
        }
        ExprKind::SizeofType(ty_name) => {
            let ty = build_type_from_type_name(cx, ty_name, expr.span);
            let size = type_size_of(ty, &cx.types, &cx.records)
                .ok_or_else(|| IceEvalError::non_constant(expr.span))?;
            i64::try_from(size).map_err(|_| IceEvalError::signed_overflow(expr.span))
        }
        ExprKind::SizeofExpr(inner) => {
            // sizeof(expr) requires expression type inference.
            let _ = inner;
            Err(IceEvalError::non_constant(expr.span))
        }
        _ => Err(IceEvalError::non_constant(expr.span)),
    }
}

pub(crate) fn cast_ice_integer_value(cx: &SemaContext<'_>, value: i64, ty: TypeId) -> i64 {
    match &cx.types.get(ty).kind {
        TypeKind::Bool => i64::from(value != 0),
        TypeKind::Char | TypeKind::SignedChar => (value as i8) as i64,
        TypeKind::UnsignedChar => (value as u8) as i64,
        TypeKind::Short { signed: true } => (value as i16) as i64,
        TypeKind::Short { signed: false } => (value as u16) as i64,
        TypeKind::Int { signed: true } | TypeKind::Enum(_) => (value as i32) as i64,
        TypeKind::Int { signed: false } => (value as u32) as i64,
        TypeKind::Long { signed: true } | TypeKind::LongLong { signed: true } => value,
        TypeKind::Long { signed: false } | TypeKind::LongLong { signed: false } => {
            (value as u64) as i64
        }
        _ => value,
    }
}

/// Resolve a typedef name into its target semantic type.
fn resolve_typedef_name_type(
    cx: &mut SemaContext<'_>,
    name: &str,
    span: SourceSpan,
) -> Result<TypeId, SemaDiagnostic> {
    let Some(sym_id) = cx.lookup_ordinary(name) else {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::UndefinedSymbol,
            format!("unknown type name '{name}'"),
            span,
        ));
    };
    let symbol = cx.symbol(sym_id);
    if symbol.kind() != SymbolKind::Typedef {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::TypeMismatch,
            format!("'{name}' is not a typedef name"),
            span,
        ));
    }
    Ok(symbol.ty())
}

/// Return the keyword string for a record kind.
fn record_kind_str(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Struct => "struct",
        RecordKind::Union => "union",
    }
}

/// Resolve a `struct`/`union` specifier into semantic record type.
///
/// Three cases:
/// 1. Forward declaration: `struct S;` — create incomplete record, register tag.
/// 2. Definition: `struct S { ... }` — create complete record with fields, register tag.
/// 3. Reference: `struct S` (no body, tag already exists) — look up existing tag.
fn resolve_record_specifier_type(
    cx: &mut SemaContext<'_>,
    record_spec: &RecordSpecifier,
    span: SourceSpan,
    allow_outer_tag_reference: bool,
) -> Result<TypeId, SemaDiagnostic> {
    let has_body = record_spec.members.is_some();

    // Check current scope first to handle shadowing correctly.
    if let Some(tag_name) = &record_spec.tag {
        if let Some(existing_tag) = cx.lookup_tag_in_current_scope(tag_name) {
            // Found in current scope - try to merge or report conflict.
            match existing_tag {
                TagId::Record(record_id) => {
                    let existing_rec = cx.records.get(record_id);
                    // Check struct vs union consistency.
                    if existing_rec.kind != record_spec.kind {
                        return Err(SemaDiagnostic::new(
                            SemaDiagnosticCode::RedeclarationConflict,
                            format!(
                                "'{tag_name}' defined as a {} but was previously declared as a {}",
                                record_kind_str(record_spec.kind),
                                record_kind_str(existing_rec.kind),
                            ),
                            span,
                        ));
                    }
                    if has_body {
                        if cx.records.get(record_id).is_complete {
                            return Err(SemaDiagnostic::new(
                                SemaDiagnosticCode::RedeclarationConflict,
                                format!(
                                    "redefinition of '{} {tag_name}'",
                                    record_kind_str(record_spec.kind),
                                ),
                                span,
                            ));
                        }
                        // Complete the existing forward declaration.
                        let fields = lower_record_fields(
                            cx,
                            record_spec.members.as_deref().unwrap(),
                            record_spec.kind,
                            Some(record_id),
                            span,
                        );
                        let rec = cx.records.get_mut(record_id);
                        rec.fields = fields;
                        rec.is_complete = true;
                    }
                    return Ok(cx.types.intern(Type {
                        kind: TypeKind::Record(record_id),
                        quals: Qualifiers::default(),
                    }));
                }
                TagId::Enum(_) => {
                    return Err(SemaDiagnostic::new(
                        SemaDiagnosticCode::RedeclarationConflict,
                        format!("'{tag_name}' previously declared as enum"),
                        span,
                    ));
                }
            }
        } else if !has_body && allow_outer_tag_reference {
            // No body (reference or forward declaration) — look up in outer scopes.
            if let Some(existing_tag) = cx.lookup_tag(tag_name) {
                match existing_tag {
                    TagId::Record(record_id) => {
                        return Ok(cx.types.intern(Type {
                            kind: TypeKind::Record(record_id),
                            quals: Qualifiers::default(),
                        }));
                    }
                    TagId::Enum(_) => {
                        return Err(SemaDiagnostic::new(
                            SemaDiagnosticCode::RedeclarationConflict,
                            format!("'{tag_name}' previously declared as enum"),
                            span,
                        ));
                    }
                }
            }
        }
    }

    // For a tagged definition, pre-register an incomplete record so fields can
    // reference the same tag during member type construction.
    if has_body && let Some(tag_name) = &record_spec.tag {
        let record_id = cx.records.insert(RecordDef {
            tag: record_spec.tag.clone(),
            kind: record_spec.kind,
            fields: Vec::new(),
            is_complete: false,
        });
        register_tag(cx, tag_name, TagId::Record(record_id), span)?;

        let fields = lower_record_fields(
            cx,
            record_spec.members.as_deref().unwrap(),
            record_spec.kind,
            Some(record_id),
            span,
        );
        let rec = cx.records.get_mut(record_id);
        rec.fields = fields;
        rec.is_complete = true;

        return Ok(cx.types.intern(Type {
            kind: TypeKind::Record(record_id),
            quals: Qualifiers::default(),
        }));
    }

    // Create new record (either definition or forward declaration in current scope).
    let fields = if has_body {
        lower_record_fields(
            cx,
            record_spec.members.as_deref().unwrap(),
            record_spec.kind,
            None,
            span,
        )
    } else {
        Vec::new()
    };

    let record_id = cx.records.insert(RecordDef {
        tag: record_spec.tag.clone(),
        kind: record_spec.kind,
        fields,
        is_complete: has_body,
    });

    if let Some(tag_name) = &record_spec.tag {
        register_tag(cx, tag_name, TagId::Record(record_id), span)?;
    }

    Ok(cx.types.intern(Type {
        kind: TypeKind::Record(record_id),
        quals: Qualifiers::default(),
    }))
}

/// Resolve an `enum` specifier into semantic enum type.
///
/// Two cases:
/// 1. Definition: `enum E { A, B }` — create enum with constants, register tag.
/// 2. Reference: `enum E` (no body, tag exists) — look up existing tag.
/// Forward reference without body is an error (C99 forbids incomplete enum types).
fn resolve_enum_specifier_type(
    cx: &mut SemaContext<'_>,
    enum_spec: &EnumSpecifier,
    span: SourceSpan,
) -> Result<TypeId, SemaDiagnostic> {
    let has_body = enum_spec.variants.is_some();

    // Check current scope first to handle shadowing correctly.
    if let Some(tag_name) = &enum_spec.tag {
        if let Some(existing_tag) = cx.lookup_tag_in_current_scope(tag_name) {
            // Found in current scope - try to merge or report conflict.
            match existing_tag {
                TagId::Enum(enum_id) => {
                    if has_body {
                        let existing = cx.enums.get(enum_id);
                        if !existing.constants.is_empty() {
                            return Err(SemaDiagnostic::new(
                                SemaDiagnosticCode::RedeclarationConflict,
                                format!("redefinition of 'enum {tag_name}'"),
                                span,
                            ));
                        }
                        let underlying_ty = int_type(cx);
                        let constants = lower_enum_variants(
                            cx,
                            enum_spec.variants.as_deref().unwrap(),
                            underlying_ty,
                        );
                        cx.enums.get_mut(enum_id).constants = constants;
                    }
                    return Ok(cx.types.intern(Type {
                        kind: TypeKind::Enum(enum_id),
                        quals: Qualifiers::default(),
                    }));
                }
                TagId::Record(_) => {
                    return Err(SemaDiagnostic::new(
                        SemaDiagnosticCode::RedeclarationConflict,
                        format!("'{tag_name}' previously declared as struct/union"),
                        span,
                    ));
                }
            }
        } else if !has_body {
            // No body (reference) - look up in outer scopes.
            if let Some(existing_tag) = cx.lookup_tag(tag_name) {
                match existing_tag {
                    TagId::Enum(enum_id) => {
                        return Ok(cx.types.intern(Type {
                            kind: TypeKind::Enum(enum_id),
                            quals: Qualifiers::default(),
                        }));
                    }
                    TagId::Record(_) => {
                        return Err(SemaDiagnostic::new(
                            SemaDiagnosticCode::RedeclarationConflict,
                            format!("'{tag_name}' previously declared as struct/union"),
                            span,
                        ));
                    }
                }
            }
            // Forward reference without definition is an error.
            return Err(SemaDiagnostic::new(
                SemaDiagnosticCode::IncompleteType,
                format!("use of undefined enum '{tag_name}'"),
                span,
            ));
        }
    }

    let underlying_ty = int_type(cx);
    let constants = if has_body {
        lower_enum_variants(cx, enum_spec.variants.as_deref().unwrap(), underlying_ty)
    } else {
        Vec::new()
    };

    let enum_id = cx.enums.insert(EnumDef {
        tag: enum_spec.tag.clone(),
        underlying_ty,
        constants,
    });

    if let Some(tag_name) = &enum_spec.tag {
        register_tag(cx, tag_name, TagId::Enum(enum_id), span)?;
    }

    Ok(cx.types.intern(Type {
        kind: TypeKind::Enum(enum_id),
        quals: Qualifiers::default(),
    }))
}

/// Lower record member declarations into field definitions.
fn lower_record_fields(
    cx: &mut SemaContext<'_>,
    members: &[RecordMemberDecl],
    record_kind: RecordKind,
    self_record_id: Option<crate::frontend::sema::types::RecordId>,
    record_span: SourceSpan,
) -> Vec<FieldDef> {
    let mut fields_with_span: Vec<(FieldDef, SourceSpan)> = Vec::new();
    let mut seen_named_members: std::collections::HashMap<String, SourceSpan> =
        std::collections::HashMap::new();

    for member in members {
        let base_ty = resolve_base_type(cx, &member.specifiers, member.span, true);
        if member.declarators.is_empty() {
            // Anonymous member (e.g. unnamed struct/union).
            fields_with_span.push((
                FieldDef {
                    name: None,
                    ty: base_ty,
                    bit_width: None,
                },
                member.span,
            ));
        } else {
            for declarator in &member.declarators {
                let name = declarator_ident(declarator).map(|s| s.to_string());
                let mut ty = apply_declarator_with_base(cx, base_ty, declarator);
                let field_span = declarator.direct.span;

                if let Some(member_name) = name.as_deref() {
                    if let Some(previous_span) = seen_named_members.get(member_name).copied() {
                        cx.emit(
                            SemaDiagnostic::new(
                                SemaDiagnosticCode::RedeclarationConflict,
                                format!("duplicate member name '{member_name}'"),
                                field_span,
                            )
                            .with_secondary(previous_span, "previous member declaration is here"),
                        );
                        ty = cx.error_type();
                    } else {
                        seen_named_members.insert(member_name.to_string(), field_span);
                    }
                }

                fields_with_span.push((
                    FieldDef {
                        name,
                        ty,
                        bit_width: None,
                    },
                    field_span,
                ));
            }
        }
    }

    if fields_with_span.is_empty() {
        cx.emit(
            SemaDiagnostic::new(
                SemaDiagnosticCode::TypeMismatch,
                format!(
                    "empty {} definition is not supported in V1",
                    record_kind_str(record_kind)
                ),
                record_span,
            )
            .with_note("add at least one member declaration"),
        );
        return Vec::new();
    }

    for idx in 0..fields_with_span.len() {
        let is_last = idx + 1 == fields_with_span.len();
        let named_before = fields_with_span[..idx]
            .iter()
            .filter(|(field, _)| field.name.is_some())
            .count();

        let (field, field_span) = &mut fields_with_span[idx];
        if matches!(cx.types.get(field.ty).kind, TypeKind::Error) {
            continue;
        }

        let has_incomplete_array_ty = matches!(
            cx.types.get(field.ty).kind,
            TypeKind::Array {
                len: ArrayLen::Incomplete,
                ..
            }
        );
        if has_incomplete_array_ty {
            if record_kind == RecordKind::Struct && is_last {
                let mut valid_flexible_member = true;
                if field.name.is_none() {
                    cx.emit(
                        SemaDiagnostic::new(
                            SemaDiagnosticCode::TypeMismatch,
                            "flexible array member must be named",
                            *field_span,
                        )
                        .with_note("declare the flexible array member with an identifier"),
                    );
                    valid_flexible_member = false;
                }
                if named_before == 0 {
                    cx.emit(
                        SemaDiagnostic::new(
                            SemaDiagnosticCode::TypeMismatch,
                            "flexible array member requires at least one named member before it",
                            *field_span,
                        )
                        .with_note("add a regular named member before the flexible array member"),
                    );
                    valid_flexible_member = false;
                }

                if valid_flexible_member {
                    field.ty = mark_flexible_array_member_type(cx, field.ty);
                    continue;
                }
                field.ty = cx.error_type();
                continue;
            }

            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::IncompleteType,
                    format!(
                        "{} member has incomplete array type",
                        record_kind_str(record_kind)
                    ),
                    *field_span,
                )
                .with_note("only the last member of a struct can be a flexible array member"),
            );
            field.ty = cx.error_type();
            continue;
        }

        if matches!(cx.types.get(field.ty).kind, TypeKind::Function(_)) {
            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    format!(
                        "{} member cannot have function type",
                        record_kind_str(record_kind)
                    ),
                    *field_span,
                )
                .with_note("use a pointer to function instead"),
            );
            field.ty = cx.error_type();
            continue;
        }

        if !is_complete_type(field.ty, cx) {
            // This catches by-value self-recursion because the placeholder record
            // is still incomplete while its members are being lowered.
            let mut diag = SemaDiagnostic::new(
                SemaDiagnosticCode::IncompleteType,
                format!(
                    "{} member has incomplete type",
                    record_kind_str(record_kind)
                ),
                *field_span,
            );
            if let Some(self_id) = self_record_id
                && contains_direct_record(field.ty, self_id, cx)
            {
                diag = diag.with_note("self-referential members must use pointers");
            }
            cx.emit(diag);
            field.ty = cx.error_type();
        }
    }

    fields_with_span
        .into_iter()
        .map(|(field, _)| field)
        .collect()
}

/// Lower enum variants into semantic constants and symbol table entries.
///
/// Each enumerator is registered as an `EnumConst` symbol in the ordinary namespace.
/// Values are assigned sequentially (previous + 1) unless an explicit value is given.
fn lower_enum_variants(
    cx: &mut SemaContext<'_>,
    variants: &[crate::frontend::parser::ast::EnumVariant],
    underlying_ty: TypeId,
) -> Vec<EnumConstant> {
    let mut constants = Vec::new();
    let mut next_value: i64 = 0;

    for variant in variants {
        let value = if let Some(expr) = &variant.value {
            match evaluate_integer_constant_expr(cx, expr) {
                Ok(v) => Some(v),
                Err(err) => {
                    emit_ice_eval_error(
                        cx,
                        err,
                        "enumerator value is not an integer constant expression",
                    );
                    None
                }
            }
        } else {
            Some(next_value)
        };

        let Some(value) = value else {
            // Keep sequence state unchanged for invalid explicit enumerator values.
            continue;
        };

        if !enum_value_representable_as_int(value) {
            cx.emit(
                SemaDiagnostic::new(
                    SemaDiagnosticCode::TypeMismatch,
                    format!(
                        "enumerator '{}' value {value} is not representable as int",
                        variant.name
                    ),
                    variant.span,
                )
                .with_note("V1 models enum underlying type as 'int'"),
            );
            next_value = advance_enum_value(cx, value, variant.span);
            continue;
        }

        // Check for duplicate enumerator names.
        if cx.lookup_ordinary_in_current_scope(&variant.name).is_some() {
            cx.emit(SemaDiagnostic::new(
                SemaDiagnosticCode::RedeclarationConflict,
                format!("redefinition of enumerator '{}'", variant.name),
                variant.span,
            ));
            next_value = advance_enum_value(cx, value, variant.span);
            continue;
        }

        constants.push(EnumConstant {
            name: variant.name.clone(),
            value,
        });

        // Register as symbol in ordinary namespace.
        let sym = Symbol::new(
            variant.name.clone(),
            SymbolKind::EnumConst,
            underlying_ty,
            Linkage::None,
            DefinitionStatus::Defined,
            variant.span,
        );
        let sym_id = cx.insert_symbol(sym);
        cx.set_enum_const_value(sym_id, value);
        let _ = cx.insert_ordinary(variant.name.clone(), sym_id);

        next_value = advance_enum_value(cx, value, variant.span);
    }

    constants
}

fn advance_enum_value(cx: &mut SemaContext<'_>, current: i64, span: SourceSpan) -> i64 {
    current.checked_add(1).unwrap_or_else(|| {
        cx.emit(SemaDiagnostic::new(
            SemaDiagnosticCode::ConstantSignedOverflow,
            "signed integer overflow in enum value computation",
            span,
        ));
        current
    })
}

fn enum_value_representable_as_int(value: i64) -> bool {
    (i32::MIN as i64..=i32::MAX as i64).contains(&value)
}

fn mark_flexible_array_member_type(cx: &mut SemaContext<'_>, ty: TypeId) -> TypeId {
    let t = cx.types.get(ty);
    let elem = match &t.kind {
        TypeKind::Array {
            elem,
            len: ArrayLen::Incomplete,
        } => *elem,
        _ => return ty,
    };

    cx.types.intern(Type {
        kind: TypeKind::Array {
            elem,
            len: ArrayLen::FlexibleMember,
        },
        quals: t.quals,
    })
}

fn contains_direct_record(
    ty: TypeId,
    target_record: crate::frontend::sema::types::RecordId,
    cx: &SemaContext<'_>,
) -> bool {
    match cx.types.get(ty).kind {
        TypeKind::Record(record_id) => record_id == target_record,
        TypeKind::Array { elem, .. } => contains_direct_record(elem, target_record, cx),
        _ => false,
    }
}

/// Register one tag (`struct`/`union`/`enum`) in current scope.
fn register_tag(
    cx: &mut SemaContext<'_>,
    tag: &str,
    tag_id: TagId,
    span: SourceSpan,
) -> Result<(), SemaDiagnostic> {
    if let Err((name, existing)) = cx.insert_tag(tag.to_string(), tag_id) {
        // Same tag kind re-entering the same scope is allowed for forward decl + definition.
        // Different tag kinds in the same scope conflict.
        let conflicts = match (existing, tag_id) {
            (TagId::Record(a), TagId::Record(b)) => a != b,
            (TagId::Enum(a), TagId::Enum(b)) => a != b,
            _ => true,
        };
        if conflicts {
            return Err(SemaDiagnostic::new(
                SemaDiagnosticCode::RedeclarationConflict,
                format!("tag '{name}' conflicts with a previous declaration"),
                span,
            ));
        }
    }
    Ok(())
}

/// Return canonical semantic `int` type.
fn int_type(cx: &mut SemaContext<'_>) -> TypeId {
    cx.types.intern(Type {
        kind: TypeKind::Int { signed: true },
        quals: Qualifiers::default(),
    })
}

/// Build semantic type from parser `TypeName` node.
pub(crate) fn build_type_from_type_name(
    cx: &mut SemaContext<'_>,
    ty_name: &TypeName,
    span: SourceSpan,
) -> TypeId {
    let base_ty = resolve_base_type(cx, &ty_name.specifiers, span, true);
    if let Some(declarator) = &ty_name.declarator {
        apply_declarator_with_base(cx, base_ty, declarator)
    } else {
        base_ty
    }
}

/// Returns whether a type is valid as the result type of an integer constant expression.
fn is_integer_ice_type(cx: &SemaContext<'_>, ty: TypeId) -> bool {
    matches!(
        cx.types.get(ty).kind,
        TypeKind::Bool
            | TypeKind::Char
            | TypeKind::SignedChar
            | TypeKind::UnsignedChar
            | TypeKind::Short { .. }
            | TypeKind::Int { .. }
            | TypeKind::Long { .. }
            | TypeKind::LongLong { .. }
            | TypeKind::Enum(_)
    )
}

/// Apply top-level qualifiers to an existing semantic type.
fn apply_type_qualifiers(cx: &mut SemaContext<'_>, ty: TypeId, quals: Qualifiers) -> TypeId {
    if quals == Qualifiers::default() {
        return ty;
    }
    let mut qualified = cx.types.get(ty).clone();
    qualified.quals = merge_qualifiers(qualified.quals, quals);
    cx.types.intern(qualified)
}

/// Merge qualifier sets with OR semantics.
fn merge_qualifiers(lhs: Qualifiers, rhs: Qualifiers) -> Qualifiers {
    Qualifiers {
        is_const: lhs.is_const || rhs.is_const,
        is_volatile: lhs.is_volatile || rhs.is_volatile,
        is_restrict: lhs.is_restrict || rhs.is_restrict,
    }
}

/// Check if a type is complete under current semantic context.
fn is_complete_type(ty: TypeId, cx: &SemaContext<'_>) -> bool {
    let t = cx.types.get(ty);
    match &t.kind {
        TypeKind::Void => false,
        TypeKind::Array { elem, len } => {
            !matches!(len, ArrayLen::Incomplete | ArrayLen::FlexibleMember)
                && is_complete_type(*elem, cx)
        }
        TypeKind::Record(record_id) => cx.records.get(*record_id).is_complete,
        TypeKind::Function(_) => false,
        TypeKind::Error => true, // Error type is considered complete for recovery.
        _ => true,
    }
}

/// Convert AST qualifiers into semantic qualifier flags.
fn qualifiers_from_slice(qualifiers: &[TypeQualifier]) -> Qualifiers {
    let mut out = Qualifiers::default();
    for qualifier in qualifiers {
        match qualifier {
            TypeQualifier::Const => out.is_const = true,
            TypeQualifier::Volatile => out.is_volatile = true,
            TypeQualifier::Restrict => out.is_restrict = true,
        }
    }
    out
}

/// Compute declaration span from first specifier to last declarator.
fn declaration_span(decl: &Declaration) -> SourceSpan {
    let first_spec_span = decl.specifiers.ty.first().map(|spec| spec.span);
    let first_decl_span = decl
        .declarators
        .first()
        .map(|item| item.declarator.direct.span);
    let last_decl_span = decl
        .declarators
        .last()
        .map(|item| item.declarator.direct.span);

    match (first_spec_span.or(first_decl_span), last_decl_span) {
        (Some(start), Some(end)) => start.join(end),
        (Some(span), None) | (None, Some(span)) => span,
        (None, None) => SourceSpan::new(0, 0),
    }
}

fn has_prior_initializer_diagnostic(cx: &SemaContext<'_>, span: SourceSpan) -> bool {
    cx.diagnostics()
        .iter()
        .any(|diag| diag.primary.start >= span.start && diag.primary.end <= span.end)
}

/// Convert linkage merge errors into user-facing diagnostics.
fn linkage_error_to_diag(err: LinkageError, span: SourceSpan) -> SemaDiagnostic {
    match err {
        LinkageError::InvalidStorageClass(storage) => SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidLinkageMerge,
            format!("storage class {storage:?} is invalid in this scope"),
            span,
        ),
        LinkageError::ConflictingLinkage => SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidLinkageMerge,
            "conflicting linkage with previous declaration",
            span,
        ),
    }
}

fn enforce_inline_function_only(
    cx: &mut SemaContext<'_>,
    has_inline: bool,
    kind: SymbolKind,
    span: SourceSpan,
) {
    if !has_inline || kind == SymbolKind::Function {
        return;
    }

    cx.emit(
        SemaDiagnostic::new(
            SemaDiagnosticCode::TypeMismatch,
            "'inline' can only be applied to function declarations",
            span,
        )
        .with_note("for objects, remove 'inline'; for functions, use a function declarator"),
    );
}
