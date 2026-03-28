use super::BackendError;
use super::lowering::MirTypeLowering;
use crate::mir::ir::{
    MirExternFunction, MirGlobal, MirGlobalInit, MirLinkage, MirProgram, MirRelocationTarget,
};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module, ModuleRelocTarget};
use cranelift_object::ObjectModule;
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct ModuleSymbols {
    function_ids: HashMap<String, FuncId>,
    addressable_function_ids: HashMap<String, FuncId>,
    import_function_ids: HashMap<String, FuncId>,
    wrapper_function_ids: HashMap<String, FuncId>,
    wrapped_imports: HashMap<String, MirExternFunction>,
    global_ids: HashMap<String, DataId>,
}

impl ModuleSymbols {
    pub(crate) fn function_id(&self, name: &str) -> Option<FuncId> {
        self.function_ids.get(name).copied()
    }

    pub(crate) fn addressable_function_id(&self, name: &str) -> Option<FuncId> {
        self.addressable_function_ids.get(name).copied()
    }

    pub(crate) fn import_function_id(&self, name: &str) -> Option<FuncId> {
        self.import_function_ids.get(name).copied()
    }

    pub(crate) fn wrapper_function_id(&self, name: &str) -> Option<FuncId> {
        self.wrapper_function_ids.get(name).copied()
    }

    pub(crate) fn wrapped_import(&self, name: &str) -> Option<&MirExternFunction> {
        self.wrapped_imports.get(name)
    }

    pub(crate) fn global_id(&self, name: &str) -> Option<DataId> {
        self.global_ids.get(name).copied()
    }
}

/// First pass: declare all functions and globals at module scope.
pub(crate) fn declare_module_symbols(
    module: &mut ObjectModule,
    program: &MirProgram,
    type_lowering: &MirTypeLowering,
) -> Result<ModuleSymbols, BackendError> {
    let mut symbols = ModuleSymbols::default();

    for ext in &program.extern_functions {
        if ext.boundary_sig.has_unsupported_aggregate() {
            return Err(BackendError::UnsupportedFunctionLowering {
                function: ext.name.clone(),
                message: "unsupported x64 SysV aggregate ABI classification at extern boundary"
                    .to_string(),
            });
        }
        if ext.boundary_sig.requires_wrapper() {
            let boundary_signature = type_lowering.lower_boundary_signature(
                module.isa().default_call_conv(),
                &ext.boundary_sig,
                &ext.name,
            )?;
            let import_id =
                module.declare_function(&ext.name, Linkage::Import, &boundary_signature)?;
            let internal_signature = type_lowering.lower_signature(
                module.isa().default_call_conv(),
                &ext.sig.params,
                ext.sig.return_type,
                ext.sig.variadic,
                &ext.name,
            )?;
            let wrapper_name = import_wrapper_name(&ext.name);
            let wrapper_id =
                module.declare_function(&wrapper_name, Linkage::Local, &internal_signature)?;
            symbols.function_ids.insert(ext.name.clone(), wrapper_id);
            symbols
                .addressable_function_ids
                .insert(ext.name.clone(), import_id);
            symbols
                .import_function_ids
                .insert(ext.name.clone(), import_id);
            symbols
                .wrapped_imports
                .insert(ext.name.clone(), ext.clone());
        } else {
            let signature = type_lowering.lower_signature(
                module.isa().default_call_conv(),
                &ext.sig.params,
                ext.sig.return_type,
                ext.sig.variadic,
                &ext.name,
            )?;
            let func_id = module.declare_function(&ext.name, Linkage::Import, &signature)?;
            symbols.function_ids.insert(ext.name.clone(), func_id);
            symbols
                .addressable_function_ids
                .insert(ext.name.clone(), func_id);
            symbols
                .import_function_ids
                .insert(ext.name.clone(), func_id);
        }
    }

    for function in &program.functions {
        if function.boundary_sig.has_unsupported_aggregate() {
            return Err(BackendError::UnsupportedFunctionLowering {
                function: function.name.clone(),
                message: "unsupported x64 SysV aggregate ABI classification at function boundary"
                    .to_string(),
            });
        }
        let signature = type_lowering.lower_signature(
            module.isa().default_call_conv(),
            &function.params,
            function.return_type,
            function.variadic,
            &function.name,
        )?;
        if function.boundary_sig.requires_wrapper() {
            let body_name = internal_body_name(&function.name);
            let body_id = module.declare_function(&body_name, Linkage::Local, &signature)?;
            let boundary_signature = type_lowering.lower_boundary_signature(
                module.isa().default_call_conv(),
                &function.boundary_sig,
                &function.name,
            )?;
            let (wrapper_name, wrapper_linkage) = if function.linkage == MirLinkage::External {
                (function.name.clone(), Linkage::Export)
            } else {
                (local_wrapper_name(&function.name), Linkage::Local)
            };
            let wrapper_id =
                module.declare_function(&wrapper_name, wrapper_linkage, &boundary_signature)?;
            symbols.function_ids.insert(function.name.clone(), body_id);
            symbols
                .addressable_function_ids
                .insert(function.name.clone(), wrapper_id);
            symbols
                .wrapper_function_ids
                .insert(function.name.clone(), wrapper_id);
        } else {
            let linkage = module_linkage_for_function(function.linkage);
            let func_id = module.declare_function(&function.name, linkage, &signature)?;
            symbols.function_ids.insert(function.name.clone(), func_id);
            symbols
                .addressable_function_ids
                .insert(function.name.clone(), func_id);
        }
    }

    for global in &program.globals {
        let linkage = module_linkage_for_global(global)?;
        let writable = module_writable_for_global(global);
        let data_id = module.declare_data(&global.name, linkage, writable, false)?;
        symbols.global_ids.insert(global.name.clone(), data_id);
    }

    Ok(symbols)
}

fn internal_body_name(name: &str) -> String {
    format!("__rcc_body_{name}")
}

fn local_wrapper_name(name: &str) -> String {
    format!("__rcc_wrap_{name}")
}

fn import_wrapper_name(name: &str) -> String {
    format!("__rcc_import_{name}")
}

/// Second pass: define global objects that have initializers.
pub(crate) fn define_global_objects(
    module: &mut ObjectModule,
    symbols: &ModuleSymbols,
    program: &MirProgram,
) -> Result<(), BackendError> {
    let mut data_description = DataDescription::new();
    for global in &program.globals {
        let Some(init) = &global.init else {
            continue;
        };
        let data_id = symbols
            .global_id(&global.name)
            .ok_or_else(|| BackendError::MissingGlobalSymbol(global.name.clone()))?;
        define_global_object(
            module,
            symbols,
            global,
            init,
            data_id,
            &mut data_description,
        )?;
    }
    Ok(())
}

fn module_linkage_for_global(global: &MirGlobal) -> Result<Linkage, BackendError> {
    match (global.linkage, global.init.is_some()) {
        (MirLinkage::External, true) => Ok(Linkage::Export),
        (MirLinkage::External, false) => Ok(Linkage::Import),
        (MirLinkage::Internal, true) => Ok(Linkage::Local),
        (MirLinkage::Internal, false) => {
            Err(BackendError::InvalidGlobalDeclaration(global.name.clone()))
        }
    }
}

fn module_linkage_for_function(linkage: MirLinkage) -> Linkage {
    match linkage {
        MirLinkage::External => Linkage::Export,
        MirLinkage::Internal => Linkage::Local,
    }
}

fn module_writable_for_global(global: &MirGlobal) -> bool {
    // MIR does not carry object-level constness yet. As an immediate safety fix,
    // keep interned string literals in read-only sections.
    !global.name.starts_with(".str.")
}

fn define_global_object(
    module: &mut ObjectModule,
    symbols: &ModuleSymbols,
    global: &MirGlobal,
    init: &MirGlobalInit,
    data_id: DataId,
    data_description: &mut DataDescription,
) -> Result<(), BackendError> {
    data_description.clear();

    if global.alignment > 1 {
        if !global.alignment.is_power_of_two() {
            return Err(BackendError::InvalidGlobalInitializer {
                global: global.name.clone(),
                message: format!("alignment {} is not a power of two", global.alignment),
            });
        }
        data_description.set_align(u64::from(global.alignment));
    }

    match init {
        MirGlobalInit::Zero => {
            let size = usize::try_from(global.size).map_err(|_| {
                BackendError::InvalidGlobalInitializer {
                    global: global.name.clone(),
                    message: format!("size {} does not fit usize", global.size),
                }
            })?;
            data_description.define_zeroinit(size);
        }
        MirGlobalInit::Data(bytes) => {
            let bytes = build_initialized_bytes(global, bytes)?;
            data_description.define(bytes.into_boxed_slice());
        }
        MirGlobalInit::RelocatedData { bytes, relocations } => {
            let bytes = build_initialized_bytes(global, bytes)?;
            data_description.define(bytes.into_boxed_slice());
            apply_relocations(module, symbols, global, relocations, data_description)?;
        }
    }

    module.define_data(data_id, data_description)?;
    Ok(())
}

fn build_initialized_bytes(global: &MirGlobal, bytes: &[u8]) -> Result<Vec<u8>, BackendError> {
    let expected_size =
        usize::try_from(global.size).map_err(|_| BackendError::InvalidGlobalInitializer {
            global: global.name.clone(),
            message: format!("size {} does not fit usize", global.size),
        })?;
    if bytes.len() > expected_size {
        return Err(BackendError::InvalidGlobalInitializer {
            global: global.name.clone(),
            message: format!(
                "initializer is too large: {} bytes for {}-byte object",
                bytes.len(),
                expected_size
            ),
        });
    }
    let mut padded = Vec::with_capacity(expected_size);
    padded.extend_from_slice(bytes);
    padded.resize(expected_size, 0);
    Ok(padded)
}

fn apply_relocations(
    module: &mut ObjectModule,
    symbols: &ModuleSymbols,
    global: &MirGlobal,
    relocations: &[crate::mir::ir::MirRelocation],
    data_description: &mut DataDescription,
) -> Result<(), BackendError> {
    for relocation in relocations {
        let end =
            relocation
                .offset
                .checked_add(8)
                .ok_or_else(|| BackendError::InvalidRelocation {
                    global: global.name.clone(),
                    message: format!("relocation offset {} overflows", relocation.offset),
                })?;
        if end > global.size {
            return Err(BackendError::InvalidRelocation {
                global: global.name.clone(),
                message: format!(
                    "relocation at offset {} exceeds {}-byte object size",
                    relocation.offset, global.size
                ),
            });
        }

        let offset =
            u32::try_from(relocation.offset).map_err(|_| BackendError::InvalidRelocation {
                global: global.name.clone(),
                message: format!("relocation offset {} exceeds u32 range", relocation.offset),
            })?;

        match &relocation.target {
            MirRelocationTarget::Global(name) => {
                let target_id = symbols
                    .global_id(name)
                    .ok_or_else(|| BackendError::MissingGlobalSymbol(name.clone()))?;
                let target = module.declare_data_in_data(target_id, data_description);
                data_description.write_data_addr(offset, target, relocation.addend);
            }
            MirRelocationTarget::Function(name) => {
                let target_id = symbols
                    .addressable_function_id(name)
                    .ok_or_else(|| BackendError::MissingFunctionSymbol(name.clone()))?;
                let target = data_description
                    .import_global_value(ModuleRelocTarget::user(0, target_id.as_u32()));
                data_description.write_data_addr(offset, target, relocation.addend);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_string_literal_globals_readonly() {
        let string_global = MirGlobal {
            name: ".str.1".to_string(),
            size: 4,
            alignment: 1,
            linkage: MirLinkage::Internal,
            init: Some(MirGlobalInit::Data(b"abc\0".to_vec())),
        };

        assert!(!module_writable_for_global(&string_global));
    }

    #[test]
    fn keeps_non_string_globals_writable() {
        let global = MirGlobal {
            name: "g".to_string(),
            size: 4,
            alignment: 4,
            linkage: MirLinkage::External,
            init: Some(MirGlobalInit::Data(vec![1, 0, 0, 0])),
        };

        assert!(module_writable_for_global(&global));
    }
}
