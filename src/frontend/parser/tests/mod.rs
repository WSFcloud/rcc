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
    match direct {
        DirectDeclarator::Ident(name) => assert_eq!(name, expected),
        other => panic!("expected identifier declarator, got {other:?}"),
    }
}

fn assert_ident_declarator(init_declarator: &InitDeclarator, expected: &str) {
    assert_direct_ident(init_declarator.declarator.direct.as_ref(), expected);
}

mod control_flow;
mod declarations;
mod functions;
mod type_name;
mod typedef_scope;
