use crate::common::config::CompilerConfig;
use crate::common::token::TokenKind;
use crate::frontend::lexer::lexer_from_source;
use crate::frontend::parser::diagnostic::emit_parse_diagnostics;
use crate::frontend::parser::parse;
use crate::frontend::sema;
use crate::frontend::sema::diagnostic::SemaDiagnostic;
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
            emit_sema_diagnostics(filename, &source, &diagnostics);
            return Err(PipelineError::SemaDiagnostic(format!(
                "failed semantic analysis for '{}'",
                input_path.display()
            )));
        }
    };
    println!("Typed AST: {:#?}", sema_result.typed_tu);

    Ok(())
}

fn emit_sema_diagnostics(filename: &str, source: &str, diagnostics: &[SemaDiagnostic]) {
    for diag in diagnostics {
        eprintln!(
            "{filename}:{}..{}: error[{:#?}]: {}",
            diag.primary.start, diag.primary.end, diag.code, diag.message
        );
        if let Some(snippet) = diagnostic_snippet(source, diag.primary.start, diag.primary.end) {
            eprintln!("  primary: {snippet}");
        }
        for (span, message) in &diag.secondary {
            eprintln!("  secondary {}..{}: {}", span.start, span.end, message);
            if let Some(snippet) = diagnostic_snippet(source, span.start, span.end) {
                eprintln!("    {snippet}");
            }
        }
        for note in &diag.notes {
            eprintln!("  note: {note}");
        }
    }
}

fn diagnostic_snippet(source: &str, start: usize, end: usize) -> Option<String> {
    let raw = source.get(start..end)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let one_line = trimmed.replace('\n', "\\n");
    const MAX_LEN: usize = 120;
    if one_line.chars().count() <= MAX_LEN {
        Some(one_line)
    } else {
        Some(format!("{}...", one_line.chars().take(MAX_LEN).collect::<String>()))
    }
}
