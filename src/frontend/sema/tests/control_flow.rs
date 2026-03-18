use super::*;

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
