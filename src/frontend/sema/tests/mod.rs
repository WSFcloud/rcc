use crate::frontend::lexer::lexer_from_source;
use crate::frontend::parser::parse;
use crate::frontend::sema;
use crate::frontend::sema::diagnostic::SemaDiagnosticCode;
use crate::frontend::sema::symbols::{DefinitionStatus, Linkage, SymbolId, SymbolKind};
use crate::frontend::sema::types::TypeKind;
use chumsky::input::{Input, Stream};

fn analyze_source(src: &str) -> Result<sema::SemaResult, Vec<sema::diagnostic::SemaDiagnostic>> {
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));
    let tu = parse::parse(stream).expect("test source should parse");
    sema::analyze("test.c", src, &tu)
}

fn assert_has_code(diags: &[sema::diagnostic::SemaDiagnostic], code: SemaDiagnosticCode) {
    assert!(
        diags.iter().any(|diag| diag.code == code),
        "missing diagnostic code {code:?}, actual diagnostics: {diags:?}"
    );
}

#[test]
fn analyzes_simple_function_successfully() {
    let src = "int g; int main(void) { int x = 1; return x; }";
    let result = analyze_source(src);
    assert!(result.is_ok(), "unexpected diagnostics: {result:?}");
}

#[test]
fn reports_undefined_symbol_in_expression() {
    let src = "int main(void) { return missing_value; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::UndefinedSymbol);
}

#[test]
fn reports_undefined_goto_label() {
    let src = "int main(void) { goto done; return 0; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::UndefinedLabel);
}

#[test]
fn reports_duplicate_label_definition() {
    let src = "int main(void) { L: return 0; L: return 1; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::DuplicateLabel);
}

#[test]
fn reports_redeclaration_in_same_block() {
    let src = "int main(void) { int x = 0; int x = 1; return x; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::RedeclarationConflict);
}

#[test]
fn reports_jump_over_initializer() {
    let src = "int main(void) { goto done; int x = 42; done: return x; }";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::JumpOverInitializer);
}

#[test]
fn reports_extern_after_static_linkage_conflict() {
    let src = "static int value; extern int value;";
    let diagnostics = analyze_source(src).expect_err("sema should fail");
    assert_has_code(&diagnostics, SemaDiagnosticCode::InvalidLinkageMerge);
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
fn supports_enum_constants_and_sizeof_in_array_length() {
    let src = r#"
        enum { A = 2, B = A + 3 };
        int arr[sizeof(int) + B];
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
