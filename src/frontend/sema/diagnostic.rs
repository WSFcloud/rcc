use crate::common::span::SourceSpan;

/// Stable semantic diagnostic codes.
///
/// Each code represents a specific category of semantic error or warning.
/// These codes are stable across versions to support tooling integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemaDiagnosticCode {
    /// Reference to an undeclared identifier.
    UndefinedSymbol,
    /// Reference to an undeclared label.
    UndefinedLabel,
    /// Multiple labels with the same name in a function.
    DuplicateLabel,

    /// Type mismatch in an operation or assignment.
    TypeMismatch,
    /// Incompatible types in declaration merging or comparison.
    IncompatibleTypes,
    /// Invalid type cast.
    InvalidCast,
    /// Use of an incomplete type where a complete type is required.
    IncompleteType,

    /// Conflicting redeclaration of a symbol.
    RedeclarationConflict,
    /// Invalid linkage merge between declarations.
    InvalidLinkageMerge,

    /// Invalid initializer for a variable or aggregate.
    InvalidInitializer,
    /// Jump (goto) bypasses variable initialization.
    JumpOverInitializer,
    /// Invalid placement of control-flow statements/labels (break/continue/case/default).
    InvalidControlFlow,

    /// Variable-modified types are not supported.
    UnsupportedVmType,
    /// K&R-style function definitions are not supported.
    UnsupportedKnrDefinition,
    /// Non-constant expression in a context requiring a constant.
    NonConstantInRequiredContext,
    /// Division or modulo by zero in a constant expression.
    ConstantDivisionByZero,
    /// Signed integer overflow while evaluating a constant expression.
    ConstantSignedOverflow,
}

/// One semantic diagnostic with optional secondary labels and notes.
///
/// This structure represents a single error or warning message with:
/// - A stable diagnostic code
/// - A primary message and source location
/// - Optional secondary labels pointing to related code
/// - Optional notes with additional context
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemaDiagnostic {
    pub code: SemaDiagnosticCode,
    pub message: String,
    pub primary: SourceSpan,
    pub secondary: Vec<(SourceSpan, String)>,
    pub notes: Vec<String>,
}

impl SemaDiagnostic {
    /// Creates a new diagnostic with a primary message and location.
    pub fn new(code: SemaDiagnosticCode, message: impl Into<String>, primary: SourceSpan) -> Self {
        Self {
            code,
            message: message.into(),
            primary,
            secondary: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Adds a secondary label pointing to related code.
    ///
    /// Secondary labels are used to show context, such as:
    /// - "previous declaration is here"
    /// - "conflicting type defined here"
    #[must_use]
    pub fn with_secondary(mut self, span: SourceSpan, message: impl Into<String>) -> Self {
        self.secondary.push((span, message.into()));
        self
    }

    /// Adds a note with additional context or suggestions.
    ///
    /// Notes are displayed after the main diagnostic message.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}
