use super::*;

// Type-name and abstract declarator tests
#[test]
fn parses_sizeof_with_abstract_array() {
    let cases = [
        (
            "int f(void) { return sizeof(int [10]); }",
            ArraySize::Expr(Expr::int(10)),
        ),
        (
            "int f(void) { return sizeof(int [2 + 1]); }",
            ArraySize::Expr(Expr::binary(Expr::int(2), BinaryOp::Add, Expr::int(1))),
        ),
    ];
    for (src, expected_size) in cases {
        let unit = parse_source(src);
        let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
            panic!("expected function");
        };
        let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
            panic!("expected return");
        };
        let ExprKind::SizeofType(ty) = &expr.kind else {
            panic!("expected sizeof(type)");
        };
        let declarator = ty
            .declarator
            .as_ref()
            .expect("abstract array declarator expected");
        let DirectDeclarator::Array { size, .. } = declarator.direct.as_ref() else {
            panic!("expected array");
        };
        assert_eq!(size.as_ref(), &expected_size);
    }
}

#[test]
fn parses_cast_with_function_pointer() {
    let unit = parse_source("int f(void) { return (int (*)(int))fp; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
        panic!("expected function");
    };
    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
        panic!("expected return");
    };
    let ExprKind::Cast { ty, .. } = &expr.kind else {
        panic!("expected cast");
    };
    let declarator = ty
        .declarator
        .as_ref()
        .expect("function-pointer abstract declarator expected");
    let DirectDeclarator::Function { inner, params } = declarator.direct.as_ref() else {
        panic!("expected function");
    };
    let DirectDeclarator::Grouped(grouped) = inner.as_ref() else {
        panic!("expected grouped");
    };
    assert_eq!(grouped.pointers.len(), 1);
    assert_eq!(grouped.direct.as_ref(), &DirectDeclarator::Abstract);

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 1);
}

#[test]
fn parses_function_pointer_with_pointer_params() {
    let cases = [
        "int f(void) { return sizeof(void (*)(int *)); }",
        "int f(void) { return (int (*)(const char *))ptr; }",
    ];
    for src in cases {
        let unit = parse_source(src);
        let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
            panic!("expected function");
        };
        let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
            panic!("expected return");
        };
        let ty = match &expr.kind {
            ExprKind::SizeofType(ty) => ty,
            ExprKind::Cast { ty, .. } => ty,
            _ => panic!("expected sizeof or cast"),
        };
        let declarator = ty.declarator.as_ref().expect("declarator expected");
        let DirectDeclarator::Function { params, .. } = declarator.direct.as_ref() else {
            panic!("expected function");
        };
        let FunctionParams::Prototype { params, .. } = params else {
            panic!("expected prototype");
        };
        assert_eq!(params.len(), 1);
        let param_decl = params[0]
            .declarator
            .as_ref()
            .expect("pointer param expected");
        assert_eq!(param_decl.pointers.len(), 1);
    }
}

#[test]
fn parses_function_pointer_with_array_param() {
    let unit = parse_source("int f(void) { return sizeof(int (*)(int, char *[])); }");
    let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
        panic!("expected function");
    };
    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
        panic!("expected return");
    };
    let ExprKind::SizeofType(ty) = &expr.kind else {
        panic!("expected sizeof");
    };
    let declarator = ty.declarator.as_ref().expect("declarator expected");
    let DirectDeclarator::Function { params, .. } = declarator.direct.as_ref() else {
        panic!("expected function");
    };
    let FunctionParams::Prototype { params, .. } = params else {
        panic!("expected prototype");
    };
    assert_eq!(params.len(), 2);
    let second_param = params[1]
        .declarator
        .as_ref()
        .expect("second param expected");
    assert_eq!(second_param.pointers.len(), 1);
    let DirectDeclarator::Array { size, .. } = second_param.direct.as_ref() else {
        panic!("expected array");
    };
    assert_eq!(size.as_ref(), &ArraySize::Unspecified);
}
