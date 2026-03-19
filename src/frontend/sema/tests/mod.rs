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

mod control_flow;
mod declarations;
mod functions;
mod initializers;
