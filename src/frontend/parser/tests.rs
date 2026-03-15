use super::*;
use crate::common::token::TokenKind;
use crate::frontend::lexer::lexer_from_source;
use crate::frontend::parser::ast::{ExprKind, Literal};
use crate::frontend::parser::typedefs::ScopeEntry;
use chumsky::input::{Input, Stream};

fn parse_source(src: &str) -> TranslationUnit {
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));

    parse(stream).expect("source should parse")
}

fn parse_tokens(tokens: Vec<TokenKind>) -> TranslationUnit {
    let token_count = tokens.len();
    let stream = Stream::from_iter(tokens.into_iter().enumerate().map(|(idx, token)| {
        let span: Span = (idx..idx + 1).into();
        (token, span)
    }))
    .map((token_count..token_count).into(), |(token, span)| {
        (token, span)
    });

    parse(stream).expect("token stream should parse")
}

fn parse_source_error(src: &str) -> Vec<ParseError<'_>> {
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));

    parse(stream).expect_err("source should fail to parse")
}

fn parse_statement_source(src: &str) -> Stmt {
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));

    parse_statement(stream).expect("statement should parse")
}

fn parse_statement_source_error(src: &str) -> Vec<ParseError<'_>> {
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));

    parse_statement(stream).expect_err("statement should fail to parse")
}

fn parse_block_item_source(src: &str) -> BlockItem {
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));

    parse_block_item(stream).expect("block item should parse")
}

fn assert_direct_ident(direct: &DirectDeclarator, expected: &str) {
    match direct {
        DirectDeclarator::Ident(name) => assert_eq!(name, expected),
        other => panic!("expected identifier declarator, got {other:?}"),
    }
}

fn assert_ident_declarator(init_declarator: &InitDeclarator, expected: &str) {
    assert_direct_ident(init_declarator.declarator.direct.as_ref(), expected);
}

// Declaration and object declarator parsing tests.
#[test]
fn parses_single_int_declaration() {
    let unit = parse_source("int a;");
    assert_eq!(unit.items.len(), 1);

    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
    assert!(decl.specifiers.storage.is_empty());
    assert!(decl.specifiers.qualifiers.is_empty());
    assert_eq!(decl.declarators.len(), 1);
    assert_ident_declarator(&decl.declarators[0], "a");
    assert!(decl.declarators[0].init.is_none());
}

#[test]
fn parses_declaration_with_initializer_and_multiple_declarators() {
    let unit = parse_source("int a = 1, b;");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
    assert_eq!(decl.declarators.len(), 2);

    assert_ident_declarator(&decl.declarators[0], "a");
    let Some(init) = decl.declarators[0].init.as_ref() else {
        panic!("first declarator should contain initializer");
    };
    assert_eq!(init.kind, InitializerKind::Expr(Expr::int(1)));

    assert_ident_declarator(&decl.declarators[1], "b");
    assert!(decl.declarators[1].init.is_none());
}

#[test]
fn parses_static_storage_class_declaration() {
    let unit = parse_source("static int x, y;");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    assert_eq!(decl.specifiers.storage, vec![StorageClass::Static]);
    assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
    assert_eq!(decl.declarators.len(), 2);
    assert_ident_declarator(&decl.declarators[0], "x");
    assert_ident_declarator(&decl.declarators[1], "y");
}

#[test]
fn parses_const_double_declaration() {
    let unit = parse_source("const double pi = 3.14;");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    assert_eq!(decl.specifiers.qualifiers, vec![TypeQualifier::Const]);
    assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Double]);
    assert_eq!(decl.declarators.len(), 1);
    assert_ident_declarator(&decl.declarators[0], "pi");

    let Some(init) = decl.declarators[0].init.as_ref() else {
        panic!("pi should contain initializer");
    };
    match &init.kind {
        InitializerKind::Expr(Expr {
            kind: ExprKind::Literal(Literal::Float(value)),
        }) => assert!((*value - 3.14).abs() < f64::EPSILON),
        other => panic!("expected float initializer, got {other:?}"),
    }
}

#[test]
fn parses_parenthesized_comma_expression_initializer() {
    let unit = parse_source("int value = (1, 2);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    let Some(init) = decl.declarators[0].init.as_ref() else {
        panic!("initializer expected");
    };

    match &init.kind {
        InitializerKind::Expr(Expr {
            kind: ExprKind::Comma { left, right },
        }) => {
            assert_eq!(**left, Expr::int(1));
            assert_eq!(**right, Expr::int(2));
        }
        other => panic!("expected comma expression initializer, got {other:?}"),
    }
}

#[test]
fn parses_array_declaration_with_constant_size() {
    let unit = parse_source("int arr[10];");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    let direct = decl.declarators[0].declarator.direct.as_ref();
    let DirectDeclarator::Array {
        inner,
        qualifiers,
        is_static,
        size,
    } = direct
    else {
        panic!("expected array declarator");
    };
    assert!(qualifiers.is_empty());
    assert!(!is_static);
    assert_eq!(size.as_ref(), &ArraySize::Expr(Expr::int(10)));
    assert_direct_ident(inner.as_ref(), "arr");
}

#[test]
fn parses_multi_dimensional_array_declaration() {
    let unit = parse_source("int matrix[2][3];");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    let direct = decl.declarators[0].declarator.direct.as_ref();
    let DirectDeclarator::Array {
        inner: outer_inner,
        size: outer_size,
        ..
    } = direct
    else {
        panic!("expected outer array declarator");
    };
    assert_eq!(outer_size.as_ref(), &ArraySize::Expr(Expr::int(3)));

    let DirectDeclarator::Array {
        inner: inner_inner,
        size: inner_size,
        ..
    } = outer_inner.as_ref()
    else {
        panic!("expected inner array declarator");
    };
    assert_eq!(inner_size.as_ref(), &ArraySize::Expr(Expr::int(2)));
    assert_direct_ident(inner_inner.as_ref(), "matrix");
}

#[test]
fn rejects_vla_marker_array_declaration() {
    let src = "int arr[*];";
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));

    let errors = parse(stream).expect_err("VLA marker should be rejected");
    assert!(
        !errors.is_empty(),
        "expected at least one parser error for VLA marker syntax"
    );
}

#[test]
fn parses_struct_definition_without_declarator() {
    let unit = parse_source("struct Point { int x; int y; };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };

    assert!(decl.declarators.is_empty());
    assert_eq!(decl.specifiers.ty.len(), 1);
    let TypeSpecifier::StructOrUnion(record) = &decl.specifiers.ty[0] else {
        panic!("expected record type specifier");
    };
    assert_eq!(record.kind, RecordKind::Struct);
    assert_eq!(record.tag.as_deref(), Some("Point"));

    let members = record.members.as_ref().expect("record members expected");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].specifiers.ty, vec![TypeSpecifier::Int]);
    assert_direct_ident(members[0].declarators[0].direct.as_ref(), "x");
    assert_eq!(members[1].specifiers.ty, vec![TypeSpecifier::Int]);
    assert_direct_ident(members[1].declarators[0].direct.as_ref(), "y");
}

#[test]
fn parses_struct_members_with_complex_declarators() {
    let unit = parse_source("struct Ops { int (*apply)(int, int); int data[2 + 1]; int *next; };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };

    let TypeSpecifier::StructOrUnion(record) = &decl.specifiers.ty[0] else {
        panic!("expected struct type specifier");
    };
    assert_eq!(record.kind, RecordKind::Struct);
    let members = record.members.as_ref().expect("record members expected");
    assert_eq!(members.len(), 3);

    let apply_decl = &members[0].declarators[0];
    let DirectDeclarator::Function { inner, params } = apply_decl.direct.as_ref() else {
        panic!("expected function declarator member");
    };
    let DirectDeclarator::Grouped(grouped) = inner.as_ref() else {
        panic!("expected grouped declarator for function pointer member");
    };
    assert_eq!(grouped.pointers.len(), 1);
    assert_direct_ident(grouped.direct.as_ref(), "apply");
    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
    assert_eq!(params[1].specifiers.ty, vec![TypeSpecifier::Int]);

    let data_decl = &members[1].declarators[0];
    let DirectDeclarator::Array { inner, size, .. } = data_decl.direct.as_ref() else {
        panic!("expected array member declarator");
    };
    assert_direct_ident(inner.as_ref(), "data");
    assert_eq!(
        size.as_ref(),
        &ArraySize::Expr(Expr::binary(Expr::int(2), BinaryOp::Add, Expr::int(1)))
    );

    let next_decl = &members[2].declarators[0];
    assert_eq!(next_decl.pointers.len(), 1);
    assert_direct_ident(next_decl.direct.as_ref(), "next");
}

#[test]
fn parses_struct_member_function_pointer_params_with_register_and_qualifiers() {
    let unit = parse_source("struct S { int (*fp)(register int n, const int *p); };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };

    let TypeSpecifier::StructOrUnion(record) = &decl.specifiers.ty[0] else {
        panic!("expected struct type specifier");
    };
    let members = record.members.as_ref().expect("record members expected");
    let fp_decl = &members[0].declarators[0];

    let DirectDeclarator::Function { inner, params } = fp_decl.direct.as_ref() else {
        panic!("expected function declarator member");
    };
    let DirectDeclarator::Grouped(grouped) = inner.as_ref() else {
        panic!("expected grouped declarator for function pointer member");
    };
    assert_eq!(grouped.pointers.len(), 1);
    assert_direct_ident(grouped.direct.as_ref(), "fp");

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].specifiers.storage, vec![StorageClass::Register]);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
    let first_param_decl = params[0]
        .declarator
        .as_ref()
        .expect("first parameter should have declarator");
    assert_direct_ident(first_param_decl.direct.as_ref(), "n");

    assert_eq!(params[1].specifiers.qualifiers, vec![TypeQualifier::Const]);
    assert_eq!(params[1].specifiers.ty, vec![TypeSpecifier::Int]);
    let second_param_decl = params[1]
        .declarator
        .as_ref()
        .expect("second parameter should have declarator");
    assert_eq!(second_param_decl.pointers.len(), 1);
    assert_direct_ident(second_param_decl.direct.as_ref(), "p");
}

#[test]
fn parses_union_members_with_complex_declarators() {
    let unit = parse_source("union Payload { int i; int (*fp)(int); int values[3]; };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };

    let TypeSpecifier::StructOrUnion(record) = &decl.specifiers.ty[0] else {
        panic!("expected union type specifier");
    };
    assert_eq!(record.kind, RecordKind::Union);
    let members = record.members.as_ref().expect("union members expected");
    assert_eq!(members.len(), 3);

    let fp_decl = &members[1].declarators[0];
    let DirectDeclarator::Function { inner, params } = fp_decl.direct.as_ref() else {
        panic!("expected function declarator member");
    };
    let DirectDeclarator::Grouped(grouped) = inner.as_ref() else {
        panic!("expected grouped declarator for function pointer member");
    };
    assert_eq!(grouped.pointers.len(), 1);
    assert_direct_ident(grouped.direct.as_ref(), "fp");
    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);

    let values_decl = &members[2].declarators[0];
    let DirectDeclarator::Array { inner, size, .. } = values_decl.direct.as_ref() else {
        panic!("expected array member declarator");
    };
    assert_direct_ident(inner.as_ref(), "values");
    assert_eq!(size.as_ref(), &ArraySize::Expr(Expr::int(3)));
}

#[test]
fn parses_member_array_size_with_identifier_constant_expression() {
    let unit = parse_source("struct Buffer { int data[BUFSIZE]; };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };

    let TypeSpecifier::StructOrUnion(record) = &decl.specifiers.ty[0] else {
        panic!("expected struct type specifier");
    };
    let members = record.members.as_ref().expect("record members expected");
    let data_decl = &members[0].declarators[0];
    let DirectDeclarator::Array { inner, size, .. } = data_decl.direct.as_ref() else {
        panic!("expected array member declarator");
    };
    assert_direct_ident(inner.as_ref(), "data");
    assert_eq!(
        size.as_ref(),
        &ArraySize::Expr(Expr::var("BUFSIZE".to_string()))
    );
}

#[test]
fn rejects_initializer_with_empty_designator_before_assign() {
    let errors = parse_source_error("int a[1] = { = 1 };");
    assert!(
        !errors.is_empty(),
        "initializer item should require at least one designator before '='"
    );
}

#[test]
fn rejects_designator_index_with_assignment_expression() {
    let errors = parse_source_error("int x = 0; int arr[3] = { [x = 1] = 2 };");
    assert!(
        !errors.is_empty(),
        "designator index should parse as constant-expression, not assignment-expression"
    );
}

#[test]
fn parses_union_designated_initializer() {
    let unit = parse_source("union Value { int i; char c; }; union Value v = { .c = 1 };");
    assert_eq!(unit.items.len(), 2);

    let ExternalDecl::Declaration(ty_decl) = &unit.items[0] else {
        panic!("expected record declaration");
    };
    let TypeSpecifier::StructOrUnion(record) = &ty_decl.specifiers.ty[0] else {
        panic!("expected union type specifier");
    };
    assert_eq!(record.kind, RecordKind::Union);
    assert_eq!(record.tag.as_deref(), Some("Value"));

    let ExternalDecl::Declaration(var_decl) = &unit.items[1] else {
        panic!("expected variable declaration");
    };
    assert_eq!(var_decl.declarators.len(), 1);

    let init = var_decl.declarators[0]
        .init
        .as_ref()
        .expect("initializer should exist");
    let InitializerKind::Aggregate(items) = &init.kind else {
        panic!("expected aggregate initializer");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].designators,
        vec![Designator::Field("c".to_string())]
    );
    assert_eq!(items[0].init.kind, InitializerKind::Expr(Expr::int(1)));
}

#[test]
fn parses_nested_record_and_enum_definitions_in_members() {
    let unit = parse_source(
        "struct Outer { struct Inner { int x; } inner; enum Kind { A = 1 << 2, B } kind; };",
    );
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };
    let TypeSpecifier::StructOrUnion(outer) = &decl.specifiers.ty[0] else {
        panic!("expected outer struct");
    };
    let members = outer.members.as_ref().expect("outer members expected");
    assert_eq!(members.len(), 2);

    let TypeSpecifier::StructOrUnion(inner_spec) = &members[0].specifiers.ty[0] else {
        panic!("expected nested struct specifier");
    };
    assert_eq!(inner_spec.tag.as_deref(), Some("Inner"));
    let inner_members = inner_spec
        .members
        .as_ref()
        .expect("nested struct members expected");
    assert_eq!(inner_members.len(), 1);
    assert_eq!(inner_members[0].specifiers.ty, vec![TypeSpecifier::Int]);
    assert_direct_ident(inner_members[0].declarators[0].direct.as_ref(), "x");
    assert_direct_ident(members[0].declarators[0].direct.as_ref(), "inner");

    let TypeSpecifier::Enum(kind_spec) = &members[1].specifiers.ty[0] else {
        panic!("expected nested enum specifier");
    };
    assert_eq!(kind_spec.tag.as_deref(), Some("Kind"));
    let variants = kind_spec
        .variants
        .as_ref()
        .expect("nested enum variants expected");
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0].name, "A");
    assert_eq!(
        variants[0].value,
        Some(Expr::binary(Expr::int(1), BinaryOp::Shl, Expr::int(2)))
    );
    assert_eq!(variants[1].name, "B");
    assert_direct_ident(members[1].declarators[0].direct.as_ref(), "kind");
}

#[test]
fn parses_enum_definition_and_initializer_use() {
    let unit = parse_source("enum Color { Red, Green = 3, Blue, }; enum Color c = Green;");
    assert_eq!(unit.items.len(), 2);

    let ExternalDecl::Declaration(enum_decl) = &unit.items[0] else {
        panic!("expected enum declaration");
    };
    assert!(enum_decl.declarators.is_empty());
    let TypeSpecifier::Enum(enum_spec) = &enum_decl.specifiers.ty[0] else {
        panic!("expected enum specifier");
    };
    assert_eq!(enum_spec.tag.as_deref(), Some("Color"));
    let variants = enum_spec.variants.as_ref().expect("enum variants expected");
    assert_eq!(variants.len(), 3);
    assert_eq!(variants[0].name, "Red");
    assert_eq!(variants[0].value, None);
    assert_eq!(variants[1].name, "Green");
    assert_eq!(variants[1].value, Some(Expr::int(3)));
    assert_eq!(variants[2].name, "Blue");

    let ExternalDecl::Declaration(var_decl) = &unit.items[1] else {
        panic!("expected enum variable declaration");
    };
    let init = var_decl.declarators[0]
        .init
        .as_ref()
        .expect("enum variable initializer expected");
    assert_eq!(
        init.kind,
        InitializerKind::Expr(Expr::var("Green".to_string()))
    );
}

#[test]
fn parses_enum_value_constant_expressions() {
    let unit = parse_source("enum Bits { A = 1 << 2, B = A + 3, C = (B & 7), D = sizeof(int) };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected enum declaration");
    };
    let TypeSpecifier::Enum(enum_spec) = &decl.specifiers.ty[0] else {
        panic!("expected enum specifier");
    };
    let variants = enum_spec.variants.as_ref().expect("enum variants expected");
    assert_eq!(variants.len(), 4);
    assert_eq!(
        variants[0].value,
        Some(Expr::binary(Expr::int(1), BinaryOp::Shl, Expr::int(2)))
    );
    assert_eq!(
        variants[1].value,
        Some(Expr::binary(
            Expr::var("A".to_string()),
            BinaryOp::Add,
            Expr::int(3)
        ))
    );
    assert_eq!(
        variants[2].value,
        Some(Expr::binary(
            Expr::var("B".to_string()),
            BinaryOp::BitAnd,
            Expr::int(7)
        ))
    );
    let Some(Expr {
        kind: ExprKind::SizeofType(ty),
    }) = variants[3].value.as_ref()
    else {
        panic!("expected sizeof(type-name) enum value");
    };
    assert_eq!(ty.specifiers.ty, vec![TypeSpecifier::Int]);
}

#[test]
fn parses_enum_value_character_literals() {
    let unit = parse_tokens(vec![
        TokenKind::Enum,
        TokenKind::LBrace,
        TokenKind::Identifier("LETTER_A".to_string()),
        TokenKind::Assign,
        TokenKind::CharLiteral('A'),
        TokenKind::Comma,
        TokenKind::Identifier("LETTER_B".to_string()),
        TokenKind::Assign,
        TokenKind::CharLiteral('B'),
        TokenKind::RBrace,
        TokenKind::Semicolon,
    ]);
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected enum declaration");
    };
    let TypeSpecifier::Enum(enum_spec) = &decl.specifiers.ty[0] else {
        panic!("expected enum specifier");
    };
    let variants = enum_spec.variants.as_ref().expect("enum variants expected");
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0].name, "LETTER_A");
    assert_eq!(variants[0].value, Some(Expr::char('A')));
    assert_eq!(variants[1].name, "LETTER_B");
    assert_eq!(variants[1].value, Some(Expr::char('B')));
}

#[test]
fn parses_enum_sizeof_with_multi_specifier_type_name() {
    let unit = parse_source("enum Sizes { A = sizeof(unsigned long), B = sizeof(const int *) };");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected enum declaration");
    };
    let TypeSpecifier::Enum(enum_spec) = &decl.specifiers.ty[0] else {
        panic!("expected enum specifier");
    };
    let variants = enum_spec.variants.as_ref().expect("enum variants expected");
    assert_eq!(variants.len(), 2);

    let Some(Expr {
        kind: ExprKind::SizeofType(ty),
    }) = variants[0].value.as_ref()
    else {
        panic!("expected sizeof(type-name) for first variant");
    };
    assert_eq!(
        ty.specifiers.ty,
        vec![TypeSpecifier::Unsigned, TypeSpecifier::Long]
    );
    assert!(ty.specifiers.qualifiers.is_empty());
    assert!(ty.declarator.is_none());

    let Some(Expr {
        kind: ExprKind::SizeofType(ty),
    }) = variants[1].value.as_ref()
    else {
        panic!("expected sizeof(type-name) for second variant");
    };
    assert_eq!(ty.specifiers.qualifiers, vec![TypeQualifier::Const]);
    assert_eq!(ty.specifiers.ty, vec![TypeSpecifier::Int]);
    let declarator = ty
        .declarator
        .as_ref()
        .expect("pointer abstract declarator expected");
    assert_eq!(declarator.pointers.len(), 1);
    assert_eq!(declarator.direct.as_ref(), &DirectDeclarator::Abstract);
}

#[test]
fn rejects_enum_enumerator_redeclaring_typedef_name() {
    let errors = parse_source_error("typedef int foo; enum { foo = 1 };");
    assert!(
        !errors.is_empty(),
        "enum enumerator should not be allowed to redeclare typedef-name"
    );
}

#[test]
fn parses_struct_aggregate_init_and_member_access() {
    let unit = parse_source(
        "struct Point { int x; int y; }; int f(void) { struct Point p = {1, 2}; return p.x; }",
    );
    assert_eq!(unit.items.len(), 2);

    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function definition");
    };
    assert_eq!(def.body.items.len(), 2);

    let BlockItem::Decl(decl) = &def.body.items[0] else {
        panic!("expected declaration in function body");
    };
    let init = decl.declarators[0]
        .init
        .as_ref()
        .expect("struct initializer expected");
    let InitializerKind::Aggregate(items) = &init.kind else {
        panic!("expected aggregate initializer");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].init.kind, InitializerKind::Expr(Expr::int(1)));
    assert_eq!(items[1].init.kind, InitializerKind::Expr(Expr::int(2)));

    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[1] else {
        panic!("expected return statement");
    };
    assert_eq!(
        *expr,
        Expr::member(Expr::var("p".to_string()), "x".to_string(), false)
    );
}

// Statement and block-item basic parsing tests.
#[test]
fn parses_empty_expression_statement() {
    let stmt = parse_statement_source(";");
    assert_eq!(stmt, Stmt::Empty);
}

#[test]
fn parses_non_empty_expression_statement() {
    let stmt = parse_statement_source("a = 1 + 2;");
    assert_eq!(
        stmt,
        Stmt::Expr(Expr::assign(
            Expr::var("a".to_string()),
            AssignOp::Assign,
            Expr::binary(Expr::int(1), BinaryOp::Add, Expr::int(2)),
        ))
    );
}

#[test]
fn parses_declaration_block_item() {
    let item = parse_block_item_source("int counter;");
    let BlockItem::Decl(decl) = item else {
        panic!("expected declaration block item");
    };

    assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
    assert_eq!(decl.declarators.len(), 1);
    assert_ident_declarator(&decl.declarators[0], "counter");
}

#[test]
fn parses_statement_block_item() {
    let item = parse_block_item_source("counter++;");
    assert_eq!(
        item,
        BlockItem::Stmt(Stmt::Expr(Expr::post_inc(Expr::var("counter".to_string()))))
    );
}

// Function declarator and function definition parsing tests.
#[test]
fn parses_function_declaration_with_void_params() {
    let unit = parse_source("int main(void);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    let direct = decl.declarators[0].declarator.direct.as_ref();
    let DirectDeclarator::Function { inner, params } = direct else {
        panic!("expected function declarator");
    };
    assert_direct_ident(inner.as_ref(), "main");
    assert_eq!(
        params,
        &FunctionParams::Prototype {
            params: Vec::new(),
            variadic: false
        }
    );
}

#[test]
fn parses_function_declaration_with_named_params() {
    let unit = parse_source("int sum(int x, char *p);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    let direct = decl.declarators[0].declarator.direct.as_ref();
    let DirectDeclarator::Function { inner, params } = direct else {
        panic!("expected function declarator");
    };
    assert_direct_ident(inner.as_ref(), "sum");

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype params");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
    assert_eq!(params[1].specifiers.ty, vec![TypeSpecifier::Char]);

    let first = params[0]
        .declarator
        .as_ref()
        .expect("first parameter should have declarator");
    assert_direct_ident(first.direct.as_ref(), "x");

    let second = params[1]
        .declarator
        .as_ref()
        .expect("second parameter should have declarator");
    assert_eq!(second.pointers.len(), 1);
    assert_direct_ident(second.direct.as_ref(), "p");
}

#[test]
fn parses_function_declaration_with_array_param() {
    let unit = parse_source("int f(int a[]);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    let direct = decl.declarators[0].declarator.direct.as_ref();
    let DirectDeclarator::Function { params, .. } = direct else {
        panic!("expected function declarator");
    };

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype params");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 1);

    let param_decl = params[0]
        .declarator
        .as_ref()
        .expect("parameter declarator expected");

    let DirectDeclarator::Array { inner, size, .. } = param_decl.direct.as_ref() else {
        panic!("expected array declarator for parameter");
    };
    assert_eq!(size.as_ref(), &ArraySize::Unspecified);
    assert_direct_ident(inner.as_ref(), "a");
}

#[test]
fn parses_function_declaration_with_const_char_pointer_array_param() {
    let unit = parse_source("void p(const char *strings[], int count);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    let direct = decl.declarators[0].declarator.direct.as_ref();
    let DirectDeclarator::Function { inner, params } = direct else {
        panic!("expected function declarator");
    };
    assert_direct_ident(inner.as_ref(), "p");

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype params");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 2);

    assert_eq!(params[0].specifiers.qualifiers, vec![TypeQualifier::Const]);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Char]);

    let first = params[0]
        .declarator
        .as_ref()
        .expect("first parameter declarator expected");
    assert_eq!(first.pointers.len(), 1);

    let DirectDeclarator::Array { inner, size, .. } = first.direct.as_ref() else {
        panic!("expected array declarator for first parameter");
    };
    assert_eq!(size.as_ref(), &ArraySize::Unspecified);
    assert_direct_ident(inner.as_ref(), "strings");

    assert_eq!(params[1].specifiers.ty, vec![TypeSpecifier::Int]);
    let second = params[1]
        .declarator
        .as_ref()
        .expect("second parameter declarator expected");
    assert_direct_ident(second.direct.as_ref(), "count");
}

#[test]
fn parses_grouped_function_pointer_declaration() {
    let unit = parse_source("int (*fp)(int);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    let direct = decl.declarators[0].declarator.direct.as_ref();
    let DirectDeclarator::Function { inner, params } = direct else {
        panic!("expected function declarator");
    };

    let DirectDeclarator::Grouped(grouped_decl) = inner.as_ref() else {
        panic!("expected grouped declarator as function inner");
    };
    assert_eq!(grouped_decl.pointers.len(), 1);
    assert_direct_ident(grouped_decl.direct.as_ref(), "fp");

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameters");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
}

#[test]
fn preserves_pointer_layers_for_unnamed_parameter_declarator() {
    let unit = parse_source("int sum(int, char *);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration item");
    };

    let direct = decl.declarators[0].declarator.direct.as_ref();
    let DirectDeclarator::Function { params, .. } = direct else {
        panic!("expected function declarator");
    };

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype params");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 2);

    assert!(
        params[0].declarator.is_none(),
        "plain unnamed `int` parameter should have no declarator"
    );

    let second = params[1]
        .declarator
        .as_ref()
        .expect("unnamed `char *` should keep declarator");
    assert_eq!(second.pointers.len(), 1);
    assert_eq!(second.direct.as_ref(), &DirectDeclarator::Abstract);
}

#[test]
fn parses_function_definition_with_compound_body() {
    let unit = parse_source("int main(void) { return 0; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
        panic!("expected function definition");
    };

    assert_eq!(def.specifiers.ty, vec![TypeSpecifier::Int]);
    let DirectDeclarator::Function { inner, .. } = def.declarator.direct.as_ref() else {
        panic!("expected function declarator");
    };
    assert_direct_ident(inner.as_ref(), "main");
    assert!(def.declarations.is_empty());
    assert_eq!(def.body.items.len(), 1);
    assert_eq!(
        def.body.items[0],
        BlockItem::Stmt(Stmt::Return(Some(Expr::int(0))))
    );
}

// Control-flow statement parsing tests.
#[test]
fn parses_compound_statement_with_decl_and_expr_stmt() {
    let stmt = parse_statement_source("{ int x; x = 1; }");
    let Stmt::Compound(compound) = stmt else {
        panic!("expected compound statement");
    };

    assert_eq!(compound.items.len(), 2);
    let BlockItem::Decl(decl) = &compound.items[0] else {
        panic!("first item should be declaration");
    };
    assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
    assert_ident_declarator(&decl.declarators[0], "x");

    assert_eq!(
        compound.items[1],
        BlockItem::Stmt(Stmt::Expr(Expr::assign(
            Expr::var("x".to_string()),
            AssignOp::Assign,
            Expr::int(1),
        )))
    );
}

#[test]
fn parses_return_statements() {
    assert_eq!(parse_statement_source("return;"), Stmt::Return(None));
    assert_eq!(
        parse_statement_source("return x + 1;"),
        Stmt::Return(Some(Expr::binary(
            Expr::var("x".to_string()),
            BinaryOp::Add,
            Expr::int(1),
        )))
    );
}

#[test]
fn parses_break_statement() {
    assert_eq!(parse_statement_source("break;"), Stmt::Break);
}

#[test]
fn parses_continue_statement() {
    assert_eq!(parse_statement_source("continue;"), Stmt::Continue);
}

#[test]
fn parses_if_else_statement() {
    let stmt = parse_statement_source("if (flag) x = 1; else x = 2;");
    assert_eq!(
        stmt,
        Stmt::If {
            cond: Expr::var("flag".to_string()),
            then_branch: Box::new(Stmt::Expr(Expr::assign(
                Expr::var("x".to_string()),
                AssignOp::Assign,
                Expr::int(1),
            ))),
            else_branch: Some(Box::new(Stmt::Expr(Expr::assign(
                Expr::var("x".to_string()),
                AssignOp::Assign,
                Expr::int(2),
            )))),
        }
    );
}

#[test]
fn parses_while_statement() {
    let stmt = parse_statement_source("while (x < 10) x++;");
    assert_eq!(
        stmt,
        Stmt::While {
            cond: Expr::binary(Expr::var("x".to_string()), BinaryOp::Lt, Expr::int(10)),
            body: Box::new(Stmt::Expr(Expr::post_inc(Expr::var("x".to_string())))),
        }
    );
}

#[test]
fn parses_do_while_statement() {
    let stmt = parse_statement_source("do x++; while (x < 10);");
    assert_eq!(
        stmt,
        Stmt::DoWhile {
            body: Box::new(Stmt::Expr(Expr::post_inc(Expr::var("x".to_string())))),
            cond: Expr::binary(Expr::var("x".to_string()), BinaryOp::Lt, Expr::int(10)),
        }
    );
}

#[test]
fn parses_switch_statement() {
    let stmt = parse_statement_source("switch (x) break;");
    assert_eq!(
        stmt,
        Stmt::Switch {
            expr: Expr::var("x".to_string()),
            body: Box::new(Stmt::Break),
        }
    );
}

#[test]
fn parses_case_statement() {
    let stmt = parse_statement_source("case 1: break;");
    assert_eq!(
        stmt,
        Stmt::Case {
            expr: Expr::int(1),
            stmt: Box::new(Stmt::Break),
        }
    );
}

#[test]
fn rejects_case_statement_with_comma_expression() {
    let errors = parse_statement_source_error("case 1, 2: break;");
    assert!(
        !errors.is_empty(),
        "case label should reject top-level comma expression"
    );
}

#[test]
fn rejects_case_statement_with_assignment_expression() {
    let errors = parse_statement_source_error("case y = 1: break;");
    assert!(
        !errors.is_empty(),
        "case label should reject assignment expression"
    );
}

#[test]
fn rejects_case_statement_with_function_call_expression() {
    let errors = parse_statement_source_error("case foo(): break;");
    assert!(
        !errors.is_empty(),
        "case label should reject non-constant call expression"
    );
}

#[test]
fn parses_default_statement() {
    let stmt = parse_statement_source("default: continue;");
    assert_eq!(
        stmt,
        Stmt::Default {
            stmt: Box::new(Stmt::Continue),
        }
    );
}

#[test]
fn parses_label_statement() {
    let stmt = parse_statement_source("entry: x = 1;");
    assert_eq!(
        stmt,
        Stmt::Label {
            label: "entry".to_string(),
            stmt: Box::new(Stmt::Expr(Expr::assign(
                Expr::var("x".to_string()),
                AssignOp::Assign,
                Expr::int(1),
            ))),
        }
    );
}

#[test]
fn parses_goto_statement() {
    let stmt = parse_statement_source("goto entry;");
    assert_eq!(stmt, Stmt::Goto("entry".to_string()));
}

#[test]
fn parses_for_statement_with_expression_init() {
    let stmt = parse_statement_source("for (i = 0; i < 10; i++) i;");
    assert_eq!(
        stmt,
        Stmt::For {
            init: Some(ForInit::Expr(Expr::assign(
                Expr::var("i".to_string()),
                AssignOp::Assign,
                Expr::int(0),
            ))),
            cond: Some(Expr::binary(
                Expr::var("i".to_string()),
                BinaryOp::Lt,
                Expr::int(10),
            )),
            step: Some(Expr::post_inc(Expr::var("i".to_string()))),
            body: Box::new(Stmt::Expr(Expr::var("i".to_string()))),
        }
    );
}

#[test]
fn parses_for_statement_with_declaration_init() {
    let stmt = parse_statement_source("for (int i = 0; i < 3; i++) ;");
    let Stmt::For {
        init,
        cond,
        step,
        body,
    } = stmt
    else {
        panic!("expected for statement");
    };

    let Some(ForInit::Decl(decl)) = init else {
        panic!("for init should be declaration");
    };
    assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
    assert_eq!(decl.declarators.len(), 1);
    assert_ident_declarator(&decl.declarators[0], "i");
    assert_eq!(
        decl.declarators[0]
            .init
            .as_ref()
            .expect("initializer expected")
            .kind,
        InitializerKind::Expr(Expr::int(0))
    );

    assert_eq!(
        cond,
        Some(Expr::binary(
            Expr::var("i".to_string()),
            BinaryOp::Lt,
            Expr::int(3),
        ))
    );
    assert_eq!(step, Some(Expr::post_inc(Expr::var("i".to_string()))));
    assert_eq!(*body, Stmt::Empty);
}

// Postfix expression statement parsing tests.
#[test]
fn parses_function_call_expression_statement() {
    let stmt = parse_statement_source("result = add(1, 2);");
    assert_eq!(
        stmt,
        Stmt::Expr(Expr::assign(
            Expr::var("result".to_string()),
            AssignOp::Assign,
            Expr::call(
                Expr::var("add".to_string()),
                vec![Expr::int(1), Expr::int(2)]
            ),
        ))
    );
}

#[test]
fn parses_empty_argument_function_call() {
    let stmt = parse_statement_source("result = get();");
    assert_eq!(
        stmt,
        Stmt::Expr(Expr::assign(
            Expr::var("result".to_string()),
            AssignOp::Assign,
            Expr::call(Expr::var("get".to_string()), vec![]),
        ))
    );
}

#[test]
fn parses_chained_function_call() {
    let stmt = parse_statement_source("result = factory()(42);");
    assert_eq!(
        stmt,
        Stmt::Expr(Expr::assign(
            Expr::var("result".to_string()),
            AssignOp::Assign,
            Expr::call(
                Expr::call(Expr::var("factory".to_string()), vec![]),
                vec![Expr::int(42)],
            ),
        ))
    );
}

#[test]
fn parses_grouped_comma_expression_as_single_call_argument() {
    let stmt = parse_statement_source("result = f((1, 2));");
    assert_eq!(
        stmt,
        Stmt::Expr(Expr::assign(
            Expr::var("result".to_string()),
            AssignOp::Assign,
            Expr::call(
                Expr::var("f".to_string()),
                vec![Expr::comma(Expr::int(1), Expr::int(2))],
            ),
        ))
    );
}

#[test]
fn parses_array_subscript_expression_statement() {
    let stmt = parse_statement_source("value = arr[i + 1];");
    assert_eq!(
        stmt,
        Stmt::Expr(Expr::assign(
            Expr::var("value".to_string()),
            AssignOp::Assign,
            Expr::index(
                Expr::var("arr".to_string()),
                Expr::binary(Expr::var("i".to_string()), BinaryOp::Add, Expr::int(1)),
            ),
        ))
    );
}

#[test]
fn parses_member_access_expression_statement() {
    let stmt = parse_statement_source("value = point.x;");
    assert_eq!(
        stmt,
        Stmt::Expr(Expr::assign(
            Expr::var("value".to_string()),
            AssignOp::Assign,
            Expr::member(Expr::var("point".to_string()), "x".to_string(), false),
        ))
    );
}

#[test]
fn parses_pointer_member_access_expression_statement() {
    let stmt = parse_statement_source("value = node->next;");
    assert_eq!(
        stmt,
        Stmt::Expr(Expr::assign(
            Expr::var("value".to_string()),
            AssignOp::Assign,
            Expr::member(Expr::var("node".to_string()), "next".to_string(), true),
        ))
    );
}

#[test]
fn parses_chained_postfix_access_expression_statement() {
    let stmt = parse_statement_source("value = factory().items[i].count;");
    assert_eq!(
        stmt,
        Stmt::Expr(Expr::assign(
            Expr::var("value".to_string()),
            AssignOp::Assign,
            Expr::member(
                Expr::index(
                    Expr::member(
                        Expr::call(Expr::var("factory".to_string()), vec![]),
                        "items".to_string(),
                        false,
                    ),
                    Expr::var("i".to_string()),
                ),
                "count".to_string(),
                false,
            ),
        ))
    );
}

// Typedef-name resolution and scope interaction tests.
#[test]
fn parses_typedef_and_uses_typedef_name_in_later_declaration() {
    let unit = parse_source("typedef int T; T x;");
    assert_eq!(unit.items.len(), 2);

    let ExternalDecl::Declaration(typedef_decl) = &unit.items[0] else {
        panic!("expected typedef declaration");
    };
    assert_eq!(typedef_decl.specifiers.storage, vec![StorageClass::Typedef]);
    assert_ident_declarator(&typedef_decl.declarators[0], "T");

    let ExternalDecl::Declaration(var_decl) = &unit.items[1] else {
        panic!("expected declaration using typedef-name");
    };
    assert_eq!(
        var_decl.specifiers.ty,
        vec![TypeSpecifier::TypedefName("T".to_string())]
    );
    assert_ident_declarator(&var_decl.declarators[0], "x");
}

#[test]
fn parses_cast_expression_using_typedef_name() {
    let unit = parse_source("typedef int T; int f(void) { return (T)+1; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function definition");
    };

    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
        panic!("expected return statement");
    };

    let ExprKind::Cast { ty, expr } = &expr.kind else {
        panic!("expected cast expression");
    };
    assert_eq!(
        ty.specifiers.ty,
        vec![TypeSpecifier::TypedefName("T".to_string())]
    );
    assert!(ty.declarator.is_none());

    let ExprKind::Unary {
        op: UnaryOp::Plus,
        expr: cast_inner,
    } = &expr.kind
    else {
        panic!("expected unary plus inside cast expression");
    };
    assert_eq!(**cast_inner, Expr::int(1));
}

#[test]
fn parses_sizeof_type_and_sizeof_expr() {
    let unit = parse_source("typedef int T; int f(void) { return sizeof(T) + sizeof(x); }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function definition");
    };

    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
        panic!("expected return statement");
    };

    let ExprKind::Binary {
        left,
        op: BinaryOp::Add,
        right,
    } = &expr.kind
    else {
        panic!("expected binary add expression");
    };

    let ExprKind::SizeofType(ty) = &left.kind else {
        panic!("expected sizeof(type-name) on left");
    };
    assert_eq!(
        ty.specifiers.ty,
        vec![TypeSpecifier::TypedefName("T".to_string())]
    );

    let ExprKind::SizeofExpr(inner) = &right.kind else {
        panic!("expected sizeof(expr) on right");
    };
    assert_eq!(**inner, Expr::var("x".to_string()));
}

#[test]
fn ordinary_identifier_shadows_typedef_name_in_inner_scope() {
    let unit = parse_source("typedef int T; void f(void) { int T; T = 1; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function definition");
    };
    assert_eq!(def.body.items.len(), 2);

    let BlockItem::Decl(inner_decl) = &def.body.items[0] else {
        panic!("expected declaration in function body");
    };
    assert_ident_declarator(&inner_decl.declarators[0], "T");

    assert_eq!(
        def.body.items[1],
        BlockItem::Stmt(Stmt::Expr(Expr::assign(
            Expr::var("T".to_string()),
            AssignOp::Assign,
            Expr::int(1),
        )))
    );
}

#[test]
fn rejects_conflicting_same_scope_typedef_and_ordinary_declaration() {
    let errors = parse_source_error("typedef int T; int T;");
    assert!(
        !errors.is_empty(),
        "same-scope typedef/object name conflict should be rejected"
    );
}

#[test]
fn conflicting_same_scope_declaration_does_not_pollute_typedef_disambiguation() {
    let src = "typedef int T; int T; T x;";
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));

    let mut state = Typedefs::default();
    let result = parser().parse_with_state(stream, &mut state).into_result();
    assert!(
        result.is_err(),
        "conflicting same-scope declaration should fail parsing"
    );
    assert!(
        state.is_typedef_name("T"),
        "typedef binding should be preserved after conflict, entries = {:?}",
        state.entries()
    );
}

#[test]
fn enum_enumerator_shadows_outer_scope_typedef_name() {
    let unit = parse_source("typedef int T; int f(void) { enum { T = 42 }; return T; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function definition");
    };
    assert_eq!(def.body.items.len(), 2);

    let BlockItem::Decl(enum_decl) = &def.body.items[0] else {
        panic!("expected enum declaration in function body");
    };
    let TypeSpecifier::Enum(enum_spec) = &enum_decl.specifiers.ty[0] else {
        panic!("expected enum specifier in function body");
    };
    let variants = enum_spec.variants.as_ref().expect("enum variants expected");
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0].name, "T");
    assert_eq!(variants[0].value, Some(Expr::int(42)));

    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[1] else {
        panic!("expected return statement");
    };
    assert_eq!(*expr, Expr::var("T".to_string()));
}

#[test]
fn typedef_name_works_in_for_declaration_init() {
    let unit = parse_source("typedef int T; void f(void) { for (T i = 0; i < 1; i++) {} }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function definition");
    };
    let BlockItem::Stmt(Stmt::For {
        init,
        cond,
        step,
        body,
    }) = &def.body.items[0]
    else {
        panic!("expected for statement");
    };

    let Some(ForInit::Decl(decl)) = init else {
        panic!("expected declaration init in for statement");
    };
    assert_eq!(
        decl.specifiers.ty,
        vec![TypeSpecifier::TypedefName("T".to_string())]
    );
    assert_ident_declarator(&decl.declarators[0], "i");

    assert_eq!(
        cond,
        &Some(Expr::binary(
            Expr::var("i".to_string()),
            BinaryOp::Lt,
            Expr::int(1),
        ))
    );
    assert_eq!(step, &Some(Expr::post_inc(Expr::var("i".to_string()))));
    assert_eq!(**body, Stmt::Compound(CompoundStmt { items: Vec::new() }));
}

#[test]
fn parameter_name_shadows_typedef_name_in_function_body() {
    let unit = parse_source("typedef int T; int f(T T) { T = 1; return T; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[1] else {
        panic!("expected function definition");
    };

    let DirectDeclarator::Function { params, .. } = def.declarator.direct.as_ref() else {
        panic!("expected function declarator");
    };
    let FunctionParams::Prototype { params, .. } = params else {
        panic!("expected prototype parameter list");
    };
    assert_eq!(params.len(), 1);
    assert_eq!(
        params[0].specifiers.ty,
        vec![TypeSpecifier::TypedefName("T".to_string())]
    );
    let param_decl = params[0]
        .declarator
        .as_ref()
        .expect("parameter name should be present");
    assert_direct_ident(param_decl.direct.as_ref(), "T");

    assert_eq!(
        def.body.items[0],
        BlockItem::Stmt(Stmt::Expr(Expr::assign(
            Expr::var("T".to_string()),
            AssignOp::Assign,
            Expr::int(1),
        )))
    );
    assert_eq!(
        def.body.items[1],
        BlockItem::Stmt(Stmt::Return(Some(Expr::var("T".to_string()))))
    );
}

#[test]
fn typedef_visibility_survives_function_definition() {
    let src = "typedef int T; void test() {}";
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));

    let mut state = Typedefs::default();
    parser()
        .parse_with_state(stream, &mut state)
        .into_result()
        .expect("source should parse");

    assert!(
        state.is_typedef_name("T"),
        "typedef should remain visible, entries = {:?}",
        state.entries()
    );
}

#[test]
fn function_definition_uses_single_scope_for_params_and_outer_body() {
    let src = "int f(int x) { int y; return x; }";
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));

    let mut state = Typedefs::default();
    parser()
        .parse_with_state(stream, &mut state)
        .into_result()
        .expect("source should parse");

    let scope_starts = state
        .entries()
        .iter()
        .filter(|entry| matches!(entry, ScopeEntry::ScopeStart))
        .count();
    let scope_ends = state
        .entries()
        .iter()
        .filter(|entry| matches!(entry, ScopeEntry::ScopeEnd))
        .count();
    assert_eq!(
        scope_starts, 1,
        "function definition should enter only one scope"
    );
    assert_eq!(scope_ends, 1, "function definition should exit one scope");
}

#[test]
fn function_name_shadows_typedef_inside_body() {
    let errors = parse_source_error(
        "typedef int f;\n\
             f f(void) {\n\
                 f x;\n\
                 return x;\n\
             }",
    );
    assert!(
        !errors.is_empty(),
        "function name should hide typedef-name inside body"
    );
}

// Type-name and abstract declarator parsing tests.
#[test]
fn parses_sizeof_with_abstract_array_type_name() {
    let unit = parse_source("int f(void) { return sizeof(int [10]); }");
    let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
        panic!("expected function definition");
    };

    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
        panic!("expected return statement");
    };

    let ExprKind::SizeofType(ty) = &expr.kind else {
        panic!("expected sizeof(type-name)");
    };
    assert_eq!(ty.specifiers.ty, vec![TypeSpecifier::Int]);

    let declarator = ty
        .declarator
        .as_ref()
        .expect("abstract array declarator should be present");
    let DirectDeclarator::Array { inner, size, .. } = declarator.direct.as_ref() else {
        panic!("expected array abstract declarator");
    };
    assert_eq!(inner.as_ref(), &DirectDeclarator::Abstract);
    assert_eq!(size.as_ref(), &ArraySize::Expr(Expr::int(10)));
}

#[test]
fn parses_sizeof_with_abstract_array_type_name_constant_expression_size() {
    let unit = parse_source("int f(void) { return sizeof(int [2 + 1]); }");
    let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
        panic!("expected function definition");
    };

    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
        panic!("expected return statement");
    };

    let ExprKind::SizeofType(ty) = &expr.kind else {
        panic!("expected sizeof(type-name)");
    };
    assert_eq!(ty.specifiers.ty, vec![TypeSpecifier::Int]);

    let declarator = ty
        .declarator
        .as_ref()
        .expect("abstract array declarator should be present");
    let DirectDeclarator::Array { inner, size, .. } = declarator.direct.as_ref() else {
        panic!("expected array abstract declarator");
    };
    assert_eq!(inner.as_ref(), &DirectDeclarator::Abstract);
    assert_eq!(
        size.as_ref(),
        &ArraySize::Expr(Expr::binary(Expr::int(2), BinaryOp::Add, Expr::int(1)))
    );
}

#[test]
fn parses_cast_with_function_pointer_type_name() {
    let unit = parse_source("int f(void) { return (int (*)(int))fp; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
        panic!("expected function definition");
    };

    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
        panic!("expected return statement");
    };

    let ExprKind::Cast {
        ty,
        expr: cast_expr,
    } = &expr.kind
    else {
        panic!("expected cast expression");
    };
    assert_eq!(ty.specifiers.ty, vec![TypeSpecifier::Int]);
    assert_eq!(**cast_expr, Expr::var("fp".to_string()));

    let declarator = ty
        .declarator
        .as_ref()
        .expect("function-pointer abstract declarator should be present");
    let DirectDeclarator::Function { inner, params } = declarator.direct.as_ref() else {
        panic!("expected function abstract declarator");
    };
    let DirectDeclarator::Grouped(grouped) = inner.as_ref() else {
        panic!("expected grouped pointer declarator");
    };
    assert_eq!(grouped.pointers.len(), 1);
    assert_eq!(grouped.direct.as_ref(), &DirectDeclarator::Abstract);

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
}

#[test]
fn parses_sizeof_with_function_pointer_type_name_pointer_parameter() {
    let unit = parse_source("int f(void) { return sizeof(void (*)(int *)); }");
    let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
        panic!("expected function definition");
    };

    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
        panic!("expected return statement");
    };

    let ExprKind::SizeofType(ty) = &expr.kind else {
        panic!("expected sizeof(type-name)");
    };
    assert_eq!(ty.specifiers.ty, vec![TypeSpecifier::Void]);

    let declarator = ty
        .declarator
        .as_ref()
        .expect("function-pointer abstract declarator should be present");
    let DirectDeclarator::Function { params, .. } = declarator.direct.as_ref() else {
        panic!("expected function abstract declarator");
    };

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);

    let param_decl = params[0]
        .declarator
        .as_ref()
        .expect("pointer parameter declarator should be present");
    assert_eq!(param_decl.pointers.len(), 1);
    assert_eq!(param_decl.direct.as_ref(), &DirectDeclarator::Abstract);
}

#[test]
fn parses_cast_with_function_pointer_type_name_const_char_pointer_parameter() {
    let unit = parse_source("int f(void) { return (int (*)(const char *))ptr; }");
    let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
        panic!("expected function definition");
    };

    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
        panic!("expected return statement");
    };

    let ExprKind::Cast { ty, expr } = &expr.kind else {
        panic!("expected cast expression");
    };
    assert_eq!(**expr, Expr::var("ptr".to_string()));
    assert_eq!(ty.specifiers.ty, vec![TypeSpecifier::Int]);

    let declarator = ty
        .declarator
        .as_ref()
        .expect("function-pointer abstract declarator should be present");
    let DirectDeclarator::Function { params, .. } = declarator.direct.as_ref() else {
        panic!("expected function abstract declarator");
    };
    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.qualifiers, vec![TypeQualifier::Const]);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Char]);
    let param_decl = params[0]
        .declarator
        .as_ref()
        .expect("pointer parameter declarator should be present");
    assert_eq!(param_decl.pointers.len(), 1);
    assert_eq!(param_decl.direct.as_ref(), &DirectDeclarator::Abstract);
}

#[test]
fn parses_sizeof_with_function_pointer_type_name_array_parameter() {
    let unit = parse_source("int f(void) { return sizeof(int (*)(int, char *[])); }");
    let ExternalDecl::FunctionDef(def) = &unit.items[0] else {
        panic!("expected function definition");
    };

    let BlockItem::Stmt(Stmt::Return(Some(expr))) = &def.body.items[0] else {
        panic!("expected return statement");
    };

    let ExprKind::SizeofType(ty) = &expr.kind else {
        panic!("expected sizeof(type-name)");
    };
    assert_eq!(ty.specifiers.ty, vec![TypeSpecifier::Int]);

    let declarator = ty
        .declarator
        .as_ref()
        .expect("function-pointer abstract declarator should be present");
    let DirectDeclarator::Function { params, .. } = declarator.direct.as_ref() else {
        panic!("expected function abstract declarator");
    };

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
    assert_eq!(params[1].specifiers.ty, vec![TypeSpecifier::Char]);

    let second_param_decl = params[1]
        .declarator
        .as_ref()
        .expect("second parameter declarator should be present");
    assert_eq!(second_param_decl.pointers.len(), 1);
    let DirectDeclarator::Array { inner, size, .. } = second_param_decl.direct.as_ref() else {
        panic!("expected array declarator on second parameter");
    };
    assert_eq!(inner.as_ref(), &DirectDeclarator::Abstract);
    assert_eq!(size.as_ref(), &ArraySize::Unspecified);
}

// Function-pointer parameter declarator shape tests.
#[test]
fn parses_function_pointer_parameter_declarator() {
    let unit = parse_source("int f(int (*callback)(int));");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };

    let DirectDeclarator::Function { params, .. } = decl.declarators[0].declarator.direct.as_ref()
    else {
        panic!("expected function declarator");
    };
    let FunctionParams::Prototype { params, .. } = params else {
        panic!("expected prototype parameter list");
    };
    assert_eq!(params.len(), 1);

    let param_declarator = params[0]
        .declarator
        .as_ref()
        .expect("parameter declarator expected");
    let DirectDeclarator::Function { inner, params } = param_declarator.direct.as_ref() else {
        panic!("expected function-pointer parameter declarator");
    };
    let DirectDeclarator::Grouped(grouped) = inner.as_ref() else {
        panic!("expected grouped inner declarator");
    };
    assert_eq!(grouped.pointers.len(), 1);
    assert_direct_ident(grouped.direct.as_ref(), "callback");

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
}

#[test]
fn parses_unnamed_function_pointer_parameter_declarator() {
    let unit = parse_source("int f(int (*)(int));");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };

    let DirectDeclarator::Function { params, .. } = decl.declarators[0].declarator.direct.as_ref()
    else {
        panic!("expected function declarator");
    };
    let FunctionParams::Prototype { params, .. } = params else {
        panic!("expected prototype parameter list");
    };
    assert_eq!(params.len(), 1);

    let param_declarator = params[0]
        .declarator
        .as_ref()
        .expect("parameter declarator expected");
    let DirectDeclarator::Function { inner, params } = param_declarator.direct.as_ref() else {
        panic!("expected function-pointer parameter declarator");
    };
    let DirectDeclarator::Grouped(grouped) = inner.as_ref() else {
        panic!("expected grouped inner declarator");
    };
    assert_eq!(grouped.pointers.len(), 1);
    assert_eq!(grouped.direct.as_ref(), &DirectDeclarator::Abstract);

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
    assert!(params[0].declarator.is_none());
}

#[test]
fn parses_array_of_function_pointer_parameter_declarator() {
    let unit = parse_source("int f4(int (*[])(int));");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };

    let DirectDeclarator::Function { params, .. } = decl.declarators[0].declarator.direct.as_ref()
    else {
        panic!("expected function declarator");
    };
    let FunctionParams::Prototype { params, .. } = params else {
        panic!("expected prototype parameter list");
    };
    assert_eq!(params.len(), 1);

    let param_declarator = params[0]
        .declarator
        .as_ref()
        .expect("parameter declarator expected");
    let DirectDeclarator::Function { inner, params } = param_declarator.direct.as_ref() else {
        panic!("expected function-pointer declarator");
    };
    let DirectDeclarator::Grouped(grouped) = inner.as_ref() else {
        panic!("expected grouped inner declarator");
    };
    assert_eq!(grouped.pointers.len(), 1);
    let DirectDeclarator::Array { inner, size, .. } = grouped.direct.as_ref() else {
        panic!("expected inner array declarator");
    };
    assert_eq!(inner.as_ref(), &DirectDeclarator::Abstract);
    assert_eq!(size.as_ref(), &ArraySize::Unspecified);

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(!variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
}

#[test]
fn parses_variadic_function_declaration() {
    let unit = parse_source("int printf(const char *fmt, ...);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };
    assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);

    let declarator = &decl.declarators[0].declarator;
    let DirectDeclarator::Function { inner, params } = declarator.direct.as_ref() else {
        panic!("expected function declarator");
    };
    assert_direct_ident(inner.as_ref(), "printf");

    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.qualifiers, vec![TypeQualifier::Const]);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Char]);
    let param_decl = params[0]
        .declarator
        .as_ref()
        .expect("parameter should have declarator");
    assert_eq!(param_decl.pointers.len(), 1);
    assert_direct_ident(param_decl.direct.as_ref(), "fmt");
}

#[test]
fn parses_variadic_function_with_unnamed_params() {
    let unit = parse_source("void f(int, ...);");
    let ExternalDecl::Declaration(decl) = &unit.items[0] else {
        panic!("expected declaration");
    };

    let declarator = &decl.declarators[0].declarator;
    let DirectDeclarator::Function { params, .. } = declarator.direct.as_ref() else {
        panic!("expected function declarator");
    };
    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
    assert!(params[0].declarator.is_none());
}

#[test]
fn parses_variadic_function_definition() {
    let unit = parse_source("int sum(int count, ...) { return 0; }");
    let ExternalDecl::FunctionDef(func) = &unit.items[0] else {
        panic!("expected function definition");
    };

    let DirectDeclarator::Function { params, .. } = func.declarator.direct.as_ref() else {
        panic!("expected function declarator");
    };
    let FunctionParams::Prototype { params, variadic } = params else {
        panic!("expected prototype parameter list");
    };
    assert!(variadic);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].specifiers.ty, vec![TypeSpecifier::Int]);
}
