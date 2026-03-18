use super::*;

// Function declarator and parameter tests
#[test]
fn parses_function_declarations() {
    let cases = [
        ("int main(void);", "main", 0, false),
        ("int sum(int x, char *p);", "sum", 2, false),
        ("int printf(const char *fmt, ...);", "printf", 1, true),
    ];
    for (src, name, param_count, variadic) in cases {
        let unit = parse_source(src);
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration");
        };
        let DirectDeclaratorKind::Function { inner, params } =
            &decl.declarators[0].declarator.direct.kind
        else {
            panic!("expected function declarator");
        };
        assert_direct_ident(inner.as_ref(), name);
        let FunctionParams::Prototype {
            params,
            variadic: v,
        } = params
        else {
            panic!("expected prototype");
        };
        assert_eq!(params.len(), param_count);
        assert_eq!(*v, variadic);
    }
}

#[test]
fn parses_array_parameter_variants() {
    let cases = [
        (
            "int f(int a[]);",
            Vec::<TypeQualifier>::new(),
            false,
            ArraySize::Unspecified,
        ),
        (
            "int f(int a[static 4]);",
            Vec::<TypeQualifier>::new(),
            true,
            ArraySize::Expr(Expr::int(4)),
        ),
        (
            "int f(int a[const static 4]);",
            vec![TypeQualifier::Const],
            true,
            ArraySize::Expr(Expr::int(4)),
        ),
    ];
    for (src, qualifiers, is_static, size) in cases {
        let unit = parse_source(src);
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration");
        };
        let DirectDeclaratorKind::Function { params, .. } =
            &decl.declarators[0].declarator.direct.kind
        else {
            panic!("expected function");
        };
        let FunctionParams::Prototype { params, .. } = params else {
            panic!("expected prototype");
        };
        let param_decl = params[0]
            .declarator
            .as_ref()
            .expect("param declarator expected");
        let DirectDeclaratorKind::Array {
            qualifiers: q,
            is_static: s,
            size: sz,
            ..
        } = &param_decl.direct.kind
        else {
            panic!("expected array");
        };
        assert_eq!(q.as_slice(), qualifiers.as_slice());
        assert_eq!(*s, is_static);
        assert_eq!(sz.as_ref(), &size);
    }
}

#[test]
fn parses_function_pointer_declarator() {
    let unit = parse_source("int (*fp)(int);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };
    let DirectDeclaratorKind::Function { inner, .. } = &decl.declarators[0].declarator.direct.kind
    else {
        panic!("expected function");
    };
    let DirectDeclaratorKind::Grouped(grouped) = &inner.kind else {
        panic!("expected grouped");
    };
    assert_eq!(grouped.pointers.len(), 1);
    assert_direct_ident(grouped.direct.as_ref(), "fp");
}

#[test]
fn parses_function_pointer_parameter() {
    let unit = parse_source("int f(int (*callback)(int));");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };
    let DirectDeclaratorKind::Function { params, .. } = &decl.declarators[0].declarator.direct.kind
    else {
        panic!("expected function");
    };
    let FunctionParams::Prototype { params, .. } = params else {
        panic!("expected prototype");
    };
    let param_decl = params[0]
        .declarator
        .as_ref()
        .expect("param declarator expected");
    let DirectDeclaratorKind::Function { inner, .. } = &param_decl.direct.kind else {
        panic!("expected function pointer param");
    };
    let DirectDeclaratorKind::Grouped(grouped) = &inner.kind else {
        panic!("expected grouped");
    };
    assert_eq!(grouped.pointers.len(), 1);
}

#[test]
fn parses_pointer_to_array_parameter() {
    let unit = parse_source("int f(int (*)(), double (*)[3]);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };
    let DirectDeclaratorKind::Function { params, .. } = &decl.declarators[0].declarator.direct.kind
    else {
        panic!("expected function");
    };
    let FunctionParams::Prototype { params, .. } = params else {
        panic!("expected prototype");
    };
    assert_eq!(params.len(), 2);

    let second = params[1]
        .declarator
        .as_ref()
        .expect("second parameter declarator expected");
    let DirectDeclaratorKind::Array { inner, size, .. } = &second.direct.kind else {
        panic!("expected pointer-to-array parameter");
    };
    let DirectDeclaratorKind::Grouped(grouped) = &inner.kind else {
        panic!("expected grouped declarator");
    };
    assert_eq!(grouped.pointers.len(), 1);
    assert_eq!(grouped.direct.as_ref(), &DirectDeclarator::Abstract);
    assert_eq!(size.as_ref(), &ArraySize::Expr(Expr::int(3)));
}

#[test]
fn preserves_unnamed_pointer_parameter() {
    let unit = parse_source("int sum(int, char *);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };
    let DirectDeclaratorKind::Function { params, .. } = &decl.declarators[0].declarator.direct.kind
    else {
        panic!("expected function");
    };
    let FunctionParams::Prototype { params, .. } = params else {
        panic!("expected prototype");
    };
    assert!(params[0].declarator.is_none());
    let second = params[1]
        .declarator
        .as_ref()
        .expect("unnamed char * should keep declarator");
    assert_eq!(second.pointers.len(), 1);
    assert_eq!(second.direct.as_ref(), &DirectDeclarator::Abstract);
}

#[test]
fn rejects_invalid_variadic() {
    assert!(!parse_source_error("int f(, ...);").is_empty());
    assert!(!parse_source_error("int f(void, ...);").is_empty());
}

#[test]
fn parses_function_definition() {
    let unit = parse_source("int main(void) { return 0; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
        panic!("expected function definition");
    };
    assert_eq!(def.specifiers.ty, vec![TypeSpecifier::Int]);
    assert_eq!(def.body.items.len(), 1);
}

#[test]
fn function_def_span_covers_signature_and_body() {
    let src = "int foo(int x) { return x; }";
    let unit = parse_source(src);
    let ExternalDecl::FunctionDef(func) = &unit.items[0] else {
        panic!("expected function definition");
    };
    assert_eq!(func.span.start, 0);
    assert_eq!(func.span.end, src.len());
}

#[test]
fn function_def_span_with_storage_class() {
    let src = "static inline int helper(void) { return 42; }";
    let unit = parse_source(src);
    let ExternalDecl::FunctionDef(func) = &unit.items[0] else {
        panic!("expected function definition");
    };
    assert_eq!(func.span.start, 0);
    assert_eq!(func.span.end, src.len());
}
