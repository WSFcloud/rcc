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
