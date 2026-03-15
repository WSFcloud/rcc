use super::*;

// Typedef-name resolution and scope tests
#[test]
fn parses_typedef_and_usage() {
    let unit = parse_source("typedef int T; T x;");
    let ExternalDecl::Declaration(typedef_decl) = &unit.items[0] else {
        panic!("expected typedef");
    };
    assert_eq!(typedef_decl.specifiers.storage, vec![StorageClass::Typedef]);

    let ExternalDecl::Declaration(var_decl) = &unit.items[1] else {
        panic!("expected declaration");
    };
    assert_eq!(
        var_decl.specifiers.ty,
        vec![TypeSpecifier::TypedefName("T".to_string())]
    );
}

#[test]
fn parses_typedef_in_cast_and_sizeof() {
    let unit = parse_source("typedef int T; int f(void) { return (T)+1 + sizeof(T); }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function");
    };
    let expr = expect_return_expr(&def.body.items[0]);
    let ExprKind::Binary { left, right, .. } = &expr.kind else {
        panic!("expected binary");
    };

    // Cast
    let ExprKind::Cast { ty, .. } = &left.kind else {
        panic!("expected cast");
    };
    assert_eq!(
        ty.specifiers.ty,
        vec![TypeSpecifier::TypedefName("T".to_string())]
    );

    // Sizeof
    let ExprKind::SizeofType(ty) = &right.kind else {
        panic!("expected sizeof");
    };
    assert_eq!(
        ty.specifiers.ty,
        vec![TypeSpecifier::TypedefName("T".to_string())]
    );
}

#[test]
fn ordinary_identifier_shadows_typedef() {
    let unit = parse_source("typedef int T; void f(void) { int T; T = 1; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function");
    };
    let BlockItem::Decl(inner_decl) = &def.body.items[0] else {
        panic!("expected declaration");
    };
    assert_ident_declarator(&inner_decl.declarators[0], "T");
}

#[test]
fn rejects_conflicting_same_scope_typedef() {
    assert!(!parse_source_error("typedef int T; int T;").is_empty());
}

#[test]
fn enum_shadows_typedef() {
    let unit = parse_source("typedef int T; int f(void) { enum { T = 42 }; return T; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function");
    };
    let BlockItem::Decl(enum_decl) = &def.body.items[0] else {
        panic!("expected enum");
    };
    let TypeSpecifier::Enum(enum_spec) = &enum_decl.specifiers.ty[0] else {
        panic!("expected enum");
    };
    let variants = enum_spec.variants.as_ref().expect("variants expected");
    assert_eq!(variants[0].name, "T");
}

#[test]
fn typedef_in_for_init() {
    let unit = parse_source("typedef int T; void f(void) { for (T i = 0; i < 1; i++) {} }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function");
    };
    let stmt = expect_stmt(&def.body.items[0]);
    let StmtKind::For { init, .. } = &stmt.kind else {
        panic!("expected for");
    };
    let Some(ForInit::Decl(decl)) = init else {
        panic!("expected decl init");
    };
    assert_eq!(
        decl.specifiers.ty,
        vec![TypeSpecifier::TypedefName("T".to_string())]
    );
}

#[test]
fn parameter_shadows_typedef() {
    let unit = parse_source("typedef int T; int f(T T) { T = 1; return T; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function");
    };
    let DirectDeclarator::Function { params, .. } = def.declarator.direct.as_ref() else {
        panic!("expected function declarator");
    };
    let FunctionParams::Prototype { params, .. } = params else {
        panic!("expected prototype");
    };
    assert_eq!(
        params[0].specifiers.ty,
        vec![TypeSpecifier::TypedefName("T".to_string())]
    );
}

#[test]
fn function_name_shadows_typedef() {
    let errors = parse_source_error("typedef int f; f f(void) { f x; return x; }");
    assert!(!errors.is_empty());
}
