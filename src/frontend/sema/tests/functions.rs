use super::*;

#[test]
fn analyzes_simple_function_successfully() {
    let src = "int g; int main(void) { int x = 1; return x; }";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn static_inline_function_uses_internal_linkage() {
    let src = "static inline int helper(void) { return 42; }";
    let result = analyze_source(src).expect("sema should succeed");
    let symbol = result.symbols.get(SymbolId(0));
    assert_eq!(symbol.linkage(), Linkage::Internal);
}

#[test]
fn function_parameter_keeps_declared_type() {
    let src = "int id(int x) { return x; }";
    let result = analyze_source(src).expect("sema should succeed");

    let mut found = false;
    for idx in 0..result.symbols.len() {
        let symbol = result.symbols.get(SymbolId(idx as u32));
        if symbol.name() == "x" {
            found = true;
            assert!(
                !matches!(result.types.get(symbol.ty()).kind, TypeKind::Error),
                "function parameter should not use error type"
            );
        }
    }

    assert!(found, "parameter symbol not found");
}

#[test]
fn normalizes_array_parameter_to_pointer_in_function_body() {
    let src = "int f(int a[3]) { a = 0; return 0; }";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");

    let result = result.expect("sema should succeed");
    let mut found = false;
    for idx in 0..result.symbols.len() {
        let symbol = result.symbols.get(SymbolId(idx as u32));
        if symbol.name() == "a" {
            found = true;
            assert!(
                matches!(result.types.get(symbol.ty()).kind, TypeKind::Pointer { .. }),
                "array parameter should decay to pointer"
            );
        }
    }
    assert!(found, "parameter symbol not found");
}

#[test]
fn allows_repeated_block_scope_function_declarations() {
    let src = "int main(void) { int helper(void); int helper(void); return 0; }";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn block_scope_function_declaration_reuses_outer_entity() {
    let src = "int helper(void); int main(void) { int helper(void); return helper(); }";
    let result = analyze_source(src).expect("sema should succeed");

    let mut helper_count = 0usize;
    for idx in 0..result.symbols.len() {
        let symbol = result.symbols.get(SymbolId(idx as u32));
        if symbol.name() == "helper" {
            helper_count += 1;
            assert_eq!(symbol.kind(), SymbolKind::Function);
            assert_eq!(symbol.linkage(), Linkage::External);
        }
    }

    assert_eq!(
        helper_count, 1,
        "block-scope function declaration should reuse file-scope function symbol"
    );
}

#[test]
fn accepts_block_scope_static_object_declaration() {
    let src = "int main(void) { static int x = 1; return x; }";
    let result = analyze_source(src).expect("sema should succeed");

    let mut found = false;
    for idx in 0..result.symbols.len() {
        let symbol = result.symbols.get(SymbolId(idx as u32));
        if symbol.name() == "x" {
            found = true;
            assert_eq!(symbol.kind(), SymbolKind::Object);
            assert_eq!(symbol.linkage(), Linkage::None);
        }
    }

    assert!(found, "local static symbol not found");
}

#[test]
fn rejects_non_constant_block_scope_static_initializer() {
    let src = "int g; int main(void) { static int x = g; return x; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(
        &diagnostics,
        SemaDiagnosticCode::NonConstantInRequiredContext,
    );
}

#[test]
fn rejects_incompatible_pointer_equality_comparison() {
    let src = "int main(void) { int *p; char *q; return p == q; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn allows_pointer_equality_comparison_with_void_pointer() {
    let src = "int main(void) { int *p; void *q; return p == q; }";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn comma_expression_preserves_array_decay_in_subscript_context() {
    let src = "int main(void) { int arr[2]; return (0, arr)[0]; }";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}
