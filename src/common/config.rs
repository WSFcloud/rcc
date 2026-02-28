use std::fs::File;
use std::io::ErrorKind;
use std::path::PathBuf;

/// Compiler configuration derived from CLI arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct CompilerConfig {
    /// Input file paths
    pub files: Vec<PathBuf>,

    /// Output file path
    pub output: Option<PathBuf>,

    /// Include search paths
    pub include_dirs: Vec<PathBuf>,

    /// Macro definitions (macro name, value)
    pub macros_define: Vec<(String, String)>,

    /// Macros to undefine
    pub macros_undefine: Vec<String>,

    /// Optimization level (0, 1, 2, 3)
    pub optimization: u32,

    /// Enabled warnings
    pub warnings: Vec<String>,

    /// Compile only (no link)
    pub compile_only: bool,

    /// Preprocess only (no compile, assemble, link)
    pub preprocess_only: bool,

    /// Compile only (no assemble, link)
    pub assemble_only: bool,
}

impl CompilerConfig {
    /// Check if preprocessing is enabled
    pub fn is_preprocess_only(&self) -> bool {
        self.preprocess_only
    }

    /// Check if compilation is enabled
    pub fn is_compile_only(&self) -> bool {
        self.compile_only
    }

    /// Check if assembly only is enabled
    pub fn is_assemble_only(&self) -> bool {
        self.assemble_only
    }

    /// Process and print enabled warnings
    pub fn display_warnings(&self) {
        for w in &self.warnings {
            match w.as_str() {
                "all" => println!("Enabled: All Warnings (-Wall)"),
                "error" => println!("Enabled: Treat Warnings as Errors (-Werror)"),
                other => println!("Enabled: Specific Warning '{}'", other),
            }
        }
    }

    /// Validate compiler configuration and return a readable error message when invalid
    pub fn check_config(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.files.is_empty() {
            errors.push("no input files provided".to_string());
        }

        for file in &self.files {
            match std::fs::metadata(file) {
                Ok(metadata) => {
                    if !metadata.is_file() {
                        errors.push(format!(
                            "input path '{}' is not a regular file",
                            file.display()
                        ));
                        continue;
                    }
                    if let Err(err) = File::open(file) {
                        errors.push(format!(
                            "cannot open input file '{}': {}",
                            file.display(),
                            err
                        ));
                    }
                }
                Err(err) if err.kind() == ErrorKind::NotFound => {
                    errors.push(format!("input file '{}' does not exist", file.display()));
                }
                Err(err) => {
                    errors.push(format!(
                        "cannot access input file '{}': {}",
                        file.display(),
                        err
                    ));
                }
            }
        }

        let stage_count = [self.compile_only, self.preprocess_only, self.assemble_only]
            .into_iter()
            .filter(|enabled| *enabled)
            .count();
        if stage_count > 1 {
            errors.push("options -c, -E, and -S are mutually exclusive".to_string());
        }

        if self.optimization > 3 {
            errors.push(format!(
                "invalid optimization level '{}', expected 0..=3",
                self.optimization
            ));
        }

        if self
            .macros_define
            .iter()
            .any(|(name, _)| name.trim().is_empty())
        {
            errors.push("macro definition contains empty macro name".to_string());
        }

        if self
            .macros_undefine
            .iter()
            .any(|name| name.trim().is_empty())
        {
            errors.push("macro undefinition contains empty macro name".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn create_temp_c_file(case: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        path.push(format!("rcc_{case}_{}_{}.c", std::process::id(), nanos));
        std::fs::write(&path, "int main(void) { return 0; }\n")
            .expect("failed to create temporary test file");
        path
    }

    fn cleanup_path(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(path);
    }

    #[test]
    fn test_check_config_ok() {
        let input = create_temp_c_file("ok");
        let config = CompilerConfig {
            files: vec![input.clone()],
            output: None,
            include_dirs: vec![],
            macros_define: vec![("DEBUG".to_string(), "1".to_string())],
            macros_undefine: vec!["NDEBUG".to_string()],
            optimization: 2,
            warnings: vec![],
            compile_only: true,
            preprocess_only: false,
            assemble_only: false,
        };

        assert!(config.check_config().is_ok());
        cleanup_path(&input);
    }

    #[test]
    fn test_check_config_rejects_conflicting_stages() {
        let input = create_temp_c_file("conflicting_stages");
        let config = CompilerConfig {
            files: vec![input.clone()],
            output: None,
            include_dirs: vec![],
            macros_define: vec![],
            macros_undefine: vec![],
            optimization: 0,
            warnings: vec![],
            compile_only: true,
            preprocess_only: true,
            assemble_only: false,
        };

        let err = config.check_config().unwrap_err();
        assert!(err.iter().any(|e| e.contains("mutually exclusive")));
        cleanup_path(&input);
    }

    #[test]
    fn test_check_config_rejects_invalid_optimization() {
        let input = create_temp_c_file("invalid_opt");
        let config = CompilerConfig {
            files: vec![input.clone()],
            output: None,
            include_dirs: vec![],
            macros_define: vec![],
            macros_undefine: vec![],
            optimization: 4,
            warnings: vec![],
            compile_only: false,
            preprocess_only: false,
            assemble_only: false,
        };

        let err = config.check_config().unwrap_err();
        assert!(err.iter().any(|e| e.contains("invalid optimization level")));
        cleanup_path(&input);
    }

    #[test]
    fn test_check_config_rejects_empty_macro_name() {
        let input = create_temp_c_file("empty_macro");
        let config = CompilerConfig {
            files: vec![input.clone()],
            output: None,
            include_dirs: vec![],
            macros_define: vec![("".to_string(), "1".to_string())],
            macros_undefine: vec![],
            optimization: 0,
            warnings: vec![],
            compile_only: false,
            preprocess_only: false,
            assemble_only: false,
        };

        let err = config.check_config().unwrap_err();
        assert!(err.iter().any(|e| e.contains("empty macro name")));
        cleanup_path(&input);
    }

    #[test]
    fn test_check_config_rejects_missing_input_file() {
        let mut missing = std::env::temp_dir();
        missing.push(format!(
            "rcc_missing_{}_{}.c",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));

        let config = CompilerConfig {
            files: vec![missing.clone()],
            output: None,
            include_dirs: vec![],
            macros_define: vec![],
            macros_undefine: vec![],
            optimization: 0,
            warnings: vec![],
            compile_only: false,
            preprocess_only: false,
            assemble_only: false,
        };

        let err = config.check_config().unwrap_err();
        assert!(err.iter().any(|e| e.contains("does not exist")));
        cleanup_path(&missing);
    }

    #[test]
    fn test_check_config_rejects_directory_input() {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "rcc_dir_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir(&dir).expect("failed to create temporary test directory");

        let config = CompilerConfig {
            files: vec![dir.clone()],
            output: None,
            include_dirs: vec![],
            macros_define: vec![],
            macros_undefine: vec![],
            optimization: 0,
            warnings: vec![],
            compile_only: false,
            preprocess_only: false,
            assemble_only: false,
        };

        let err = config.check_config().unwrap_err();
        assert!(err.iter().any(|e| e.contains("not a regular file")));
        cleanup_path(&dir);
    }
}
