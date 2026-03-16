use super::*;
use crate::frontend::lexer::lexer_from_source;
use chumsky::input::{Input, Stream};

fn parse_source(src: &str) -> TranslationUnit {
    let tokens = lexer_from_source(src);
    let stream =
        Stream::from_iter(tokens).map((src.len()..src.len()).into(), |(token, span)| (token, span));

    parse(stream).expect("source should parse")
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

fn assert_direct_ident(direct: &DirectDeclarator, expected: &str) {
    match &direct.kind {
        DirectDeclaratorKind::Ident(name) => assert_eq!(name, expected),
        other => panic!("expected identifier declarator, got {other:?}"),
    }
}

fn assert_ident_declarator(init_declarator: &InitDeclarator, expected: &str) {
    assert_direct_ident(init_declarator.declarator.direct.as_ref(), expected);
}

fn expect_stmt(item: &BlockItem) -> &Stmt {
    let BlockItem::Stmt(stmt) = item else {
        panic!("expected statement");
    };
    stmt
}

fn expect_return_expr(item: &BlockItem) -> &Expr {
    let stmt = expect_stmt(item);
    let StmtKind::Return(Some(expr)) = &stmt.kind else {
        panic!("expected return expression");
    };
    expr
}

impl TypeSpecifier {
    pub fn test_new(kind: TypeSpecifierKind) -> Self {
        Self::new(kind, SourceSpan::dummy())
    }

    #[allow(non_upper_case_globals)]
    pub const Void: Self = Self {
        kind: TypeSpecifierKind::Void,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_upper_case_globals)]
    pub const Char: Self = Self {
        kind: TypeSpecifierKind::Char,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_upper_case_globals)]
    pub const Short: Self = Self {
        kind: TypeSpecifierKind::Short,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_upper_case_globals)]
    pub const Int: Self = Self {
        kind: TypeSpecifierKind::Int,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_upper_case_globals)]
    pub const Long: Self = Self {
        kind: TypeSpecifierKind::Long,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_upper_case_globals)]
    pub const Float: Self = Self {
        kind: TypeSpecifierKind::Float,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_upper_case_globals)]
    pub const Double: Self = Self {
        kind: TypeSpecifierKind::Double,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_upper_case_globals)]
    pub const Signed: Self = Self {
        kind: TypeSpecifierKind::Signed,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_upper_case_globals)]
    pub const Unsigned: Self = Self {
        kind: TypeSpecifierKind::Unsigned,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_upper_case_globals)]
    pub const Bool: Self = Self {
        kind: TypeSpecifierKind::Bool,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_snake_case)]
    pub fn TypedefName(name: String) -> Self {
        Self::test_new(TypeSpecifierKind::TypedefName(name))
    }

    #[allow(non_snake_case)]
    pub fn StructOrUnion(record: RecordSpecifier) -> Self {
        Self::test_new(TypeSpecifierKind::StructOrUnion(record))
    }

    #[allow(non_snake_case)]
    pub fn Enum(enum_spec: EnumSpecifier) -> Self {
        Self::test_new(TypeSpecifierKind::Enum(enum_spec))
    }
}

impl DirectDeclarator {
    pub fn test_new(kind: DirectDeclaratorKind) -> Self {
        Self::new(kind, SourceSpan::dummy())
    }

    #[allow(non_upper_case_globals)]
    pub const Abstract: Self = Self {
        kind: DirectDeclaratorKind::Abstract,
        span: SourceSpan { start: 0, end: 0 },
    };

    #[allow(non_snake_case)]
    pub fn Ident(name: String) -> Self {
        Self::test_new(DirectDeclaratorKind::Ident(name))
    }

    #[allow(non_snake_case)]
    pub fn Grouped(declarator: Box<Declarator>) -> Self {
        Self::test_new(DirectDeclaratorKind::Grouped(declarator))
    }

    #[allow(non_snake_case)]
    pub fn Array(
        inner: Box<DirectDeclarator>,
        qualifiers: Vec<TypeQualifier>,
        is_static: bool,
        size: Box<ArraySize>,
    ) -> Self {
        Self::test_new(DirectDeclaratorKind::Array {
            inner,
            qualifiers,
            is_static,
            size,
        })
    }

    #[allow(non_snake_case)]
    pub fn Function(inner: Box<DirectDeclarator>, params: FunctionParams) -> Self {
        Self::test_new(DirectDeclaratorKind::Function { inner, params })
    }
}

impl Designator {
    pub fn test_new(kind: DesignatorKind) -> Self {
        Self::new(kind, SourceSpan::dummy())
    }

    #[allow(non_snake_case)]
    pub fn Index(expr: Expr) -> Self {
        Self::test_new(DesignatorKind::Index(expr))
    }

    #[allow(non_snake_case)]
    pub fn Field(field: String) -> Self {
        Self::test_new(DesignatorKind::Field(field))
    }
}

impl Stmt {
    pub fn test_new(kind: StmtKind) -> Self {
        Self::new(kind, SourceSpan::dummy())
    }
}

impl Expr {
    pub fn test_new(kind: ExprKind) -> Self {
        Self::new(kind, SourceSpan::dummy())
    }

    pub fn int(value: u64) -> Self {
        Self::int_with_span(value, SourceSpan::dummy())
    }

    pub fn int_with_base(value: u64, base: IntLiteralSuffix) -> Self {
        Self::int_with_base_and_span(value, base, SourceSpan::dummy())
    }

    pub fn float(value: f64) -> Self {
        Self::float_with_span(value, SourceSpan::dummy())
    }

    pub fn char(value: char) -> Self {
        Self::char_with_span(value, SourceSpan::dummy())
    }

    pub fn string(value: String) -> Self {
        Self::string_with_span(value, SourceSpan::dummy())
    }

    pub fn var(name: String) -> Self {
        Self::var_with_span(name, SourceSpan::dummy())
    }
}

mod control_flow;
mod declarations;
mod functions;
mod type_name;
mod typedef_scope;
