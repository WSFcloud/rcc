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
