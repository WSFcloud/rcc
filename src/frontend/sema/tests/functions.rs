use super::*;

#[test]
fn analyzes_simple_function_successfully() {
    let src = "int g; int main(void) { int x = 1; return x; }";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_nonprototype_function_definitions() {
    let src = r#"
        int f() { return 1; }
        int main() { return f(); }
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_call_with_prior_prototype_and_later_definition() {
    let src = r#"
        int sum(int n);
        int main(void) { return sum(5); }
        int sum(int n) { return n; }
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_block_const_integer_as_array_bound() {
    let src = r#"
        int main(void) {
            const int size = 10;
            int fixed_arr[size];
            fixed_arr[0] = 1;
            return fixed_arr[0];
        }
    "#;
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

#[test]
fn allows_register_storage_in_block_scope() {
    let src = "int main(void) { register int x = 1; return x; }";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn allows_void_pointer_object_pointer_roundtrip_assignment() {
    let src = r#"
        int main(void) {
            int value = 0;
            int *p = &value;
            void *vp = p;
            p = vp;
            return *p;
        }
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn rejects_cast_between_void_pointer_and_function_pointer() {
    let src = r#"
        int callee(void) { return 0; }
        int main(void) {
            void *vp = 0;
            int (*fp)(void) = (int (*)(void))vp;
            return fp();
        }
    "#;
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::InvalidCast);
}

#[test]
fn rejects_member_access_on_incomplete_record_pointer() {
    let src = "struct S; int f(struct S *p) { return p->x; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::IncompleteType);
}

#[test]
fn rejects_dereferencing_void_pointer() {
    let src = "int main(void) { void *p = 0; return *p; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn reports_constant_integer_division_by_zero_in_runtime_expr() {
    let src = "int main(void) { int x = 1; return x / 0; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::ConstantDivisionByZero);
}

#[test]
fn reports_constant_integer_modulo_by_zero_in_compound_assignment() {
    let src = "int main(void) { int x = 7; x %= 0; return x; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::ConstantDivisionByZero);
}

#[test]
fn rejects_dropping_pointee_const_qualification() {
    let src = r#"
        int main(void) {
            const int *cp = 0;
            int *p = cp;
            return 0;
        }
    "#;
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn rejects_double_pointer_const_hole_assignment() {
    let src = r#"
        int main(void) {
            int **pp = 0;
            const int **cpp = 0;
            cpp = pp;
            return 0;
        }
    "#;
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}
