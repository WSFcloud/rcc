use super::BackendError;
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_module::default_libcall_names;
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::path::Path;

pub(crate) fn new_object_module(isa: OwnedTargetIsa) -> Result<ObjectModule, BackendError> {
    let builder = ObjectBuilder::new(isa, "rcc", default_libcall_names())?;
    Ok(ObjectModule::new(builder))
}

pub(crate) fn emit_object_file(
    module: ObjectModule,
    output_path: &Path,
) -> Result<(), BackendError> {
    let product = module.finish();
    let bytes = product.emit()?;
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, bytes)?;
    Ok(())
}
