use super::BackendError;
use cranelift_codegen::isa::{self, OwnedTargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use std::str::FromStr;
use target_lexicon::Triple;

const DEFAULT_TARGET_TRIPLE: &str = "x86_64-unknown-linux-gnu";

pub(crate) fn build_default_isa() -> Result<OwnedTargetIsa, BackendError> {
    let triple = Triple::from_str(DEFAULT_TARGET_TRIPLE).map_err(|err| {
        BackendError::InvalidTargetTriple {
            triple: DEFAULT_TARGET_TRIPLE.to_string(),
            message: err.to_string(),
        }
    })?;
    let isa_builder = isa::lookup(triple)?;
    let mut shared_builder = settings::builder();
    shared_builder.enable("is_pic")?;
    let shared_flags = settings::Flags::new(shared_builder);
    isa_builder
        .finish(shared_flags)
        .map_err(|err| BackendError::IsaBuild(err.to_string()))
}
