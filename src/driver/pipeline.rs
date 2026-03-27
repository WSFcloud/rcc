use crate::common::config::CompilerConfig;
use crate::common::token::TokenKind;
use crate::frontend::lexer::lexer_from_source;
use crate::frontend::parser::diagnostic::emit_parse_diagnostics;
use crate::frontend::parser::parse;
use crate::frontend::sema;
use crate::frontend::sema::diagnostic::emit_sema_diagnostics;
use crate::mir::builder::lower_to_mir_with_optimization;
use crate::mir::display::dump as dump_mir;
use chumsky::input::{Input, Stream};
use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum PipelineError {
    MissingInput,
    UnsupportedArgument(&'static str),
    Io(std::io::Error),
    ParseDiagnostic(String),
    SemaDiagnostic(String),
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineError::MissingInput => write!(f, "no input file provided"),
            PipelineError::UnsupportedArgument(flag) => {
                write!(f, "argument '{}' is not supported", flag)
            }
            PipelineError::Io(err) => write!(f, "{err}"),
            PipelineError::ParseDiagnostic(msg) => write!(f, "{msg}"),
            PipelineError::SemaDiagnostic(msg) => write!(f, "{msg}"),
        }
    }
}

impl Error for PipelineError {}

impl From<std::io::Error> for PipelineError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn run(config: CompilerConfig) -> Result<(), PipelineError> {
    if config.files.is_empty() {
        return Err(PipelineError::MissingInput);
    }

    let input_path = &config.files[0];
    let source = std::fs::read_to_string(input_path)?;

    // Tokenize the source
    let tokens = lexer_from_source(&source);
    let token_stream = Stream::from_iter(tokens).map(
        (source.len()..source.len()).into(),
        |(token, span): (TokenKind, parse::Span)| (token, span),
    );

    // Parse tokens into AST
    let filename = input_path.to_str().unwrap_or("input");
    let ast = match parse::parse(token_stream) {
        Ok(ast) => ast,
        Err(errors) => {
            emit_parse_diagnostics(filename, &source, errors)?;
            return Err(PipelineError::ParseDiagnostic(format!(
                "failed to parse '{}'",
                input_path.display()
            )));
        }
    };

    println!("Parsed AST: {:#?}", ast);
    let sema_result = match sema::analyze(filename, &source, &ast) {
        Ok(result) => result,
        Err(diagnostics) => {
            emit_sema_diagnostics(filename, &source, &diagnostics)?;
            return Err(PipelineError::SemaDiagnostic(format!(
                "failed semantic analysis for '{}'",
                input_path.display()
            )));
        }
    };
    println!("Typed AST: {:#?}", sema_result.typed_tu);

    let mir_program = lower_to_mir_with_optimization(&sema_result, config.optimization);
    println!("MIR:\n{}", dump_mir(&mir_program));

    Ok(())
}
