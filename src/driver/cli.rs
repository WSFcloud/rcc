use crate::common::config;
use clap::Parser;
use std::path::PathBuf;

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliEmitKind {
    Obj,
}

/// CLI arguments parsed from command line.
#[derive(Parser, Debug)]
#[command(name = "rcc", version = "0.1.0", disable_help_flag = true)]
pub struct Cli {
    /// Display available options.
    #[arg(long, short='h', action = clap::ArgAction::Help)]
    pub help: Option<bool>,

    /// Input file path.
    #[arg(required = true)]
    pub files: Vec<PathBuf>,

    /// Write output to <FILE>.
    #[arg(short = 'o', value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Add directory to the end of the list of include search paths.
    #[arg(short = 'I', value_name = "DIR", number_of_values = 1)]
    pub include_dirs: Vec<PathBuf>,

    /// Define <macro> to <value> (or 1 if <value> omitted).
    #[arg(short = 'D', value_name = "MACRO", value_parser = Cli::parse_macros_define)]
    pub macros_define: Vec<(String, String)>,

    /// Undefine macro <macro>.
    #[arg(short = 'U', value_name = "MACRO", number_of_values = 1)]
    pub macros_undefine: Vec<String>,

    /// Optimization level.
    #[arg(
        short = 'O',
        value_name = "LEVEL",
        value_parser = clap::value_parser!(u32)
    )]
    pub optimization: Option<u32>,

    /// Enable the specified warning.
    #[arg(short = 'W', value_name = "WARNING")]
    pub warnings: Vec<String>,

    /// Compile and assemble, but do not link.
    #[arg(short = 'c')]
    pub compile_only: bool,

    /// Preprocess only; do not compile, assemble or link.
    #[arg(short = 'E')]
    pub preprocess_only: bool,

    /// Compile only; do not assemble or link.
    #[arg(short = 'S')]
    pub assemble_only: bool,

    /// Emit artifact kind (currently only object file).
    #[arg(long = "emit", value_enum, default_value_t = CliEmitKind::Obj)]
    pub emit: CliEmitKind,

    /// Print MIR after lowering.
    #[arg(long = "emit-mir")]
    pub emit_mir: bool,

    /// Print backend debugging information.
    #[arg(long = "debug-backend")]
    pub debug_backend: bool,
}

impl Cli {
    fn parse_macros_define(s: &str) -> Result<(String, String), String> {
        match s.split_once('=') {
            Some((key, value)) => Ok((key.to_string(), value.to_string())),
            None => Ok((s.to_string(), "1".to_string())),
        }
    }

    /// Convert CLI arguments to compiler configuration.
    pub fn to_config(&self) -> config::CompilerConfig {
        config::CompilerConfig {
            files: self.files.clone(),
            output: self.output.clone(),
            include_dirs: self.include_dirs.clone(),
            macros_define: self.macros_define.clone(),
            macros_undefine: self.macros_undefine.clone(),
            optimization: self.optimization.unwrap_or(0),
            warnings: self.warnings.clone(),
            compile_only: self.compile_only,
            preprocess_only: self.preprocess_only,
            assemble_only: self.assemble_only,
            emit_kind: match self.emit {
                CliEmitKind::Obj => config::EmitKind::Obj,
            },
            emit_mir: self.emit_mir,
            debug_backend: self.debug_backend,
        }
    }
}

pub fn parse() -> Cli {
    Cli::parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_macros_define_with_value() {
        let result = Cli::parse_macros_define("DEBUG=1").unwrap();
        assert_eq!(result, ("DEBUG".to_string(), "1".to_string()));
    }

    #[test]
    fn test_parse_macros_define_without_value() {
        let result = Cli::parse_macros_define("NDEBUG").unwrap();
        assert_eq!(result, ("NDEBUG".to_string(), "1".to_string()));
    }

    #[test]
    fn test_parse_macros_define_complex_value() {
        let result = Cli::parse_macros_define("VERSION=2.0.1").unwrap();
        assert_eq!(result, ("VERSION".to_string(), "2.0.1".to_string()));
    }

    #[test]
    fn test_cli_to_config() {
        let config = Cli::try_parse_from([
            "rcc",
            "test.c",
            "-o",
            "test.o",
            "-I/usr/include",
            "-DDEBUG=1",
            "-O2",
            "-Wall",
        ])
        .unwrap()
        .to_config();

        assert_eq!(config.files, vec![PathBuf::from("test.c")]);
        assert_eq!(config.output, Some(PathBuf::from("test.o")));
        assert_eq!(config.include_dirs, vec![PathBuf::from("/usr/include")]);
        assert_eq!(
            config.macros_define,
            vec![("DEBUG".to_string(), "1".to_string())]
        );
        assert_eq!(config.optimization, 2);
        assert_eq!(config.warnings, vec!["all"]);
        assert!(!config.compile_only);
        assert!(!config.preprocess_only);
        assert_eq!(config.emit_kind, config::EmitKind::Obj);
        assert!(!config.emit_mir);
        assert!(!config.debug_backend);
    }

    #[test]
    fn test_cli_parse_optimization_requires_value() {
        let result = Cli::try_parse_from(["rcc", "-O", "-c", "test.c"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_to_config_default_optimization() {
        let cli = Cli::try_parse_from(["rcc", "test.c"]).expect("cli parse should succeed");
        let config = cli.to_config();
        assert_eq!(config.optimization, 0);
        assert_eq!(config.emit_kind, config::EmitKind::Obj);
        assert!(!config.emit_mir);
        assert!(!config.debug_backend);
    }

    #[test]
    fn test_cli_parse_emit_mir_and_debug_backend_flags() {
        let cli = Cli::try_parse_from(["rcc", "--emit-mir", "--debug-backend", "test.c"])
            .expect("cli parse should succeed");
        let config = cli.to_config();
        assert!(config.emit_mir);
        assert!(config.debug_backend);
    }

    #[test]
    fn test_cli_parse_emit_obj() {
        let cli =
            Cli::try_parse_from(["rcc", "--emit=obj", "test.c"]).expect("cli parse should succeed");
        let config = cli.to_config();
        assert_eq!(config.emit_kind, config::EmitKind::Obj);
    }
}
