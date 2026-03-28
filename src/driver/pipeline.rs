use crate::backend::BackendError;
use crate::backend::compile_mir_to_object;
use crate::common::config::CompilerConfig;
use crate::common::config::EmitKind;
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
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub enum PipelineError {
    MissingInput,
    UnsupportedArgument(&'static str),
    Io(std::io::Error),
    ParseDiagnostic(String),
    SemaDiagnostic(String),
    Backend(BackendError),
    Linker(String),
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
            PipelineError::Backend(err) => write!(f, "{err}"),
            PipelineError::Linker(msg) => write!(f, "{msg}"),
        }
    }
}

impl Error for PipelineError {}

impl From<std::io::Error> for PipelineError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<BackendError> for PipelineError {
    fn from(value: BackendError) -> Self {
        Self::Backend(value)
    }
}

pub fn run(config: CompilerConfig) -> Result<(), PipelineError> {
    if config.files.is_empty() {
        return Err(PipelineError::MissingInput);
    }
    if config.preprocess_only {
        return Err(PipelineError::UnsupportedArgument("-E"));
    }
    if config.assemble_only {
        return Err(PipelineError::UnsupportedArgument("-S"));
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

    if config.debug_backend {
        println!("Parsed AST: {:#?}", ast);
    }
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
    if config.debug_backend {
        println!("Typed AST: {:#?}", sema_result.typed_tu);
    }

    let mir_program = lower_to_mir_with_optimization(&sema_result, 0);
    if config.emit_mir || config.debug_backend {
        println!("MIR:\n{}", dump_mir(&mir_program));
    }

    if config.compile_only {
        let output_path = resolve_output_path(&config, input_path);
        match config.emit_kind {
            EmitKind::Obj => compile_mir_to_object(&mir_program, &output_path)?,
        }
        return Ok(());
    }

    let output_path = resolve_output_path(&config, input_path);
    let temp_object_path = make_temp_object_path();
    match config.emit_kind {
        EmitKind::Obj => compile_mir_to_object(&mir_program, &temp_object_path)?,
    }
    if let Err(err) = link_object_to_executable(&temp_object_path, &output_path) {
        let _ = std::fs::remove_file(&temp_object_path);
        return Err(err);
    }
    let _ = std::fs::remove_file(temp_object_path);

    Ok(())
}

fn resolve_output_path(config: &CompilerConfig, input_path: &Path) -> PathBuf {
    if let Some(output) = &config.output {
        return output.clone();
    }
    if config.compile_only {
        let mut output = input_path.to_path_buf();
        output.set_extension("o");
        output
    } else {
        PathBuf::from("a.out")
    }
}

fn make_temp_object_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    path.push(format!("rcc_link_{}_{}.o", std::process::id(), nanos));
    path
}

fn link_object_to_executable(object_path: &Path, output_path: &Path) -> Result<(), PipelineError> {
    let status = Command::new("cc")
        .arg(object_path)
        .arg("-o")
        .arg(output_path)
        .status()
        .map_err(|err| {
            PipelineError::Linker(format!("failed to invoke system linker 'cc': {err}"))
        })?;
    if !status.success() {
        return Err(PipelineError::Linker(format!(
            "system linker 'cc' failed with status {status}"
        )));
    }
    Ok(())
}
