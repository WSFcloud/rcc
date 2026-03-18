use crate::common::span::SourceSpan;
use crate::frontend::parser::ast::{
    ArraySize, DeclSpec, Declaration, Declarator, DirectDeclarator, DirectDeclaratorKind,
    EnumSpecifier, ExternalDecl, FunctionDef, FunctionParams, ParameterDecl, Pointer, RecordKind,
    RecordMemberDecl, RecordSpecifier, StorageClass, TranslationUnit, TypeName, TypeQualifier,
};
use crate::frontend::sema::context::SemaContext;
use crate::frontend::sema::diagnostic::{SemaDiagnostic, SemaDiagnosticCode};
use crate::frontend::sema::symbols::{
    DefinitionStatus, Linkage, LinkageError, Symbol, SymbolId, SymbolKind, infer_linkage,
};
use crate::frontend::sema::typed_ast::TypedDeclaration;
use crate::frontend::sema::types::{
    ArrayLen, EnumConstant, FieldDef, Qualifiers, TagId, Type, TypeId, TypeKind,
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

    for init_decl in &decl.declarators {
        let Some(name) = declarator_ident(&init_decl.declarator) else {
            continue;
        };
        if let Some(symbol_id) = cx.lookup_ordinary(name) {
            symbols.push(symbol_id);
        }
    }

    TypedDeclaration {
        symbols,
        span: declaration_span(decl),
    }
}

/// Finalize tentative definitions at end of translation unit.
pub fn finalize_tentative_definitions(cx: &mut SemaContext<'_>) {
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
    }
}

/// Ensure one function symbol exists in the current scope and return its symbol id.
///
/// TODO: implement function declaration building, linkage merge, and diagnostics.
pub fn ensure_function_symbol(_cx: &mut SemaContext<'_>, _func: &FunctionDef) -> Option<SymbolId> {
    todo!("sema decl: ensure function symbol");
}

/// Lower one local declaration inside a block scope.
///
/// TODO: implement local declaration lowering, duplicate checks, and initializer checks.
pub fn lower_local_declaration(_cx: &mut SemaContext<'_>, _decl: &Declaration) -> TypedDeclaration {
    todo!("sema decl: lower local declaration");
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
///
/// TODO: implement base-type resolution and parameter declarator application.
pub(crate) fn build_parameter_type(
    _cx: &mut SemaContext<'_>,
    _parameter: &ParameterDecl,
) -> TypeId {
    todo!("sema decl: build parameter type");
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
///
/// TODO: implement symbol creation/merge, linkage rules, and tentative definition status.
fn declare_file_scope_declaration(_cx: &mut SemaContext<'_>, _decl: &Declaration) {
    todo!("sema decl: declare file-scope declaration");
}

/// Classify a declaration as object/function/typedef symbol kind.
fn symbol_kind(specifiers: &DeclSpec, declarator: &Declarator, is_definition: bool) -> SymbolKind {
    if specifiers.storage.contains(&StorageClass::Typedef) {
        return SymbolKind::Typedef;
    }
    if is_definition
        || matches!(
            declarator.direct.kind,
            DirectDeclaratorKind::Function { .. }
        )
    {
        return SymbolKind::Function;
    }
    SymbolKind::Object
}

/// Validate and normalize storage-class specifiers.
fn normalize_storage(
    specifiers: &DeclSpec,
    span: SourceSpan,
) -> Result<Option<StorageClass>, SemaDiagnostic> {
    if specifiers.storage.len() > 1 {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidLinkageMerge,
            "multiple storage-class specifiers are not yet normalized",
            span,
        ));
    }
    Ok(specifiers.storage.first().copied())
}

/// Build full declaration type from specifiers and declarator.
///
/// TODO: replace placeholder flow once declarator/type builder is fully implemented.
fn build_decl_type(
    cx: &mut SemaContext<'_>,
    specifiers: &DeclSpec,
    declarator: &Declarator,
    span: SourceSpan,
) -> TypeId {
    let base_ty = resolve_base_type(cx, specifiers, span);
    apply_declarator_with_base(cx, base_ty, declarator)
}

/// Apply declarator modifiers over a computed base type.
///
/// TODO: implement full inside-out declarator composition.
fn apply_declarator_with_base(
    _cx: &mut SemaContext<'_>,
    _base_ty: TypeId,
    _declarator: &Declarator,
) -> TypeId {
    todo!("sema decl: apply declarator with base");
}

/// Apply pointer layers (`*`) over a type.
///
/// TODO: implement pointer qualifier and ordering handling.
fn apply_pointer_layers(_cx: &mut SemaContext<'_>, _ty: TypeId, _pointers: &[Pointer]) -> TypeId {
    todo!("sema decl: apply pointer layers");
}

/// Apply one direct declarator chain (array/function/grouped/identifier).
///
/// TODO: implement direct declarator lowering recursion.
fn apply_direct_declarator(
    _cx: &mut SemaContext<'_>,
    _base_ty: TypeId,
    _direct: &DirectDeclarator,
) -> TypeId {
    todo!("sema decl: apply direct declarator");
}

/// Build function type from return type and parsed parameter list.
///
/// TODO: implement prototype/non-prototype handling and parameter normalization.
fn build_function_type(
    _cx: &mut SemaContext<'_>,
    _ret_ty: TypeId,
    _params: &FunctionParams,
) -> TypeId {
    todo!("sema decl: build function type");
}

/// Resolve declaration specifiers into a semantic base type.
fn resolve_base_type(cx: &mut SemaContext<'_>, specifiers: &DeclSpec, span: SourceSpan) -> TypeId {
    match try_build_base_type(cx, specifiers, span) {
        Ok(ty) => ty,
        Err(diag) => {
            cx.emit(diag);
            cx.error_type()
        }
    }
}

/// Normalize parameter types according to C decay rules.
///
/// TODO: implement array/function parameter decay and qualifier adjustments.
fn normalize_function_parameter_type(_cx: &mut SemaContext<'_>, _param_ty: TypeId) -> TypeId {
    todo!("sema decl: normalize function parameter type");
}

/// Convert array-size syntax node into semantic array length.
///
/// TODO: implement constant-expression evaluation and VM-type diagnostics.
fn array_len_from_size_expr(
    _cx: &mut SemaContext<'_>,
    _size: &ArraySize,
    _span: SourceSpan,
) -> ArrayLen {
    todo!("sema decl: resolve array length");
}

/// Build builtin/typedef/record/enum base type from declaration specifiers.
///
/// TODO: implement C type-specifier combination validation and lowering.
fn try_build_base_type(
    _cx: &mut SemaContext<'_>,
    _specifiers: &DeclSpec,
    _span: SourceSpan,
) -> Result<TypeId, SemaDiagnostic> {
    todo!("sema decl: build base type from specifiers");
}

/// Evaluate an integer constant expression for contexts like enum/array length.
///
/// TODO: implement constant-expression folding.
fn evaluate_integer_constant_expr(
    _cx: &mut SemaContext<'_>,
    _expr: &crate::frontend::parser::ast::Expr,
) -> Option<i64> {
    todo!("sema decl: evaluate integer constant expr");
}

/// Resolve a typedef name into its target semantic type.
///
/// TODO: implement typedef lookup and diagnostics.
fn resolve_typedef_name_type(
    _cx: &mut SemaContext<'_>,
    _name: &str,
    _span: SourceSpan,
) -> Result<TypeId, SemaDiagnostic> {
    todo!("sema decl: resolve typedef name");
}

/// Resolve a `struct`/`union` specifier into semantic record type.
///
/// TODO: implement tag lookup/registration and record completion rules.
fn resolve_record_specifier_type(
    _cx: &mut SemaContext<'_>,
    _record_spec: &RecordSpecifier,
    _span: SourceSpan,
) -> Result<TypeId, SemaDiagnostic> {
    todo!("sema decl: resolve record specifier");
}

/// Resolve an `enum` specifier into semantic enum type.
///
/// TODO: implement tag lookup/registration and variant lowering.
fn resolve_enum_specifier_type(
    _cx: &mut SemaContext<'_>,
    _enum_spec: &EnumSpecifier,
    _span: SourceSpan,
) -> Result<TypeId, SemaDiagnostic> {
    todo!("sema decl: resolve enum specifier");
}

/// Lower record member declarations into field definitions.
///
/// TODO: implement field type lowering and bit-field handling.
fn lower_record_fields(_cx: &mut SemaContext<'_>, _members: &[RecordMemberDecl]) -> Vec<FieldDef> {
    todo!("sema decl: lower record fields");
}

/// Lower enum variants into semantic constants and symbol table entries.
///
/// TODO: implement enumerator value assignment and duplicate checking.
fn lower_enum_variants(
    _cx: &mut SemaContext<'_>,
    _variants: &[crate::frontend::parser::ast::EnumVariant],
    _underlying_ty: TypeId,
) -> Vec<EnumConstant> {
    todo!("sema decl: lower enum variants");
}

/// Register one tag (`struct`/`union`/`enum`) in current scope.
///
/// TODO: implement tag insertion conflict handling.
fn register_tag(
    _cx: &mut SemaContext<'_>,
    _tag: &str,
    _tag_id: TagId,
    _span: SourceSpan,
) -> Result<(), SemaDiagnostic> {
    todo!("sema decl: register tag");
}

/// Return canonical semantic `int` type.
///
/// TODO: route through central builtin type cache when available.
fn int_type(_cx: &mut SemaContext<'_>) -> TypeId {
    todo!("sema decl: int type helper");
}

/// Build semantic type from parser `TypeName` node.
///
/// TODO: implement typenames with abstract declarator support.
fn build_type_from_type_name(
    _cx: &mut SemaContext<'_>,
    _ty_name: &TypeName,
    _span: SourceSpan,
) -> TypeId {
    todo!("sema decl: build type from type-name");
}

/// Infer expression type for `sizeof(expr)` in constant-eval contexts.
///
/// TODO: implement expression type inference subset for `sizeof`.
fn infer_sizeof_expr_type(
    _cx: &mut SemaContext<'_>,
    _expr: &crate::frontend::parser::ast::Expr,
) -> Option<TypeId> {
    todo!("sema decl: infer sizeof expression type");
}

/// Compute static type size in bytes for semantic type nodes.
///
/// TODO: implement ABI-aware layout and object-size rules.
fn type_size_of(_cx: &SemaContext<'_>, _ty: TypeId) -> Option<u64> {
    todo!("sema decl: type size evaluation");
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
///
/// TODO: implement full completeness rules for arrays/records/function types.
fn is_complete_type(_ty: TypeId, _cx: &SemaContext<'_>) -> bool {
    todo!("sema decl: complete-type check");
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
