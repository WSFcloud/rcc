mod error;
mod isa;
mod lowering;
mod object;
mod symbols;

pub use error::BackendError;

use crate::mir::ir::{MirProgram, MirType};
use cranelift_module::{Linkage, Module};
use std::path::Path;

const SYNTHETIC_EMPTY_FUNCTION: &str = "__rcc_empty_object";

/// Compile MIR program into an object file.
pub fn compile_mir_to_object(program: &MirProgram, output_path: &Path) -> Result<(), BackendError> {
    let isa = isa::build_default_isa()?;
    let mut module = object::new_object_module(isa)?;
    let mut lowering_context = lowering::FunctionLoweringContext::new(&module);
    let symbols =
        symbols::declare_module_symbols(&mut module, program, lowering_context.type_lowering())?;
    symbols::define_global_objects(&mut module, &symbols, program)?;

    if program.functions.is_empty() {
        let signature = lowering_context.type_lowering().lower_signature(
            module.isa().default_call_conv(),
            &[],
            MirType::Void,
            false,
            SYNTHETIC_EMPTY_FUNCTION,
        )?;
        let func_id =
            module.declare_function(SYNTHETIC_EMPTY_FUNCTION, Linkage::Local, &signature)?;
        lowering_context.define_synthetic_empty_function(
            &mut module,
            func_id,
            SYNTHETIC_EMPTY_FUNCTION,
        )?;
    } else {
        for function in &program.functions {
            let func_id = symbols
                .function_id(&function.name)
                .ok_or_else(|| BackendError::MissingFunctionSymbol(function.name.clone()))?;
            lowering_context.define_function(&mut module, &symbols, func_id, function)?;
        }
        for function in &program.functions {
            let Some(wrapper_id) = symbols.wrapper_function_id(&function.name) else {
                continue;
            };
            lowering_context.define_function_wrapper(
                &mut module,
                &symbols,
                wrapper_id,
                function,
            )?;
        }
        for ext in &program.extern_functions {
            if !ext.boundary_sig.requires_wrapper() {
                continue;
            }
            let wrapper_id = symbols
                .function_id(&ext.name)
                .ok_or_else(|| BackendError::MissingFunctionSymbol(ext.name.clone()))?;
            let import_id = symbols
                .import_function_id(&ext.name)
                .ok_or_else(|| BackendError::MissingFunctionSymbol(ext.name.clone()))?;
            lowering_context.define_import_wrapper(&mut module, wrapper_id, import_id, ext)?;
        }
    }

    object::emit_object_file(module, output_path)
}
