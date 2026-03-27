use super::*;
use crate::frontend::sema::symbols::ObjectStorageClass;

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
fn synthesizes_zero_initializer_for_tentative_definition() {
    let src = "int global_value;";
    let result = analyze_source(src).expect("sema should succeed");
    let target = SymbolId(0);

    let mut found_zero_init = false;
    for item in &result.typed_tu.items {
        let crate::frontend::sema::typed_ast::TypedExternalDecl::Declaration(decl) = item else {
            continue;
        };
        for init in &decl.initializers {
            if init.symbol == target
                && matches!(
                    init.init,
                    crate::frontend::sema::typed_ast::TypedInitializer::ZeroInit { .. }
                )
            {
                found_zero_init = true;
            }
        }
    }

    assert!(
        found_zero_init,
        "expected synthesized zero initializer for tentative definition"
    );
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
fn allows_function_redeclaration_between_enum_and_underlying_int_param() {
    let src = r#"
        enum E { A = 1 };
        int f(enum E x);
        int f(int x);
    "#;
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
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

#[test]
fn rejects_inline_on_object_declaration() {
    let src = "inline int x;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn rejects_register_at_file_scope() {
    let src = "register int x;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::InvalidLinkageMerge);
}

#[test]
fn reports_enum_value_not_representable_as_int() {
    let src = "enum { A = 2147483648LL };";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn out_of_range_enum_value_does_not_define_enumerator_symbol() {
    let src = "enum { A = 2147483648LL }; int x = A;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
    assert_has_code(&diagnostics, SemaDiagnosticCode::UndefinedSymbol);
}

#[test]
fn rejects_array_of_incomplete_type_even_for_extern_declaration() {
    let src = "struct S; extern struct S arr[1];";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::IncompleteType);
}

#[test]
fn rejects_array_of_function_type() {
    let src = "typedef int Fn(void); Fn table[2];";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn rejects_function_returning_array_type() {
    let src = "typedef int Arr2[2]; Arr2 make(void);";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn rejects_function_returning_function_type() {
    let src = "typedef int Fn(void); Fn make(void);";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn rejects_duplicate_member_names_within_record() {
    let src = "struct S { int x; int x; };";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::RedeclarationConflict);
}

#[test]
fn accepts_valid_flexible_array_member() {
    let src = "struct S { int n; int data[]; };";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn rejects_flexible_array_member_without_named_prefix_member() {
    let src = "struct S { int data[]; };";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn rejects_non_last_flexible_array_member() {
    let src = "struct S { int data[]; int tail; };";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::IncompleteType);
}

#[test]
fn rejects_incomplete_union_member_type() {
    let src = "struct S; union U { struct S value; };";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::IncompleteType);
}

#[test]
fn rejects_recursive_record_member_by_value() {
    let src = "struct S { struct S self; };";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::IncompleteType);
}

#[test]
fn allows_recursive_record_member_via_pointer() {
    let src = "struct S { struct S *next; }; struct S head;";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn rejects_empty_record_definitions() {
    let src = "struct EmptyS { }; union EmptyU { };";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::TypeMismatch);
}

#[test]
fn tracks_file_scope_object_storage_class_metadata() {
    let src = "int g; static int sg; extern int eg;";
    let result = analyze_source(src).expect("sema should succeed");

    assert_eq!(
        find_symbol(&result, "g").object_storage_class(),
        Some(ObjectStorageClass::FileScope)
    );
    assert_eq!(
        find_symbol(&result, "sg").object_storage_class(),
        Some(ObjectStorageClass::FileScope)
    );
    assert_eq!(
        find_symbol(&result, "eg").object_storage_class(),
        Some(ObjectStorageClass::FileScope)
    );
}

#[test]
fn tracks_block_scope_object_storage_class_metadata() {
    let src = r#"
        int f(void) {
            int a = 0;
            register int r = 0;
            static int s = 0;
            extern int ext_only;
            return a + r + s;
        }
    "#;
    let result = analyze_source(src).expect("sema should succeed");

    assert_eq!(
        find_symbol(&result, "a").object_storage_class(),
        Some(ObjectStorageClass::Auto)
    );
    assert_eq!(
        find_symbol(&result, "r").object_storage_class(),
        Some(ObjectStorageClass::Register)
    );
    assert_eq!(
        find_symbol(&result, "s").object_storage_class(),
        Some(ObjectStorageClass::Static)
    );
    assert_eq!(
        find_symbol(&result, "ext_only").object_storage_class(),
        Some(ObjectStorageClass::Extern)
    );
}
