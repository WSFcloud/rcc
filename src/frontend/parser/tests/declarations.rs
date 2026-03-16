use super::*;

// Basic declaration tests
#[test]
fn parses_basic_declarations() {
    let cases = [
        ("int a;", TypeSpecifier::Int, 1),
        ("int a = 1, b;", TypeSpecifier::Int, 2),
        ("static int x, y;", TypeSpecifier::Int, 2),
    ];
    for (src, ty, count) in cases {
        let unit = parse_source(src);
        let ExternalDecl::Declaration(decl) = &unit.items[0] else {
            panic!("expected declaration");
        };
        assert_eq!(decl.specifiers.ty, vec![ty]);
        assert_eq!(decl.declarators.len(), count);
    }
}

// Array declarator tests
#[test]
fn parses_array_declarations() {
    let unit = parse_source("int arr[10]; int matrix[2][3];");
    let ExternalDecl::Declaration(decl1) = &unit.items[0] else {
        panic!("expected first declaration");
    };
    let DirectDeclaratorKind::Array { size, .. } = &decl1.declarators[0].declarator.direct.kind
    else {
        panic!("expected array declarator");
    };
    assert_eq!(size.as_ref(), &ArraySize::Expr(Expr::int(10)));

    let ExternalDecl::Declaration(decl2) = &unit.items[1] else {
        panic!("expected second declaration");
    };
    let DirectDeclaratorKind::Array { inner, size, .. } =
        &decl2.declarators[0].declarator.direct.kind
    else {
        panic!("expected outer array");
    };
    assert_eq!(size.as_ref(), &ArraySize::Expr(Expr::int(3)));
    let DirectDeclaratorKind::Array {
        size: inner_size, ..
    } = &inner.kind
    else {
        panic!("expected inner array");
    };
    assert_eq!(inner_size.as_ref(), &ArraySize::Expr(Expr::int(2)));
}

#[test]
fn rejects_vla_marker() {
    let errors = parse_source_error("int arr[*];");
    assert!(!errors.is_empty());
}

// Struct/union tests
#[test]
fn parses_struct_definition() {
    let unit = parse_source("struct Point { int x; int y; };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };
    let TypeSpecifierKind::StructOrUnion(record) = &decl.specifiers.ty[0].kind else {
        panic!("expected record type");
    };
    assert_eq!(record.kind, RecordKind::Struct);
    let members = record.members.as_ref().expect("members expected");
    assert_eq!(members.len(), 2);
}

#[test]
fn parses_struct_with_complex_members() {
    let unit = parse_source("struct Ops { int (*apply)(int, int); int data[2 + 1]; int *next; };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };
    let TypeSpecifierKind::StructOrUnion(record) = &decl.specifiers.ty[0].kind else {
        panic!("expected struct");
    };
    let members = record.members.as_ref().expect("members expected");
    assert_eq!(members.len(), 3);

    // Function pointer member
    let DirectDeclaratorKind::Function { .. } = &members[0].declarators[0].direct.kind else {
        panic!("expected function declarator");
    };

    // Array member
    let DirectDeclaratorKind::Array { size, .. } = &members[1].declarators[0].direct.kind else {
        panic!("expected array");
    };
    assert_eq!(
        size.as_ref(),
        &ArraySize::Expr(Expr::binary(Expr::int(2), BinaryOp::Add, Expr::int(1)))
    );

    // Pointer member
    assert_eq!(members[2].declarators[0].pointers.len(), 1);
}

// Initializer tests
#[test]
fn rejects_invalid_initializers() {
    assert!(!parse_source_error("int a[1] = { = 1 };").is_empty());
    assert!(!parse_source_error("int x = 0; int arr[3] = { [x = 1] = 2 };").is_empty());
}

#[test]
fn parses_designated_initializer() {
    let unit = parse_source("union Value { int i; char c; }; union Value v = { .c = 1 };");
    let ExternalDecl::Declaration(var_decl) = &unit.items[1] else {
        panic!("expected variable declaration");
    };
    let init = var_decl.declarators[0]
        .init
        .as_ref()
        .expect("initializer expected");
    let InitializerKind::Aggregate(items) = &init.kind else {
        panic!("expected aggregate initializer");
    };
    assert_eq!(
        items[0].designators,
        vec![Designator::Field("c".to_string())]
    );
}

// Enum tests
#[test]
fn parses_enum_definition() {
    let unit = parse_source("enum Color { Red, Green = 3, Blue, }; enum Color c = Green;");
    let ExternalDecl::Declaration(enum_decl) = &unit.items[0] else {
        panic!("expected enum declaration");
    };
    let TypeSpecifierKind::Enum(enum_spec) = &enum_decl.specifiers.ty[0].kind else {
        panic!("expected enum specifier");
    };
    let variants = enum_spec.variants.as_ref().expect("variants expected");
    assert_eq!(variants.len(), 3);
    assert_eq!(variants[1].value, Some(Expr::int(3)));
}

#[test]
fn parses_enum_complex_values() {
    let unit = parse_source("enum Bits { A = 1 << 2, B = sizeof(int) };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected enum");
    };
    let TypeSpecifierKind::Enum(enum_spec) = &decl.specifiers.ty[0].kind else {
        panic!("expected enum");
    };
    let variants = enum_spec.variants.as_ref().expect("variants expected");
    assert_eq!(
        variants[0].value,
        Some(Expr::binary(Expr::int(1), BinaryOp::Shl, Expr::int(2)))
    );
}

#[test]
fn parses_enum_value_with_sizeof_abstract_array_type() {
    let unit = parse_source("enum Bits { A = sizeof(int[3]) };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected enum");
    };
    let TypeSpecifierKind::Enum(enum_spec) = &decl.specifiers.ty[0].kind else {
        panic!("expected enum");
    };
    let variants = enum_spec.variants.as_ref().expect("variants expected");
    let Some(value) = &variants[0].value else {
        panic!("expected enum value");
    };
    let ExprKind::SizeofType(ty) = &value.kind else {
        panic!("expected sizeof(type)");
    };
    let declarator = ty.declarator.as_ref().expect("array declarator expected");
    let DirectDeclaratorKind::Array { size, .. } = &declarator.direct.kind else {
        panic!("expected array declarator");
    };
    assert_eq!(size.as_ref(), &ArraySize::Expr(Expr::int(3)));
}

#[test]
fn rejects_enum_redeclaring_typedef() {
    assert!(!parse_source_error("typedef int foo; enum { foo = 1 };").is_empty());
}

// Nested record/enum tests
#[test]
fn parses_nested_definitions() {
    let unit = parse_source(
        "struct Outer { struct Inner { int x; } inner; enum Kind { A = 1 << 2 } kind; };",
    );
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };
    let TypeSpecifierKind::StructOrUnion(outer) = &decl.specifiers.ty[0].kind else {
        panic!("expected outer struct");
    };
    let members = outer.members.as_ref().expect("members expected");
    assert_eq!(members.len(), 2);
}
