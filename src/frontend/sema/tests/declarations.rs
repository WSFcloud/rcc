use super::*;

#[test]
fn reports_extern_after_static_linkage_conflict() {
    let src = "static int value; extern int value;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::InvalidLinkageMerge);
}

#[test]
fn inherits_internal_linkage_from_prior_static_object_declaration() {
    let src = "static int value; int value;";
    let result = analyze_source(src).expect("sema should succeed");
    let symbol = result.symbols.get(SymbolId(0));
    assert_eq!(symbol.linkage(), Linkage::Internal);
}

#[test]
fn inherits_internal_linkage_from_prior_static_function_declaration() {
    let src = "static int f(void); int f(void);";
    let result = analyze_source(src).expect("sema should succeed");
    let symbol = result.symbols.get(SymbolId(0));
    assert_eq!(symbol.linkage(), Linkage::Internal);
    assert_eq!(symbol.kind(), SymbolKind::Function);
}

#[test]
fn finalizes_tentative_definition_at_tu_end() {
    let src = "int global_value;";
    let result = analyze_source(src).expect("sema should succeed");
    let symbol = result.symbols.get(SymbolId(0));
    assert_eq!(symbol.status(), DefinitionStatus::Defined);
}

#[test]
fn extern_incomplete_array_declaration_is_not_tentative() {
    let src = "extern int ext_arr[];";
    let result = analyze_source(src).expect("sema should succeed");
    let symbol = result.symbols.get(SymbolId(0));
    assert_eq!(symbol.status(), DefinitionStatus::Declared);
}

#[test]
fn reports_incomplete_element_type_for_tentative_array_definition() {
    let src = "struct S; struct S arr[1];";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::IncompleteType);
}

#[test]
fn rejects_typedef_without_declarator() {
    let src = "typedef int;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn supports_typedef_and_tagged_record_types() {
    let src = r#"
        typedef int my_int;
        struct Point { int x; int y; };
        my_int value;
        struct Point pt;
    "#;

    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn rejects_void_parameter_in_multi_parameter_prototype() {
    let src = "int f(void, int);";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn rejects_function_redeclaration_with_variadic_mismatch() {
    let src = "int f(int); int f(int, ...);";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::IncompatibleTypes);
}

#[test]
fn supports_enum_constants_and_sizeof_in_array_length() {
    let src = r#"
        enum { A = 2, B = A + 3 };
        int arr[sizeof(int) + B];
    "#;

    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn supports_sizeof_complete_struct_in_array_length() {
    let src = r#"
        struct S { int a; char b; };
        int arr[sizeof(struct S)];
    "#;

    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn supports_short_circuit_integer_constant_expressions() {
    let src = r#"
        enum { A = 0 && (1 / 0), B = 1 || (1 / 0) };
        int arr[A + B + 1];
    "#;

    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn reports_division_by_zero_in_constant_expression() {
    let src = "enum { A = 1 / 0 };";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::ConstantDivisionByZero);
}

#[test]
fn invalid_enum_value_does_not_define_enumerator_symbol() {
    let src = "enum { A = 1 / 0 }; int x = A;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::ConstantDivisionByZero);
    assert_has_code(&diagnostics, SemaDiagnosticCode::UndefinedSymbol);
}

#[test]
fn reports_signed_overflow_in_constant_expression() {
    let src = "enum { A = 1 << 63 };";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::ConstantSignedOverflow);
}

#[test]
fn classifies_function_pointer_and_function_declarators_correctly() {
    let src = "int *f(void); int (*fp)(void);";
    let result = analyze_source(src).expect("sema should succeed");

    let f = result.symbols.get(SymbolId(0));
    let fp = result.symbols.get(SymbolId(1));
    assert_eq!(f.kind(), SymbolKind::Function);
    assert_eq!(fp.kind(), SymbolKind::Object);
}

#[test]
fn rejects_incompatible_typedef_redeclaration_for_function_type() {
    let src = "typedef int F(); typedef int F(int);";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::RedeclarationConflict);
}

#[test]
fn reports_incomplete_tentative_definition_at_tu_end() {
    let src = "int arr[];";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::IncompleteType);
}

#[test]
fn rejects_long_double_in_v1() {
    let src = "long double x;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn rejects_short_float_combination() {
    let src = "short float x;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn rejects_void_with_int_combination() {
    let src = "void int x;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}
