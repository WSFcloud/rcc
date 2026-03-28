use crate::mir::ir::MirType;
use cranelift_codegen::isa::LookupError;
use cranelift_codegen::settings::SetError;
use cranelift_module::ModuleError;
use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum BackendError {
    InvalidTargetTriple {
        triple: String,
        message: String,
    },
    IsaLookup(LookupError),
    IsaFlag(SetError),
    IsaBuild(String),
    Module(ModuleError),
    ObjectEmit(cranelift_object::object::write::Error),
    Io(std::io::Error),
    UnsupportedMirType {
        ty: MirType,
        context: &'static str,
    },
    UnsupportedFunctionLowering {
        function: String,
        message: String,
    },
    InvalidStackSlot {
        function: String,
        slot: u32,
        message: String,
    },
    MissingFunctionSymbol(String),
    MissingGlobalSymbol(String),
    InvalidGlobalDeclaration(String),
    InvalidGlobalInitializer {
        global: String,
        message: String,
    },
    InvalidRelocation {
        global: String,
        message: String,
    },
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTargetTriple { triple, message } => {
                write!(f, "invalid target triple '{triple}': {message}")
            }
            Self::IsaLookup(err) => write!(f, "failed to lookup target ISA: {err}"),
            Self::IsaFlag(err) => write!(f, "invalid ISA flag: {err}"),
            Self::IsaBuild(message) => write!(f, "failed to build target ISA: {message}"),
            Self::Module(err) => write!(f, "module error: {err}"),
            Self::ObjectEmit(err) => write!(f, "failed to emit object file bytes: {err}"),
            Self::Io(err) => write!(f, "i/o error: {err}"),
            Self::UnsupportedMirType { ty, context } => {
                write!(f, "unsupported MIR type '{ty}' in {context}")
            }
            Self::UnsupportedFunctionLowering { function, message } => {
                write!(
                    f,
                    "unsupported MIR function lowering for '{function}': {message}"
                )
            }
            Self::InvalidStackSlot {
                function,
                slot,
                message,
            } => {
                write!(
                    f,
                    "invalid stack slot {} in function '{function}': {message}",
                    slot
                )
            }
            Self::MissingFunctionSymbol(name) => {
                write!(
                    f,
                    "function symbol was not declared before lowering: {name}"
                )
            }
            Self::MissingGlobalSymbol(name) => {
                write!(f, "global symbol was not declared before lowering: {name}")
            }
            Self::InvalidGlobalDeclaration(name) => {
                write!(f, "invalid global declaration state for symbol: {name}")
            }
            Self::InvalidGlobalInitializer { global, message } => {
                write!(f, "invalid initializer for global '{global}': {message}")
            }
            Self::InvalidRelocation { global, message } => {
                write!(f, "invalid relocation in global '{global}': {message}")
            }
        }
    }
}

impl Error for BackendError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidTargetTriple { .. }
            | Self::IsaBuild(_)
            | Self::UnsupportedMirType { .. }
            | Self::UnsupportedFunctionLowering { .. }
            | Self::InvalidStackSlot { .. }
            | Self::MissingFunctionSymbol(_)
            | Self::MissingGlobalSymbol(_)
            | Self::InvalidGlobalDeclaration(_)
            | Self::InvalidGlobalInitializer { .. }
            | Self::InvalidRelocation { .. } => None,
            Self::IsaLookup(err) => Some(err),
            Self::IsaFlag(err) => Some(err),
            Self::Module(err) => Some(err),
            Self::ObjectEmit(err) => Some(err),
            Self::Io(err) => Some(err),
        }
    }
}

impl From<LookupError> for BackendError {
    fn from(value: LookupError) -> Self {
        Self::IsaLookup(value)
    }
}

impl From<SetError> for BackendError {
    fn from(value: SetError) -> Self {
        Self::IsaFlag(value)
    }
}

impl From<ModuleError> for BackendError {
    fn from(value: ModuleError) -> Self {
        Self::Module(value)
    }
}

impl From<cranelift_object::object::write::Error> for BackendError {
    fn from(value: cranelift_object::object::write::Error) -> Self {
        Self::ObjectEmit(value)
    }
}

impl From<std::io::Error> for BackendError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
