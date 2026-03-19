use super::*;
use crate::frontend::sema::typed_ast::{ConstValue, TypedExternalDecl, TypedInitializer};
use crate::frontend::sema::types::ArrayLen;

fn find_symbol<'a>(
    result: &'a sema::SemaResult,
    name: &str,
) -> &'a crate::frontend::sema::symbols::Symbol {
    for idx in 0..result.symbols.len() {
        let symbol = result.symbols.get(SymbolId(idx as u32));
        if symbol.name() == name {
            return symbol;
        }
    }
    panic!("symbol '{name}' not found");
}

#[test]
fn supports_array_aggregate_initialization_and_brace_elision() {
    let src = "int a[2][2] = { 1, 2, 3, 4 };";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn supports_struct_and_union_designated_initializers() {
    let src = r#"
        struct S { int a; int b; };
        union U { int i; char c; };
        struct S s = { .b = 2, .a = 1 };
        union U u = { .c = 1 };
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn supports_designator_followed_by_brace_elision() {
    let src = r#"
        struct S { int a[2]; };
        struct S s = { .a = 1, 2 };
        int m[2][2] = { [0] = 1, 2, [1][1] = 3 };
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn supports_standard_conversions_in_scalar_initializers() {
    let src = r#"
        int f(void);
        int main(void) {
            char *p = "abc";
            int (*fp)(void) = f;
            return p[0] + (fp != 0);
        }
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn infers_incomplete_array_length_from_aggregate_initializer() {
    let src = "int arr[] = { 1, 2, 3 };";
    let result = analyze_source(src).expect("sema should succeed");
    let symbol = find_symbol(&result, "arr");
    let TypeKind::Array { len, .. } = &result.types.get(symbol.ty()).kind else {
        panic!("arr should be an array");
    };
    assert_eq!(len, &ArrayLen::Known(3));
}

#[test]
fn infers_char_array_length_from_string_initializer() {
    let src = r#"char s[] = "abc";"#;
    let result = analyze_source(src).expect("sema should succeed");
    let symbol = find_symbol(&result, "s");
    let TypeKind::Array { len, .. } = &result.types.get(symbol.ty()).kind else {
        panic!("s should be an array");
    };
    assert_eq!(len, &ArrayLen::Known(4));
}

#[test]
fn supports_designated_aggregate_compound_literal() {
    let src = r#"
        struct S { int x; int y; };
        int f(void) { return ((struct S){ .y = 2, .x = 1 }).x; }
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn rejects_non_constant_file_scope_initializer() {
    let src = "int g; int x = g;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(
        &diagnostics,
        SemaDiagnosticCode::NonConstantInRequiredContext,
    );
}

#[test]
fn accepts_file_scope_address_constant_initializer() {
    let src = "int g; int *p = &g;";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_file_scope_string_literal_pointer_initializer() {
    let src = r#"char *p = "abc";"#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_file_scope_function_designator_pointer_initializer() {
    let src = r#"
        int f(void);
        int (*fp)(void) = f;
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_file_scope_integer_cast_pointer_initializer() {
    let src = r#"int *p = (int*)1;"#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn rejects_file_scope_integer_initializer_from_string_address_cast() {
    let src = r#"int x = (int)"abc";"#;
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(
        &diagnostics,
        SemaDiagnosticCode::NonConstantInRequiredContext,
    );
}

#[test]
fn rejects_file_scope_integer_initializer_from_function_address_cast() {
    let src = r#"
        int f(void);
        int x = (int)f;
    "#;
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(
        &diagnostics,
        SemaDiagnosticCode::NonConstantInRequiredContext,
    );
}

#[test]
fn accepts_file_scope_subobject_address_constant_initializer() {
    let src = r#"
        int a[3];
        int *p = &a[1];
        struct S { int x; };
        struct S s;
        int *q = &s.x;
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_file_scope_pointer_arithmetic_address_constant_initializer() {
    let src = r#"
        int a[4];
        int *p = &*(a + 1);
        int *q = &a[1] + 1;
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_file_scope_nested_subobject_address_constant_initializer() {
    let src = r#"
        struct S { int a[2]; };
        struct S s;
        int *p = &s.a[1];
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_file_scope_address_constant_with_ice_index() {
    let src = r#"
        int a[5];
        int *p = &a[1 + 2];
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn does_not_duplicate_file_scope_initializer_diagnostics_in_pass2() {
    let src = r#"int x = "abc";"#;
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    let mismatch_count = diagnostics
        .iter()
        .filter(|diag| diag.code == SemaDiagnosticCode::TypeMismatch)
        .count();
    assert_eq!(
        mismatch_count, 1,
        "expected one TypeMismatch diagnostic, got {diagnostics:?}"
    );
}

#[test]
fn accepts_braced_string_initializer_for_char_array() {
    let src = r#"char s[] = { "abc" };"#;
    let result = analyze_source(src).expect("sema should succeed");
    let symbol = find_symbol(&result, "s");
    let TypeKind::Array { len, .. } = &result.types.get(symbol.ty()).kind else {
        panic!("s should be an array");
    };
    assert_eq!(len, &ArrayLen::Known(4));
}

#[test]
fn typed_declaration_preserves_initializer_value() {
    let result = analyze_source("int x = 2;").expect("sema should succeed");
    let TypedExternalDecl::Declaration(decl) = &result.typed_tu.items[0] else {
        panic!("expected declaration");
    };
    assert_eq!(decl.initializers.len(), 1);
    let init_binding = &decl.initializers[0];
    match &init_binding.init {
        TypedInitializer::Expr(expr) => {
            assert_eq!(expr.const_value, Some(ConstValue::Int(2)));
        }
        other => panic!("unexpected initializer kind: {other:?}"),
    }
}

#[test]
fn union_initializer_keeps_selected_member_position() {
    let src = r#"
        union U { int a; int b; };
        union U u1 = { .a = 1 };
        union U u2 = { .b = 1 };
    "#;
    let result = analyze_source(src).expect("sema should succeed");

    let TypedExternalDecl::Declaration(d1) = &result.typed_tu.items[1] else {
        panic!("expected declaration for u1");
    };
    let TypedExternalDecl::Declaration(d2) = &result.typed_tu.items[2] else {
        panic!("expected declaration for u2");
    };
    let init1 = &d1.initializers[0].init;
    let init2 = &d2.initializers[0].init;

    let active_index = |init: &TypedInitializer| -> usize {
        let TypedInitializer::Aggregate(items) = init else {
            panic!("expected aggregate initializer");
        };
        items
            .iter()
            .position(|item| !matches!(item.init, TypedInitializer::ZeroInit { .. }))
            .expect("expected one active union member")
    };

    assert_eq!(active_index(init1), 0);
    assert_eq!(active_index(init2), 1);
}

// --- Regression tests for Phase 5 bug fixes ---

#[test]
fn accepts_string_initializer_for_nested_char_array_in_struct() {
    let src = r#"
        struct S { char s[4]; int x; };
        struct S a = { "abc", 1 };
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_designated_string_initializer_for_char_array_field() {
    let src = r#"
        struct S { char s[4]; int x; };
        struct S a = { .s = "abc", .x = 1 };
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_string_initializer_for_2d_char_array() {
    let src = r#"char a[2][4] = { "abc", "def" };"#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_string_initializer_for_union_char_array() {
    let src = r#"
        union U { char s[4]; int i; };
        union U u = { "abc" };
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn rejects_comma_expression_in_file_scope_initializer() {
    let src = "int x = (0, 1);";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(
        &diagnostics,
        SemaDiagnosticCode::NonConstantInRequiredContext,
    );
}

#[test]
fn accepts_conditional_address_constant_initializer() {
    let src = r#"
        int a[2];
        int *p = 1 ? &a[0] : &a[1];
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn incomplete_struct_initializer_reports_single_diagnostic() {
    let src = "struct S; struct S x = {0};";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    let incomplete_count = diagnostics
        .iter()
        .filter(|diag| diag.code == SemaDiagnosticCode::IncompleteType)
        .count();
    assert_eq!(
        incomplete_count, 1,
        "expected one IncompleteType diagnostic, got {diagnostics:?}"
    );
}

// --- Regression tests for review round 2 ---

#[test]
fn rejects_conditional_address_constant_with_non_constant_condition() {
    let src = r#"
        int g;
        int a[2];
        int *p = g ? &a[0] : &a[1];
    "#;
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(
        &diagnostics,
        SemaDiagnosticCode::NonConstantInRequiredContext,
    );
}

#[test]
fn accepts_extern_with_initializer_at_file_scope() {
    let src = "extern int x = 1;";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_extern_pointer_with_address_initializer() {
    let src = r#"
        extern int g;
        extern int *p = &g;
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn accepts_file_scope_compound_literal_pointer_init() {
    let src = "int *p = (int[]){1, 2};";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn sparse_array_initializer_does_not_oom() {
    // This should complete quickly without allocating a billion slots.
    let src = "int a[1000000000] = {[999999999] = 1};";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}
