use super::BackendError;
use super::symbols::ModuleSymbols;
use crate::mir::ir::{
    BasicBlock, BinaryOp, BlockId, CastKind, CmpDomain, CmpKind, Instruction, MirAbiParam,
    MirAbiParamPurpose, MirBoundaryParam, MirBoundaryReturn, MirBoundarySignature, MirConst,
    MirExternFunction, MirFunction, MirFunctionSig, MirType, Operand, SlotId,
    StackSlot as MirStackSlot, SwitchCase, Terminator, VReg,
};
use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::immediates::{Ieee32, Ieee64};
use cranelift_codegen::ir::{
    self, AbiParam, ArgumentPurpose, InstBuilder, MemFlags, TrapCode, types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch as ClifSwitch, Variable};
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_object::ObjectModule;
use std::collections::HashMap;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct MirTypeLowering;

impl MirTypeLowering {
    pub(crate) fn lower_signature(
        &self,
        call_conv: CallConv,
        params: &[MirAbiParam],
        return_type: MirType,
        variadic: bool,
        function_name: &str,
    ) -> Result<ir::Signature, BackendError> {
        let _ = variadic;
        let _ = function_name;
        let mut signature = ir::Signature::new(call_conv);
        for &param in params {
            signature.params.push(self.lower_param_abi_type(param)?);
        }
        if return_type != MirType::Void {
            signature
                .returns
                .push(self.lower_return_abi_type(return_type)?);
        }
        Ok(signature)
    }

    pub(crate) fn lower_boundary_signature(
        &self,
        call_conv: CallConv,
        signature: &MirBoundarySignature,
        function_name: &str,
    ) -> Result<ir::Signature, BackendError> {
        let mut lowered = ir::Signature::new(call_conv);

        if let MirBoundaryReturn::AggregateMemory { .. } = signature.return_type {
            lowered.params.push(AbiParam::special(
                self.lower_value_type(MirType::Ptr, "boundary sret parameter")?,
                ArgumentPurpose::StructReturn,
            ));
        }

        for param in &signature.params {
            match param {
                MirBoundaryParam::Scalar(ty) => lowered.params.push(AbiParam::new(
                    self.lower_value_type(*ty, "boundary parameter type")?,
                )),
                MirBoundaryParam::AggregateScalarized { parts, .. } => {
                    for &part in parts {
                        lowered.params.push(AbiParam::new(
                            self.lower_value_type(part, "boundary aggregate part")?,
                        ));
                    }
                }
                MirBoundaryParam::AggregateMemory { abi_size, .. } => lowered.params.push(
                    AbiParam::special(types::I64, ArgumentPurpose::StructArgument(*abi_size)),
                ),
                MirBoundaryParam::AggregateUnsupported { size } => {
                    return Err(BackendError::UnsupportedFunctionLowering {
                        function: function_name.to_string(),
                        message: format!(
                            "unsupported x64 SysV aggregate parameter classification for {}-byte aggregate",
                            size
                        ),
                    });
                }
            }
        }

        match &signature.return_type {
            MirBoundaryReturn::Void | MirBoundaryReturn::AggregateMemory { .. } => {}
            MirBoundaryReturn::Scalar(ty) => lowered.returns.push(AbiParam::new(
                self.lower_value_type(*ty, "boundary return type")?,
            )),
            MirBoundaryReturn::AggregateScalarized { parts, .. } => {
                for &part in parts {
                    lowered.returns.push(AbiParam::new(
                        self.lower_value_type(part, "boundary aggregate return part")?,
                    ));
                }
            }
            MirBoundaryReturn::AggregateUnsupported { size } => {
                return Err(BackendError::UnsupportedFunctionLowering {
                    function: function_name.to_string(),
                    message: format!(
                        "unsupported x64 SysV aggregate return classification for {}-byte aggregate",
                        size
                    ),
                });
            }
        }

        Ok(lowered)
    }

    pub(crate) fn lower_value_type(
        &self,
        ty: MirType,
        context: &'static str,
    ) -> Result<ir::Type, BackendError> {
        match ty {
            MirType::I8 => Ok(types::I8),
            MirType::I16 => Ok(types::I16),
            MirType::I32 => Ok(types::I32),
            MirType::I64 | MirType::Ptr => Ok(types::I64),
            MirType::F32 => Ok(types::F32),
            MirType::F64 => Ok(types::F64),
            MirType::Void => Err(BackendError::UnsupportedMirType { ty, context }),
        }
    }

    fn lower_param_abi_type(&self, param: MirAbiParam) -> Result<AbiParam, BackendError> {
        let ty = self.lower_value_type(param.ty, "function parameter type")?;
        let abi_param = match param.purpose {
            MirAbiParamPurpose::Normal => AbiParam::new(ty),
            MirAbiParamPurpose::StructArgument { size } => {
                AbiParam::special(ty, ArgumentPurpose::StructArgument(size))
            }
            MirAbiParamPurpose::StructReturn => {
                AbiParam::special(ty, ArgumentPurpose::StructReturn)
            }
        };
        Ok(abi_param)
    }

    fn lower_return_abi_type(&self, ty: MirType) -> Result<AbiParam, BackendError> {
        let ty = self.lower_value_type(ty, "function return type")?;
        Ok(AbiParam::new(ty))
    }
}

pub(crate) struct FunctionLoweringContext {
    clif_context: Context,
    func_builder_context: FunctionBuilderContext,
    type_lowering: MirTypeLowering,
}

struct PreparedFunctionContext {
    stack_slots: HashMap<SlotId, ir::StackSlot>,
}

struct BodyLoweringState<'a> {
    function: &'a MirFunction,
    type_lowering: MirTypeLowering,
    pointer_type: ir::Type,
    block_map: HashMap<BlockId, ir::Block>,
    stack_slot_map: HashMap<SlotId, ir::StackSlot>,
    vreg_types: HashMap<VReg, ir::Type>,
    vreg_vars: HashMap<VReg, Variable>,
}

impl<'a> BodyLoweringState<'a> {
    fn new(
        builder: &mut FunctionBuilder<'_>,
        function: &'a MirFunction,
        prepared: PreparedFunctionContext,
        type_lowering: MirTypeLowering,
        pointer_type: ir::Type,
    ) -> Result<Self, BackendError> {
        let mut block_map = HashMap::with_capacity(function.blocks.len());
        for block in &function.blocks {
            let clif_block = builder.create_block();
            if block_map.insert(block.id, clif_block).is_some() {
                return Err(Self::function_error(
                    function,
                    format!("duplicate MIR block id {}", block.id.0),
                ));
            }
        }

        let vreg_mir_types = collect_vreg_types(function)?;
        let mut vreg_types = HashMap::with_capacity(vreg_mir_types.len());
        let mut vreg_ids: Vec<u32> = vreg_mir_types.keys().map(|vreg| vreg.0).collect();
        vreg_ids.sort_unstable();

        let mut vreg_vars = HashMap::with_capacity(vreg_ids.len());
        for reg_id in vreg_ids {
            let vreg = VReg(reg_id);
            let mir_ty = *vreg_mir_types.get(&vreg).expect("vreg type should exist");
            let clif_ty = type_lowering.lower_value_type(mir_ty, "virtual register type")?;
            let var = builder.declare_var(clif_ty);
            vreg_types.insert(vreg, clif_ty);
            vreg_vars.insert(vreg, var);
        }

        let mut state = Self {
            function,
            type_lowering,
            pointer_type,
            block_map,
            stack_slot_map: prepared.stack_slots,
            vreg_types,
            vreg_vars,
        };
        state.define_entry_params(builder)?;
        Ok(state)
    }

    fn lower_blocks(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        module: &mut ObjectModule,
        symbols: &ModuleSymbols,
    ) -> Result<(), BackendError> {
        for block in &self.function.blocks {
            self.lower_block(builder, module, symbols, block)?;
        }
        Ok(())
    }

    fn lower_block(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        module: &mut ObjectModule,
        symbols: &ModuleSymbols,
        block: &BasicBlock,
    ) -> Result<(), BackendError> {
        let clif_block = self.block(block.id)?;
        builder.switch_to_block(clif_block);

        for instruction in &block.instructions {
            self.lower_instruction(builder, module, symbols, block.id, instruction)?;
        }
        self.lower_terminator(builder, block.id, &block.terminator)
    }

    fn lower_instruction(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        module: &mut ObjectModule,
        symbols: &ModuleSymbols,
        block_id: BlockId,
        instruction: &Instruction,
    ) -> Result<(), BackendError> {
        match instruction {
            Instruction::Load {
                dst,
                slot,
                offset,
                volatile,
            } => {
                let slot = self.stack_slot(*slot)?;
                let clif_ty = self
                    .type_lowering
                    .lower_value_type(dst.ty, "load destination")?;
                let base = builder.ins().stack_addr(self.pointer_type, slot, 0);
                let flags = mem_flags(*volatile);
                let value = builder
                    .ins()
                    .load(clif_ty, flags, base, offset_to_i32(*offset)?);
                self.def_vreg(builder, dst.reg, value)
            }
            Instruction::Store {
                slot,
                offset,
                value,
                ty,
                volatile,
            } => {
                let slot = self.stack_slot(*slot)?;
                let clif_ty = self
                    .type_lowering
                    .lower_value_type(*ty, "stack store value type")?;
                let stored_value = self.lower_operand_value(builder, value, clif_ty)?;
                let base = builder.ins().stack_addr(self.pointer_type, slot, 0);
                let flags = mem_flags(*volatile);
                builder
                    .ins()
                    .store(flags, stored_value, base, offset_to_i32(*offset)?);
                Ok(())
            }
            Instruction::PtrLoad {
                dst,
                ptr,
                ty,
                volatile,
            } => {
                let ptr_value = self.lower_operand_value(builder, ptr, self.pointer_type)?;
                let clif_ty = self
                    .type_lowering
                    .lower_value_type(*ty, "pointer load value type")?;
                let flags = mem_flags(*volatile);
                let value = builder.ins().load(clif_ty, flags, ptr_value, 0);
                self.def_vreg(builder, dst.reg, value)
            }
            Instruction::PtrStore {
                ptr,
                value,
                ty,
                volatile,
            } => {
                let ptr_value = self.lower_operand_value(builder, ptr, self.pointer_type)?;
                let clif_ty = self
                    .type_lowering
                    .lower_value_type(*ty, "pointer store value type")?;
                let stored_value = self.lower_operand_value(builder, value, clif_ty)?;
                let flags = mem_flags(*volatile);
                builder.ins().store(flags, stored_value, ptr_value, 0);
                Ok(())
            }
            Instruction::Memcpy {
                dst_ptr,
                src_ptr,
                size,
            } => {
                let dst = self.lower_operand_value(builder, dst_ptr, self.pointer_type)?;
                let src = self.lower_operand_value(builder, src_ptr, self.pointer_type)?;
                let size = builder.ins().iconst(self.pointer_type, i64::from(*size));
                builder.call_memcpy(module.target_config(), dst, src, size);
                Ok(())
            }
            Instruction::Memset {
                dst_ptr,
                value,
                size,
            } => {
                let dst = self.lower_operand_value(builder, dst_ptr, self.pointer_type)?;
                let byte = self.lower_operand_value(builder, value, types::I8)?;
                let size = builder.ins().iconst(self.pointer_type, i64::from(*size));
                builder.call_memset(module.target_config(), dst, byte, size);
                Ok(())
            }
            Instruction::Binary {
                dst,
                op,
                lhs,
                rhs,
                ty,
            } => {
                let clif_ty = self
                    .type_lowering
                    .lower_value_type(*ty, "binary operand type")?;
                let lhs = self.lower_operand_value(builder, lhs, clif_ty)?;
                let rhs = self.lower_operand_value(builder, rhs, clif_ty)?;
                let value = match op {
                    BinaryOp::Add => builder.ins().iadd(lhs, rhs),
                    BinaryOp::Sub => builder.ins().isub(lhs, rhs),
                    BinaryOp::Mul => builder.ins().imul(lhs, rhs),
                    BinaryOp::SDiv => builder.ins().sdiv(lhs, rhs),
                    BinaryOp::UDiv => builder.ins().udiv(lhs, rhs),
                    BinaryOp::SRem => builder.ins().srem(lhs, rhs),
                    BinaryOp::URem => builder.ins().urem(lhs, rhs),
                    BinaryOp::And => builder.ins().band(lhs, rhs),
                    BinaryOp::Or => builder.ins().bor(lhs, rhs),
                    BinaryOp::Xor => builder.ins().bxor(lhs, rhs),
                    BinaryOp::Shl => builder.ins().ishl(lhs, rhs),
                    BinaryOp::AShr => builder.ins().sshr(lhs, rhs),
                    BinaryOp::LShr => builder.ins().ushr(lhs, rhs),
                    BinaryOp::FAdd => builder.ins().fadd(lhs, rhs),
                    BinaryOp::FSub => builder.ins().fsub(lhs, rhs),
                    BinaryOp::FMul => builder.ins().fmul(lhs, rhs),
                    BinaryOp::FDiv => builder.ins().fdiv(lhs, rhs),
                    BinaryOp::FRem => {
                        self.emit_frem_call(builder, module, block_id, clif_ty, lhs, rhs)?
                    }
                };
                self.def_vreg(builder, dst.reg, value)
            }
            Instruction::Unary {
                dst,
                op,
                operand,
                ty,
            } => {
                let clif_ty = self
                    .type_lowering
                    .lower_value_type(*ty, "unary operand type")?;
                let operand = self.lower_operand_value(builder, operand, clif_ty)?;
                let value = match op {
                    crate::mir::ir::UnaryOp::Neg if clif_ty.is_float() => {
                        builder.ins().fneg(operand)
                    }
                    crate::mir::ir::UnaryOp::Neg if clif_ty.is_int() => builder.ins().ineg(operand),
                    crate::mir::ir::UnaryOp::Not if clif_ty.is_int() => builder.ins().bnot(operand),
                    crate::mir::ir::UnaryOp::Neg => {
                        return Err(Self::block_error(
                            self.function,
                            block_id,
                            "neg requires integer or floating-point operand",
                        ));
                    }
                    crate::mir::ir::UnaryOp::Not => {
                        return Err(Self::block_error(
                            self.function,
                            block_id,
                            "bitwise not requires integer operand",
                        ));
                    }
                };
                self.def_vreg(builder, dst.reg, value)
            }
            Instruction::Cmp {
                dst,
                kind,
                domain,
                lhs,
                rhs,
                ty,
            } => {
                let clif_ty = self
                    .type_lowering
                    .lower_value_type(*ty, "comparison operand type")?;
                let lhs = self.lower_operand_value(builder, lhs, clif_ty)?;
                let rhs = self.lower_operand_value(builder, rhs, clif_ty)?;
                let pred = match domain {
                    CmpDomain::Signed => {
                        let cc = int_cc(*kind, true);
                        builder.ins().icmp(cc, lhs, rhs)
                    }
                    CmpDomain::Unsigned => {
                        let cc = int_cc(*kind, false);
                        builder.ins().icmp(cc, lhs, rhs)
                    }
                    CmpDomain::Float => {
                        let cc = float_cc(*kind);
                        builder.ins().fcmp(cc, lhs, rhs)
                    }
                };
                let one = builder.ins().iconst(types::I8, 1);
                let zero = builder.ins().iconst(types::I8, 0);
                let normalized = builder.ins().select(pred, one, zero);
                self.def_vreg(builder, dst.reg, normalized)
            }
            Instruction::Cast {
                dst,
                kind,
                src,
                from_ty,
                to_ty,
            } => {
                let from_clif = self
                    .type_lowering
                    .lower_value_type(*from_ty, "cast source type")?;
                let to_clif = self
                    .type_lowering
                    .lower_value_type(*to_ty, "cast destination type")?;
                let src = self.lower_operand_value(builder, src, from_clif)?;

                let value = match kind {
                    CastKind::Trunc => builder.ins().ireduce(to_clif, src),
                    CastKind::ZExt => builder.ins().uextend(to_clif, src),
                    CastKind::SExt => builder.ins().sextend(to_clif, src),
                    CastKind::SIToF => builder.ins().fcvt_from_sint(to_clif, src),
                    CastKind::UIToF => builder.ins().fcvt_from_uint(to_clif, src),
                    CastKind::FToSI => builder.ins().fcvt_to_sint(to_clif, src),
                    CastKind::FToUI => builder.ins().fcvt_to_uint(to_clif, src),
                    CastKind::FExt => builder.ins().fpromote(to_clif, src),
                    CastKind::FTrunc => builder.ins().fdemote(to_clif, src),
                    CastKind::PToI | CastKind::IToP => {
                        coerce_integer_value(builder, src, from_clif, to_clif)
                    }
                };
                self.def_vreg(builder, dst.reg, value)
            }
            Instruction::Call {
                dst,
                callee,
                args,
                fixed_arg_count,
            } => {
                if let Some(wrapped_import) = symbols.wrapped_import(callee)
                    && wrapped_import.sig.variadic
                {
                    let import_id = symbols
                        .import_function_id(callee)
                        .ok_or_else(|| BackendError::MissingFunctionSymbol(callee.clone()))?;
                    return self.lower_direct_import_call_with_boundary_abi(
                        builder,
                        module,
                        block_id,
                        dst,
                        import_id,
                        args,
                        wrapped_import,
                        fixed_arg_count,
                    );
                }
                let Some(callee_id) = symbols.function_id(callee) else {
                    return Err(BackendError::MissingFunctionSymbol(callee.clone()));
                };
                let declared_sig = module
                    .declarations()
                    .get_function_decl(callee_id)
                    .signature
                    .clone();
                let mut call_sig = declared_sig.clone();
                let mut expected_arg_tys: Vec<ir::Type> = declared_sig
                    .params
                    .iter()
                    .map(|param| param.value_type)
                    .collect();
                let local_callee = if let Some(fixed) = fixed_arg_count {
                    if *fixed > args.len() {
                        return Err(Self::block_error(
                            self.function,
                            block_id,
                            format!(
                                "invalid variadic call to '{}': fixed argument count {} exceeds total {}",
                                callee,
                                fixed,
                                args.len()
                            ),
                        ));
                    }
                    if *fixed != expected_arg_tys.len() {
                        return Err(Self::block_error(
                            self.function,
                            block_id,
                            format!(
                                "invalid variadic call to '{}': fixed argument count {} does not match declared parameter count {}",
                                callee,
                                fixed,
                                expected_arg_tys.len()
                            ),
                        ));
                    }
                    for arg in args.iter().skip(*fixed) {
                        let arg_ty = self.infer_variadic_arg_type(arg, block_id)?;
                        expected_arg_tys.push(arg_ty);
                        call_sig.params.push(AbiParam::new(arg_ty));
                    }
                    self.import_direct_callee_with_signature(builder, module, callee_id, call_sig)
                } else {
                    if args.len() != expected_arg_tys.len() {
                        return Err(Self::block_error(
                            self.function,
                            block_id,
                            format!(
                                "call argument count mismatch for '{}': got {}, expected {}",
                                callee,
                                args.len(),
                                expected_arg_tys.len()
                            ),
                        ));
                    }
                    module.declare_func_in_func(callee_id, builder.func)
                };

                let mut lowered_args = Vec::with_capacity(args.len());
                for (arg, expected_ty) in args.iter().zip(expected_arg_tys.into_iter()) {
                    lowered_args.push(self.lower_operand_value(builder, arg, expected_ty)?);
                }
                let call = builder.ins().call(local_callee, &lowered_args);
                self.assign_call_result(builder, dst, call)
            }
            Instruction::CallIndirect {
                dst,
                callee_ptr,
                args,
                sig,
                boundary_sig,
                fixed_arg_count,
            } => self.lower_call_indirect(
                builder,
                module,
                block_id,
                dst,
                callee_ptr,
                args,
                sig,
                boundary_sig,
                fixed_arg_count,
            ),
            Instruction::SlotAddr { dst, slot } => {
                let slot = self.stack_slot(*slot)?;
                let addr = builder.ins().stack_addr(self.pointer_type, slot, 0);
                self.def_vreg(builder, dst.reg, addr)
            }
            Instruction::GlobalAddr { dst, global } => {
                let addr = if let Some(global_id) = symbols.global_id(global) {
                    let symbol = module.declare_data_in_func(global_id, builder.func);
                    builder.ins().symbol_value(self.pointer_type, symbol)
                } else if let Some(func_id) = symbols.addressable_function_id(global) {
                    let func_ref = module.declare_func_in_func(func_id, builder.func);
                    builder.ins().func_addr(self.pointer_type, func_ref)
                } else {
                    return Err(BackendError::UnsupportedFunctionLowering {
                        function: self.function.name.clone(),
                        message: format!(
                            "global_addr target '{}' was not declared as global or function symbol",
                            global
                        ),
                    });
                };
                self.def_vreg(builder, dst.reg, addr)
            }
            Instruction::PtrAdd {
                dst,
                base,
                byte_offset,
            } => {
                let base = self.lower_operand_value(builder, base, self.pointer_type)?;
                let byte_offset =
                    self.lower_operand_value(builder, byte_offset, self.pointer_type)?;
                let value = builder.ins().iadd(base, byte_offset);
                self.def_vreg(builder, dst.reg, value)
            }
            Instruction::Copy { dst, src } => {
                let clif_ty = self
                    .type_lowering
                    .lower_value_type(dst.ty, "copy destination type")?;
                let value = self.lower_operand_value(builder, src, clif_ty)?;
                self.def_vreg(builder, dst.reg, value)
            }
        }
    }

    fn lower_terminator(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        block_id: BlockId,
        terminator: &Terminator,
    ) -> Result<(), BackendError> {
        match terminator {
            Terminator::Jump(target) => {
                let target = self.block(*target)?;
                builder.ins().jump(target, &[]);
                Ok(())
            }
            Terminator::Branch {
                cond,
                then_bb,
                else_bb,
            } => {
                let cond_ty = *self.vreg_types.get(cond).ok_or_else(|| {
                    Self::block_error(
                        self.function,
                        block_id,
                        format!("branch condition v{} has no declared type", cond.0),
                    )
                })?;
                if !cond_ty.is_int() {
                    return Err(Self::block_error(
                        self.function,
                        block_id,
                        format!("branch condition v{} is not an integer", cond.0),
                    ));
                }
                let cond = self.use_vreg(builder, *cond)?;
                let then_block = self.block(*then_bb)?;
                let else_block = self.block(*else_bb)?;
                builder.ins().brif(cond, then_block, &[], else_block, &[]);
                Ok(())
            }
            Terminator::Switch {
                discr,
                cases,
                default,
            } => self.lower_switch_terminator(builder, block_id, *discr, cases, *default),
            Terminator::Ret(value) => {
                match (self.function.return_type, value) {
                    (MirType::Void, None) => {
                        builder.ins().return_(&[]);
                    }
                    (MirType::Void, Some(_)) => {
                        return Err(Self::block_error(
                            self.function,
                            block_id,
                            "void function cannot return a value",
                        ));
                    }
                    (return_ty, Some(value)) => {
                        let clif_ty = self
                            .type_lowering
                            .lower_value_type(return_ty, "return value type")?;
                        let value = self.lower_operand_value(builder, value, clif_ty)?;
                        builder.ins().return_(&[value]);
                    }
                    (_return_ty, None) => {
                        return Err(Self::block_error(
                            self.function,
                            block_id,
                            "non-void function must return a value",
                        ));
                    }
                }
                Ok(())
            }
            Terminator::Unreachable => {
                builder.ins().trap(TrapCode::unwrap_user(1));
                Ok(())
            }
        }
    }

    fn define_entry_params(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
    ) -> Result<(), BackendError> {
        let Some(entry_block_id) = self.function.blocks.first().map(|block| block.id) else {
            return Err(Self::function_error(
                self.function,
                "function has no basic blocks",
            ));
        };
        let entry_block = self.block(entry_block_id)?;
        builder.switch_to_block(entry_block);
        builder.append_block_params_for_function_params(entry_block);

        let params: Vec<ir::Value> = builder.block_params(entry_block).to_vec();
        if params.len() != self.function.params.len() {
            return Err(Self::function_error(
                self.function,
                format!(
                    "entry parameter count mismatch: got {}, expected {}",
                    params.len(),
                    self.function.params.len()
                ),
            ));
        }

        for (idx, value) in params.iter().copied().enumerate() {
            let reg = VReg(idx as u32);
            self.def_vreg(builder, reg, value)?;
        }

        Ok(())
    }

    fn assign_call_result(
        &self,
        builder: &mut FunctionBuilder<'_>,
        dst: &Option<crate::mir::ir::TypedVReg>,
        call: ir::Inst,
    ) -> Result<(), BackendError> {
        let results: Vec<ir::Value> = builder.inst_results(call).to_vec();
        match (dst, results.len()) {
            (Some(dst), 1) => self.def_vreg(builder, dst.reg, results[0]),
            (Some(_), 0) => Err(Self::function_error(
                self.function,
                "call has no return value but destination register was provided",
            )),
            (Some(_), n) => Err(Self::function_error(
                self.function,
                format!("call returned {n} values, expected exactly 1"),
            )),
            (None, _) => Ok(()),
        }
    }

    fn lower_direct_import_call_with_boundary_abi(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        module: &mut ObjectModule,
        block_id: BlockId,
        dst: &Option<crate::mir::ir::TypedVReg>,
        import_id: FuncId,
        args: &[Operand],
        function: &MirExternFunction,
        fixed_arg_count: &Option<usize>,
    ) -> Result<(), BackendError> {
        let fixed_internal_arg_tys: Vec<ir::Type> = function
            .sig
            .params
            .iter()
            .map(|param| self.type_lowering.lower_param_abi_type(*param))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                Self::block_error(
                    self.function,
                    block_id,
                    format!("invalid call signature for '{}': {err}", function.name),
                )
            })?
            .into_iter()
            .map(|param| param.value_type)
            .collect();
        let fixed_internal_arg_count = fixed_internal_arg_tys.len();

        if let Some(fixed) = fixed_arg_count {
            if *fixed > args.len() {
                return Err(Self::block_error(
                    self.function,
                    block_id,
                    format!(
                        "invalid variadic call to '{}': fixed argument count {} exceeds total {}",
                        function.name,
                        fixed,
                        args.len()
                    ),
                ));
            }
            if *fixed != fixed_internal_arg_count {
                return Err(Self::block_error(
                    self.function,
                    block_id,
                    format!(
                        "invalid variadic call to '{}': fixed argument count {} does not match declared parameter count {}",
                        function.name, fixed, fixed_internal_arg_count
                    ),
                ));
            }
        } else if args.len() != fixed_internal_arg_count {
            return Err(Self::block_error(
                self.function,
                block_id,
                format!(
                    "call argument count mismatch for '{}': got {}, expected {}",
                    function.name,
                    args.len(),
                    fixed_internal_arg_count
                ),
            ));
        }

        let mut fixed_internal_args = Vec::with_capacity(fixed_internal_arg_count);
        for (arg, expected_ty) in args.iter().zip(fixed_internal_arg_tys.iter().copied()) {
            fixed_internal_args.push(self.lower_operand_value(builder, arg, expected_ty)?);
        }

        let mut call_sig = self
            .type_lowering
            .lower_boundary_signature(
                builder.func.signature.call_conv,
                &function.boundary_sig,
                &function.name,
            )
            .map_err(|err| {
                Self::block_error(
                    self.function,
                    block_id,
                    format!(
                        "invalid boundary call signature for '{}': {err}",
                        function.name
                    ),
                )
            })?;

        let mut internal_cursor = 0usize;
        let internal_sret = if function
            .sig
            .params
            .first()
            .is_some_and(|param| param.purpose == MirAbiParamPurpose::StructReturn)
        {
            let value = *fixed_internal_args.get(internal_cursor).ok_or_else(|| {
                Self::block_error(
                    self.function,
                    block_id,
                    format!("missing internal sret argument for '{}'", function.name),
                )
            })?;
            internal_cursor += 1;
            Some(value)
        } else {
            None
        };

        let mut boundary_args = Vec::new();
        if matches!(
            function.boundary_sig.return_type,
            MirBoundaryReturn::AggregateMemory { .. }
        ) {
            let sret = internal_sret.ok_or_else(|| {
                Self::block_error(
                    self.function,
                    block_id,
                    format!(
                        "boundary sret return does not match internal signature for '{}'",
                        function.name
                    ),
                )
            })?;
            boundary_args.push(sret);
        }

        for (param_index, param) in function.boundary_sig.params.iter().enumerate() {
            match param {
                MirBoundaryParam::Scalar(_) | MirBoundaryParam::AggregateMemory { .. } => {
                    let value = *fixed_internal_args.get(internal_cursor).ok_or_else(|| {
                        Self::block_error(
                            self.function,
                            block_id,
                            format!(
                                "missing internal parameter {} for boundary call to '{}'",
                                param_index, function.name
                            ),
                        )
                    })?;
                    internal_cursor += 1;
                    boundary_args.push(value);
                }
                MirBoundaryParam::AggregateScalarized { parts, size } => {
                    let aggregate_ptr =
                        *fixed_internal_args.get(internal_cursor).ok_or_else(|| {
                            Self::block_error(
                                self.function,
                                block_id,
                                format!(
                                    "missing aggregate pointer parameter {} for boundary call to '{}'",
                                    param_index, function.name
                                ),
                            )
                        })?;
                    internal_cursor += 1;
                    for (part_index, part_ty) in parts.iter().copied().enumerate() {
                        let part_offset = (part_index as u32) * 8;
                        let copy_size = lane_copy_size(*size, part_index);
                        let value = load_boundary_part_bytes(
                            &self.type_lowering,
                            builder,
                            module,
                            self.pointer_type,
                            aggregate_ptr,
                            i64::from(part_offset),
                            part_ty,
                            copy_size,
                        )?;
                        boundary_args.push(value);
                    }
                }
                MirBoundaryParam::AggregateUnsupported { size } => {
                    return Err(Self::block_error(
                        self.function,
                        block_id,
                        format!(
                            "unsupported x64 SysV aggregate parameter classification for {}-byte aggregate",
                            size
                        ),
                    ));
                }
            }
        }

        if internal_cursor != fixed_internal_arg_count {
            return Err(Self::block_error(
                self.function,
                block_id,
                format!(
                    "boundary call parameter mismatch for '{}': consumed {}, expected {}",
                    function.name, internal_cursor, fixed_internal_arg_count
                ),
            ));
        }

        if let Some(fixed) = fixed_arg_count {
            for arg in args.iter().skip(*fixed) {
                let arg_ty = self.infer_variadic_arg_type(arg, block_id)?;
                call_sig.params.push(AbiParam::new(arg_ty));
                boundary_args.push(self.lower_operand_value(builder, arg, arg_ty)?);
            }
        }

        let callee = self.import_direct_callee_with_signature(builder, module, import_id, call_sig);
        let call = builder.ins().call(callee, &boundary_args);

        match &function.boundary_sig.return_type {
            MirBoundaryReturn::Void | MirBoundaryReturn::AggregateMemory { .. } => Ok(()),
            MirBoundaryReturn::Scalar(_) => self.assign_call_result(builder, dst, call),
            MirBoundaryReturn::AggregateScalarized { parts, size } => {
                let sret = internal_sret.ok_or_else(|| {
                    Self::block_error(
                        self.function,
                        block_id,
                        format!(
                            "scalarized aggregate return requires internal sret argument for '{}'",
                            function.name
                        ),
                    )
                })?;
                let results = builder.inst_results(call).to_vec();
                if results.len() != parts.len() {
                    return Err(Self::block_error(
                        self.function,
                        block_id,
                        format!(
                            "scalarized aggregate return part mismatch for '{}': got {}, expected {}",
                            function.name,
                            results.len(),
                            parts.len()
                        ),
                    ));
                }
                for (part_index, (&part_ty, value)) in
                    parts.iter().zip(results.iter().copied()).enumerate()
                {
                    let part_offset = (part_index as u32) * 8;
                    let copy_size = lane_copy_size(*size, part_index);
                    store_boundary_part_bytes(
                        &self.type_lowering,
                        builder,
                        module,
                        self.pointer_type,
                        sret,
                        i64::from(part_offset),
                        value,
                        part_ty,
                        copy_size,
                    )?;
                }
                Ok(())
            }
            MirBoundaryReturn::AggregateUnsupported { size } => Err(Self::block_error(
                self.function,
                block_id,
                format!(
                    "unsupported x64 SysV aggregate return classification for {}-byte aggregate",
                    size
                ),
            )),
        }
    }

    fn lower_operand_value(
        &self,
        builder: &mut FunctionBuilder<'_>,
        operand: &Operand,
        expected_ty: ir::Type,
    ) -> Result<ir::Value, BackendError> {
        match operand {
            Operand::VReg(reg) => {
                let value = self.use_vreg(builder, *reg)?;
                let actual_ty = *self.vreg_types.get(reg).ok_or_else(|| {
                    Self::function_error(
                        self.function,
                        format!("vreg v{} has no known type", reg.0),
                    )
                })?;
                if actual_ty != expected_ty {
                    return Err(Self::function_error(
                        self.function,
                        format!(
                            "type mismatch for operand v{}: expected {}, got {}; insert explicit MIR cast",
                            reg.0, expected_ty, actual_ty
                        ),
                    ));
                }
                Ok(value)
            }
            Operand::Const(value) => {
                lower_const_value(builder, *value, expected_ty).map_err(|message| {
                    Self::function_error(
                        self.function,
                        format!(
                            "constant lowering failed for type {}: {}",
                            expected_ty, message
                        ),
                    )
                })
            }
            Operand::StackSlot(slot) => {
                let slot = self.stack_slot(*slot)?;
                let addr = builder.ins().stack_addr(self.pointer_type, slot, 0);
                if expected_ty == self.pointer_type {
                    Ok(addr)
                } else {
                    Ok(builder.ins().load(expected_ty, mem_flags(false), addr, 0))
                }
            }
        }
    }

    fn lower_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        module: &mut ObjectModule,
        block_id: BlockId,
        dst: &Option<crate::mir::ir::TypedVReg>,
        callee_ptr: &Operand,
        args: &[Operand],
        sig: &MirFunctionSig,
        boundary_sig: &Option<MirBoundarySignature>,
        fixed_arg_count: &Option<usize>,
    ) -> Result<(), BackendError> {
        if let Some(boundary_sig) = boundary_sig {
            if boundary_sig.has_unsupported_aggregate() {
                return Err(Self::block_error(
                    self.function,
                    block_id,
                    "call_indirect crosses unsupported x64 SysV aggregate boundary".to_string(),
                ));
            }
            if boundary_sig.requires_wrapper() {
                return self.lower_call_indirect_with_boundary_abi(
                    builder,
                    module,
                    block_id,
                    dst,
                    callee_ptr,
                    args,
                    sig,
                    boundary_sig,
                    fixed_arg_count,
                );
            }
        }

        let mut call_sig = self
            .type_lowering
            .lower_signature(
                builder.func.signature.call_conv,
                &sig.params,
                sig.return_type,
                sig.variadic,
                "<indirect-call>",
            )
            .map_err(|err| {
                Self::block_error(
                    self.function,
                    block_id,
                    format!("invalid call_indirect signature: {err}"),
                )
            })?;
        let mut expected_arg_types: Vec<ir::Type> = call_sig
            .params
            .iter()
            .map(|param| param.value_type)
            .collect();

        if let Some(fixed) = fixed_arg_count {
            if *fixed > args.len() {
                return Err(Self::block_error(
                    self.function,
                    block_id,
                    format!(
                        "invalid variadic call_indirect: fixed argument count {} exceeds total {}",
                        fixed,
                        args.len()
                    ),
                ));
            }
            if *fixed != expected_arg_types.len() {
                return Err(Self::block_error(
                    self.function,
                    block_id,
                    format!(
                        "invalid variadic call_indirect: fixed argument count {} does not match signature parameter count {}",
                        fixed,
                        expected_arg_types.len()
                    ),
                ));
            }
            for arg in args.iter().skip(*fixed) {
                let arg_ty = self.infer_variadic_arg_type(arg, block_id)?;
                expected_arg_types.push(arg_ty);
                call_sig.params.push(AbiParam::new(arg_ty));
            }
        } else if args.len() != expected_arg_types.len() {
            return Err(Self::block_error(
                self.function,
                block_id,
                format!(
                    "call_indirect argument count mismatch: got {}, expected {}",
                    args.len(),
                    expected_arg_types.len()
                ),
            ));
        }

        let mut lowered_args = Vec::with_capacity(args.len());
        for (arg, expected_ty) in args.iter().zip(expected_arg_types.into_iter()) {
            lowered_args.push(self.lower_operand_value(builder, arg, expected_ty)?);
        }

        let callee_ptr = self.lower_operand_value(builder, callee_ptr, self.pointer_type)?;
        let sig_ref = builder.import_signature(call_sig);
        let call = builder
            .ins()
            .call_indirect(sig_ref, callee_ptr, &lowered_args);
        self.assign_call_result(builder, dst, call)
    }

    fn lower_call_indirect_with_boundary_abi(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        module: &mut ObjectModule,
        block_id: BlockId,
        dst: &Option<crate::mir::ir::TypedVReg>,
        callee_ptr: &Operand,
        args: &[Operand],
        sig: &MirFunctionSig,
        boundary_sig: &MirBoundarySignature,
        fixed_arg_count: &Option<usize>,
    ) -> Result<(), BackendError> {
        let fixed_internal_arg_tys: Vec<ir::Type> = sig
            .params
            .iter()
            .map(|param| self.type_lowering.lower_param_abi_type(*param))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                Self::block_error(
                    self.function,
                    block_id,
                    format!("invalid call_indirect signature: {err}"),
                )
            })?
            .into_iter()
            .map(|param| param.value_type)
            .collect();
        let fixed_internal_arg_count = fixed_internal_arg_tys.len();

        if let Some(fixed) = fixed_arg_count {
            if *fixed > args.len() {
                return Err(Self::block_error(
                    self.function,
                    block_id,
                    format!(
                        "invalid variadic call_indirect: fixed argument count {} exceeds total {}",
                        fixed,
                        args.len()
                    ),
                ));
            }
            if *fixed != fixed_internal_arg_count {
                return Err(Self::block_error(
                    self.function,
                    block_id,
                    format!(
                        "invalid variadic call_indirect: fixed argument count {} does not match signature parameter count {}",
                        fixed, fixed_internal_arg_count
                    ),
                ));
            }
        } else if args.len() != fixed_internal_arg_count {
            return Err(Self::block_error(
                self.function,
                block_id,
                format!(
                    "call_indirect argument count mismatch: got {}, expected {}",
                    args.len(),
                    fixed_internal_arg_count
                ),
            ));
        }

        let mut fixed_internal_args = Vec::with_capacity(fixed_internal_arg_count);
        for (arg, expected_ty) in args.iter().zip(fixed_internal_arg_tys.iter().copied()) {
            fixed_internal_args.push(self.lower_operand_value(builder, arg, expected_ty)?);
        }

        let mut call_sig = self
            .type_lowering
            .lower_boundary_signature(
                builder.func.signature.call_conv,
                boundary_sig,
                "<indirect-call>",
            )
            .map_err(|err| {
                Self::block_error(
                    self.function,
                    block_id,
                    format!("invalid boundary call_indirect signature: {err}"),
                )
            })?;

        let mut internal_cursor = 0usize;
        let internal_sret = if sig
            .params
            .first()
            .is_some_and(|param| param.purpose == MirAbiParamPurpose::StructReturn)
        {
            let value = *fixed_internal_args.get(internal_cursor).ok_or_else(|| {
                Self::block_error(
                    self.function,
                    block_id,
                    "missing internal sret argument for call_indirect".to_string(),
                )
            })?;
            internal_cursor += 1;
            Some(value)
        } else {
            None
        };

        let pointer_type = self.pointer_type;
        let mut boundary_args = Vec::new();
        if matches!(
            boundary_sig.return_type,
            MirBoundaryReturn::AggregateMemory { .. }
        ) {
            let sret = internal_sret.ok_or_else(|| {
                Self::block_error(
                    self.function,
                    block_id,
                    "boundary sret return does not match internal call_indirect signature"
                        .to_string(),
                )
            })?;
            boundary_args.push(sret);
        }

        for (param_index, param) in boundary_sig.params.iter().enumerate() {
            match param {
                MirBoundaryParam::Scalar(_) | MirBoundaryParam::AggregateMemory { .. } => {
                    let value = *fixed_internal_args.get(internal_cursor).ok_or_else(|| {
                        Self::block_error(
                            self.function,
                            block_id,
                            format!(
                                "missing internal parameter {} for boundary call_indirect",
                                param_index
                            ),
                        )
                    })?;
                    internal_cursor += 1;
                    boundary_args.push(value);
                }
                MirBoundaryParam::AggregateScalarized { parts, size } => {
                    let aggregate_ptr =
                        *fixed_internal_args.get(internal_cursor).ok_or_else(|| {
                            Self::block_error(
                                self.function,
                                block_id,
                                format!(
                                    "missing aggregate pointer parameter {} for boundary call_indirect",
                                    param_index
                                ),
                            )
                        })?;
                    internal_cursor += 1;
                    for (part_index, part_ty) in parts.iter().copied().enumerate() {
                        let part_offset = (part_index as u32) * 8;
                        let copy_size = lane_copy_size(*size, part_index);
                        let value = load_boundary_part_bytes(
                            &self.type_lowering,
                            builder,
                            module,
                            pointer_type,
                            aggregate_ptr,
                            i64::from(part_offset),
                            part_ty,
                            copy_size,
                        )?;
                        boundary_args.push(value);
                    }
                }
                MirBoundaryParam::AggregateUnsupported { size } => {
                    return Err(Self::block_error(
                        self.function,
                        block_id,
                        format!(
                            "unsupported x64 SysV aggregate parameter classification for {}-byte aggregate",
                            size
                        ),
                    ));
                }
            }
        }

        if internal_cursor != fixed_internal_arg_count {
            return Err(Self::block_error(
                self.function,
                block_id,
                format!(
                    "boundary call_indirect parameter mismatch: consumed {}, expected {}",
                    internal_cursor, fixed_internal_arg_count
                ),
            ));
        }

        if let Some(fixed) = fixed_arg_count {
            for arg in args.iter().skip(*fixed) {
                let arg_ty = self.infer_variadic_arg_type(arg, block_id)?;
                call_sig.params.push(AbiParam::new(arg_ty));
                boundary_args.push(self.lower_operand_value(builder, arg, arg_ty)?);
            }
        }

        let callee_ptr = self.lower_operand_value(builder, callee_ptr, self.pointer_type)?;
        let sig_ref = builder.import_signature(call_sig);
        let call = builder
            .ins()
            .call_indirect(sig_ref, callee_ptr, &boundary_args);

        match &boundary_sig.return_type {
            MirBoundaryReturn::Void | MirBoundaryReturn::AggregateMemory { .. } => Ok(()),
            MirBoundaryReturn::Scalar(_) => self.assign_call_result(builder, dst, call),
            MirBoundaryReturn::AggregateScalarized { parts, size } => {
                let sret = internal_sret.ok_or_else(|| {
                    Self::block_error(
                        self.function,
                        block_id,
                        "scalarized aggregate return requires internal sret argument".to_string(),
                    )
                })?;
                let results = builder.inst_results(call).to_vec();
                if results.len() != parts.len() {
                    return Err(Self::block_error(
                        self.function,
                        block_id,
                        format!(
                            "scalarized aggregate return part mismatch: got {}, expected {}",
                            results.len(),
                            parts.len()
                        ),
                    ));
                }
                for (part_index, (&part_ty, value)) in
                    parts.iter().zip(results.iter().copied()).enumerate()
                {
                    let part_offset = (part_index as u32) * 8;
                    let copy_size = lane_copy_size(*size, part_index);
                    store_boundary_part_bytes(
                        &self.type_lowering,
                        builder,
                        module,
                        pointer_type,
                        sret,
                        i64::from(part_offset),
                        value,
                        part_ty,
                        copy_size,
                    )?;
                }
                Ok(())
            }
            MirBoundaryReturn::AggregateUnsupported { size } => Err(Self::block_error(
                self.function,
                block_id,
                format!(
                    "unsupported x64 SysV aggregate return classification for {}-byte aggregate",
                    size
                ),
            )),
        }
    }

    fn lower_switch_terminator(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        block_id: BlockId,
        discr: VReg,
        cases: &[SwitchCase],
        default: BlockId,
    ) -> Result<(), BackendError> {
        let discr_ty = *self.vreg_types.get(&discr).ok_or_else(|| {
            Self::block_error(
                self.function,
                block_id,
                format!("switch discriminator v{} has no declared type", discr.0),
            )
        })?;
        if !discr_ty.is_int() {
            return Err(Self::block_error(
                self.function,
                block_id,
                format!("switch discriminator v{} is not an integer", discr.0),
            ));
        }

        let default_block = self.block(default)?;
        if cases.is_empty() {
            builder.ins().jump(default_block, &[]);
            return Ok(());
        }

        let discr_value = self.use_vreg(builder, discr)?;
        let mut switch = ClifSwitch::new();
        let mut seen_cases: HashMap<u128, BlockId> = HashMap::with_capacity(cases.len());
        for case in cases {
            let normalized = normalize_switch_case_value(case.value, discr_ty);
            if let Some(prev_target) = seen_cases.insert(normalized, case.target) {
                return Err(Self::block_error(
                    self.function,
                    block_id,
                    format!(
                        "duplicate switch case value {} after {}-bit normalization (targets bb{} and bb{})",
                        case.value,
                        discr_ty.bits(),
                        prev_target.0,
                        case.target.0
                    ),
                ));
            }
            let target = self.block(case.target)?;
            switch.set_entry(normalized, target);
        }

        // Cranelift Switch selects between jump tables and compare/branch trees
        // according to case density.
        switch.emit(builder, discr_value, default_block);
        Ok(())
    }

    fn import_direct_callee_with_signature(
        &self,
        builder: &mut FunctionBuilder<'_>,
        module: &ObjectModule,
        callee_id: FuncId,
        signature: ir::Signature,
    ) -> ir::FuncRef {
        let decl = module.declarations().get_function_decl(callee_id);
        let sig_ref = builder.import_signature(signature);
        let user_name_ref = builder
            .func
            .declare_imported_user_function(ir::UserExternalName {
                namespace: 0,
                index: callee_id.as_u32(),
            });
        builder.import_function(ir::ExtFuncData {
            name: ir::ExternalName::user(user_name_ref),
            signature: sig_ref,
            colocated: decl.linkage.is_final(),
            patchable: false,
        })
    }

    fn infer_variadic_arg_type(
        &self,
        operand: &Operand,
        block_id: BlockId,
    ) -> Result<ir::Type, BackendError> {
        match operand {
            Operand::VReg(reg) => self.vreg_types.get(reg).copied().ok_or_else(|| {
                Self::block_error(
                    self.function,
                    block_id,
                    format!("variadic argument v{} has no declared type", reg.0),
                )
            }),
            Operand::StackSlot(_) => Ok(self.pointer_type),
            Operand::Const(MirConst::FloatConst(_)) => Ok(types::F64),
            Operand::Const(MirConst::IntConst(_) | MirConst::ZeroConst) => Ok(types::I32),
        }
    }

    fn emit_frem_call(
        &self,
        builder: &mut FunctionBuilder<'_>,
        module: &mut ObjectModule,
        block_id: BlockId,
        ty: ir::Type,
        lhs: ir::Value,
        rhs: ir::Value,
    ) -> Result<ir::Value, BackendError> {
        let (name, ty) = if ty == types::F32 {
            ("fmodf", types::F32)
        } else if ty == types::F64 {
            ("fmod", types::F64)
        } else {
            return Err(Self::block_error(
                self.function,
                block_id,
                format!("frem expects floating-point type, got {}", ty),
            ));
        };

        let mut sig = module.make_signature();
        sig.params.push(AbiParam::new(ty));
        sig.params.push(AbiParam::new(ty));
        sig.returns.push(AbiParam::new(ty));
        let func_id = module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|err| {
                Self::block_error(self.function, block_id, format!("declare {name}: {err}"))
            })?;
        let callee = module.declare_func_in_func(func_id, builder.func);
        let call = builder.ins().call(callee, &[lhs, rhs]);
        Ok(builder.inst_results(call)[0])
    }

    fn def_vreg(
        &self,
        builder: &mut FunctionBuilder<'_>,
        reg: VReg,
        value: ir::Value,
    ) -> Result<(), BackendError> {
        let var = *self.vreg_vars.get(&reg).ok_or_else(|| {
            Self::function_error(
                self.function,
                format!("vreg v{} has no declared Variable mapping", reg.0),
            )
        })?;
        builder.try_def_var(var, value).map_err(|err| {
            Self::function_error(
                self.function,
                format!("failed to define v{}: {err:?}", reg.0),
            )
        })
    }

    fn use_vreg(
        &self,
        builder: &mut FunctionBuilder<'_>,
        reg: VReg,
    ) -> Result<ir::Value, BackendError> {
        let var = *self.vreg_vars.get(&reg).ok_or_else(|| {
            Self::function_error(
                self.function,
                format!("vreg v{} has no declared Variable mapping", reg.0),
            )
        })?;
        builder.try_use_var(var).map_err(|err| {
            Self::function_error(self.function, format!("failed to use v{}: {err:?}", reg.0))
        })
    }

    fn block(&self, block_id: BlockId) -> Result<ir::Block, BackendError> {
        self.block_map.get(&block_id).copied().ok_or_else(|| {
            Self::function_error(
                self.function,
                format!("missing CLIF block mapping for bb{}", block_id.0),
            )
        })
    }

    fn stack_slot(&self, slot_id: SlotId) -> Result<ir::StackSlot, BackendError> {
        self.stack_slot_map.get(&slot_id).copied().ok_or_else(|| {
            Self::function_error(
                self.function,
                format!("missing CLIF stack slot mapping for ${}", slot_id.0),
            )
        })
    }

    fn function_error(function: &MirFunction, message: impl Into<String>) -> BackendError {
        BackendError::UnsupportedFunctionLowering {
            function: function.name.clone(),
            message: message.into(),
        }
    }

    fn block_error(
        function: &MirFunction,
        block_id: BlockId,
        message: impl Into<String>,
    ) -> BackendError {
        BackendError::UnsupportedFunctionLowering {
            function: function.name.clone(),
            message: format!("bb{}: {}", block_id.0, message.into()),
        }
    }
}

impl FunctionLoweringContext {
    pub(crate) fn new(module: &ObjectModule) -> Self {
        Self {
            clif_context: module.make_context(),
            func_builder_context: FunctionBuilderContext::new(),
            type_lowering: MirTypeLowering,
        }
    }

    pub(crate) fn type_lowering(&self) -> &MirTypeLowering {
        &self.type_lowering
    }

    pub(crate) fn define_function(
        &mut self,
        module: &mut ObjectModule,
        symbols: &ModuleSymbols,
        func_id: FuncId,
        function: &MirFunction,
    ) -> Result<(), BackendError> {
        let prepared = self.prepare_function_context(module, function)?;
        {
            let mut builder =
                FunctionBuilder::new(&mut self.clif_context.func, &mut self.func_builder_context);
            let pointer_type = module.target_config().pointer_type();
            let mut lowering = BodyLoweringState::new(
                &mut builder,
                function,
                prepared,
                self.type_lowering,
                pointer_type,
            )?;
            lowering.lower_blocks(&mut builder, module, symbols)?;
            builder.seal_all_blocks();
            builder.finalize();
        }

        module.define_function(func_id, &mut self.clif_context)?;
        Ok(())
    }

    fn prepare_function_context(
        &mut self,
        module: &mut ObjectModule,
        function: &MirFunction,
    ) -> Result<PreparedFunctionContext, BackendError> {
        module.clear_context(&mut self.clif_context);
        self.clif_context.func.signature = self.type_lowering.lower_signature(
            module.isa().default_call_conv(),
            &function.params,
            function.return_type,
            function.variadic,
            &function.name,
        )?;
        let stack_slots = self.declare_stack_slots(function)?;
        Ok(PreparedFunctionContext { stack_slots })
    }

    pub(crate) fn define_synthetic_empty_function(
        &mut self,
        module: &mut ObjectModule,
        func_id: FuncId,
        function_name: &str,
    ) -> Result<(), BackendError> {
        module.clear_context(&mut self.clif_context);
        self.clif_context.func.signature = self.type_lowering.lower_signature(
            module.isa().default_call_conv(),
            &[],
            MirType::Void,
            false,
            function_name,
        )?;

        {
            let mut builder =
                FunctionBuilder::new(&mut self.clif_context.func, &mut self.func_builder_context);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);
            builder.ins().return_(&[]);
            builder.finalize();
        }

        module.define_function(func_id, &mut self.clif_context)?;
        Ok(())
    }

    pub(crate) fn define_function_wrapper(
        &mut self,
        module: &mut ObjectModule,
        symbols: &ModuleSymbols,
        wrapper_id: FuncId,
        function: &MirFunction,
    ) -> Result<(), BackendError> {
        module.clear_context(&mut self.clif_context);
        self.clif_context.func.signature = self.type_lowering.lower_boundary_signature(
            module.isa().default_call_conv(),
            &function.boundary_sig,
            &function.name,
        )?;

        {
            let mut builder =
                FunctionBuilder::new(&mut self.clif_context.func, &mut self.func_builder_context);
            let pointer_type = module.target_config().pointer_type();
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let params = builder.block_params(entry).to_vec();
            let mut boundary_cursor = 0usize;
            let mut internal_args = Vec::new();
            let internal_has_sret = function
                .params
                .first()
                .is_some_and(|param| param.purpose == MirAbiParamPurpose::StructReturn);
            let scalarized_return_buffer = match &function.boundary_sig.return_type {
                MirBoundaryReturn::AggregateMemory { .. } => {
                    if !internal_has_sret {
                        return Err(BackendError::UnsupportedFunctionLowering {
                            function: function.name.clone(),
                            message: "boundary sret return does not match internal signature"
                                .to_string(),
                        });
                    }
                    let sret = *params.get(boundary_cursor).ok_or_else(|| {
                        BackendError::UnsupportedFunctionLowering {
                            function: function.name.clone(),
                            message: "missing boundary sret parameter".to_string(),
                        }
                    })?;
                    boundary_cursor += 1;
                    internal_args.push(sret);
                    None
                }
                MirBoundaryReturn::AggregateScalarized { size, .. } => {
                    if !internal_has_sret {
                        return Err(BackendError::UnsupportedFunctionLowering {
                            function: function.name.clone(),
                            message: "scalarized aggregate return requires internal sret parameter"
                                .to_string(),
                        });
                    }
                    let slot = create_wrapper_stack_slot(&mut builder, *size, 8)?;
                    let addr = builder.ins().stack_addr(pointer_type, slot, 0);
                    internal_args.push(addr);
                    Some(addr)
                }
                MirBoundaryReturn::AggregateUnsupported { size } => {
                    return Err(BackendError::UnsupportedFunctionLowering {
                        function: function.name.clone(),
                        message: format!(
                            "unsupported x64 SysV aggregate return classification for {}-byte aggregate",
                            size
                        ),
                    });
                }
                MirBoundaryReturn::Void | MirBoundaryReturn::Scalar(_) => None,
            };

            for (param_index, param) in function.boundary_sig.params.iter().enumerate() {
                match param {
                    MirBoundaryParam::Scalar(_) | MirBoundaryParam::AggregateMemory { .. } => {
                        let value = *params.get(boundary_cursor).ok_or_else(|| {
                            BackendError::UnsupportedFunctionLowering {
                                function: function.name.clone(),
                                message: format!(
                                    "missing boundary parameter {} for wrapper",
                                    param_index
                                ),
                            }
                        })?;
                        boundary_cursor += 1;
                        internal_args.push(value);
                    }
                    MirBoundaryParam::AggregateScalarized { parts, size } => {
                        let slot = create_wrapper_stack_slot(&mut builder, *size, 8)?;
                        let base_ptr = builder.ins().stack_addr(pointer_type, slot, 0);
                        for (part_index, part_ty) in parts.iter().copied().enumerate() {
                            let value = *params.get(boundary_cursor).ok_or_else(|| {
                                BackendError::UnsupportedFunctionLowering {
                                    function: function.name.clone(),
                                    message: format!(
                                        "missing scalarized boundary parameter part {} for parameter {}",
                                        part_index, param_index
                                    ),
                                }
                            })?;
                            boundary_cursor += 1;
                            let part_offset = (part_index as u32) * 8;
                            let copy_size = lane_copy_size(*size, part_index);
                            store_boundary_part_bytes(
                                &self.type_lowering,
                                &mut builder,
                                module,
                                pointer_type,
                                base_ptr,
                                i64::from(part_offset),
                                value,
                                part_ty,
                                copy_size,
                            )?;
                        }
                        internal_args.push(base_ptr);
                    }
                    MirBoundaryParam::AggregateUnsupported { size } => {
                        return Err(BackendError::UnsupportedFunctionLowering {
                            function: function.name.clone(),
                            message: format!(
                                "unsupported x64 SysV aggregate parameter classification for {}-byte aggregate",
                                size
                            ),
                        });
                    }
                }
            }

            let body_id = symbols
                .function_id(&function.name)
                .ok_or_else(|| BackendError::MissingFunctionSymbol(function.name.clone()))?;
            let callee = module.declare_func_in_func(body_id, builder.func);
            let call = builder.ins().call(callee, &internal_args);

            match &function.boundary_sig.return_type {
                MirBoundaryReturn::Void | MirBoundaryReturn::AggregateMemory { .. } => {
                    builder.ins().return_(&[]);
                }
                MirBoundaryReturn::Scalar(_) => {
                    let results = builder.inst_results(call).to_vec();
                    builder.ins().return_(&results);
                }
                MirBoundaryReturn::AggregateScalarized { parts, size } => {
                    let return_buffer = scalarized_return_buffer.ok_or_else(|| {
                        BackendError::UnsupportedFunctionLowering {
                            function: function.name.clone(),
                            message: "missing scalarized return buffer for wrapper".to_string(),
                        }
                    })?;
                    let mut returns = Vec::with_capacity(parts.len());
                    for (part_index, part_ty) in parts.iter().copied().enumerate() {
                        let part_offset = (part_index as u32) * 8;
                        let copy_size = lane_copy_size(*size, part_index);
                        let value = load_boundary_part_bytes(
                            &self.type_lowering,
                            &mut builder,
                            module,
                            pointer_type,
                            return_buffer,
                            i64::from(part_offset),
                            part_ty,
                            copy_size,
                        )?;
                        returns.push(value);
                    }
                    builder.ins().return_(&returns);
                }
                MirBoundaryReturn::AggregateUnsupported { .. } => unreachable!(),
            }

            builder.finalize();
        }

        module.define_function(wrapper_id, &mut self.clif_context)?;
        Ok(())
    }

    pub(crate) fn define_import_wrapper(
        &mut self,
        module: &mut ObjectModule,
        wrapper_id: FuncId,
        import_id: FuncId,
        function: &MirExternFunction,
    ) -> Result<(), BackendError> {
        module.clear_context(&mut self.clif_context);
        self.clif_context.func.signature = self.type_lowering.lower_signature(
            module.isa().default_call_conv(),
            &function.sig.params,
            function.sig.return_type,
            function.sig.variadic,
            &function.name,
        )?;

        {
            let mut builder =
                FunctionBuilder::new(&mut self.clif_context.func, &mut self.func_builder_context);
            let pointer_type = module.target_config().pointer_type();
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let params = builder.block_params(entry).to_vec();
            let mut internal_cursor = 0usize;
            let internal_sret = function
                .sig
                .params
                .first()
                .is_some_and(|param| param.purpose == MirAbiParamPurpose::StructReturn)
                .then(|| {
                    let value = params[internal_cursor];
                    internal_cursor += 1;
                    value
                });

            let mut external_args = Vec::new();
            if matches!(
                function.boundary_sig.return_type,
                MirBoundaryReturn::AggregateMemory { .. }
            ) {
                let sret =
                    internal_sret.ok_or_else(|| BackendError::UnsupportedFunctionLowering {
                        function: function.name.clone(),
                        message: "boundary sret return does not match internal wrapper signature"
                            .to_string(),
                    })?;
                external_args.push(sret);
            }

            for (param_index, param) in function.boundary_sig.params.iter().enumerate() {
                match param {
                    MirBoundaryParam::Scalar(_) | MirBoundaryParam::AggregateMemory { .. } => {
                        let value = *params.get(internal_cursor).ok_or_else(|| {
                            BackendError::UnsupportedFunctionLowering {
                                function: function.name.clone(),
                                message: format!(
                                    "missing internal parameter {} for import wrapper",
                                    param_index
                                ),
                            }
                        })?;
                        internal_cursor += 1;
                        external_args.push(value);
                    }
                    MirBoundaryParam::AggregateScalarized { parts, size } => {
                        let aggregate_ptr = *params.get(internal_cursor).ok_or_else(|| {
                            BackendError::UnsupportedFunctionLowering {
                                function: function.name.clone(),
                                message: format!(
                                    "missing aggregate pointer parameter {} for import wrapper",
                                    param_index
                                ),
                            }
                        })?;
                        internal_cursor += 1;
                        for (part_index, part_ty) in parts.iter().copied().enumerate() {
                            let part_offset = (part_index as u32) * 8;
                            let copy_size = lane_copy_size(*size, part_index);
                            let value = load_boundary_part_bytes(
                                &self.type_lowering,
                                &mut builder,
                                module,
                                pointer_type,
                                aggregate_ptr,
                                i64::from(part_offset),
                                part_ty,
                                copy_size,
                            )?;
                            external_args.push(value);
                        }
                    }
                    MirBoundaryParam::AggregateUnsupported { size } => {
                        return Err(BackendError::UnsupportedFunctionLowering {
                            function: function.name.clone(),
                            message: format!(
                                "unsupported x64 SysV aggregate parameter classification for {}-byte aggregate",
                                size
                            ),
                        });
                    }
                }
            }

            let callee = module.declare_func_in_func(import_id, builder.func);
            let call = builder.ins().call(callee, &external_args);

            match &function.boundary_sig.return_type {
                MirBoundaryReturn::Void | MirBoundaryReturn::AggregateMemory { .. } => {
                    builder.ins().return_(&[]);
                }
                MirBoundaryReturn::Scalar(_) => {
                    let results = builder.inst_results(call).to_vec();
                    builder.ins().return_(&results);
                }
                MirBoundaryReturn::AggregateScalarized { parts, size } => {
                    let sret =
                        internal_sret.ok_or_else(|| BackendError::UnsupportedFunctionLowering {
                            function: function.name.clone(),
                            message: "scalarized aggregate return requires internal sret parameter"
                                .to_string(),
                        })?;
                    let results = builder.inst_results(call).to_vec();
                    if results.len() != parts.len() {
                        return Err(BackendError::UnsupportedFunctionLowering {
                            function: function.name.clone(),
                            message: format!(
                                "scalarized aggregate return part mismatch: got {}, expected {}",
                                results.len(),
                                parts.len()
                            ),
                        });
                    }
                    for (part_index, (&part_ty, value)) in
                        parts.iter().zip(results.iter().copied()).enumerate()
                    {
                        let part_offset = (part_index as u32) * 8;
                        let copy_size = lane_copy_size(*size, part_index);
                        store_boundary_part_bytes(
                            &self.type_lowering,
                            &mut builder,
                            module,
                            pointer_type,
                            sret,
                            i64::from(part_offset),
                            value,
                            part_ty,
                            copy_size,
                        )?;
                    }
                    builder.ins().return_(&[]);
                }
                MirBoundaryReturn::AggregateUnsupported { size } => {
                    return Err(BackendError::UnsupportedFunctionLowering {
                        function: function.name.clone(),
                        message: format!(
                            "unsupported x64 SysV aggregate return classification for {}-byte aggregate",
                            size
                        ),
                    });
                }
            }

            builder.finalize();
        }

        module.define_function(wrapper_id, &mut self.clif_context)?;
        Ok(())
    }

    fn declare_stack_slots(
        &mut self,
        function: &MirFunction,
    ) -> Result<HashMap<SlotId, ir::StackSlot>, BackendError> {
        let mut lowered_slots = HashMap::with_capacity(function.stack_slots.len());
        for slot in &function.stack_slots {
            let data = stack_slot_data_for_mir_slot(&function.name, slot)?;
            let clif_slot = self.clif_context.func.create_sized_stack_slot(data);
            if lowered_slots.insert(slot.id, clif_slot).is_some() {
                return Err(BackendError::InvalidStackSlot {
                    function: function.name.clone(),
                    slot: slot.id.0,
                    message: "duplicate MIR slot id".to_string(),
                });
            }
        }
        Ok(lowered_slots)
    }
}

fn create_wrapper_stack_slot(
    builder: &mut FunctionBuilder<'_>,
    size: u32,
    alignment: u32,
) -> Result<ir::StackSlot, BackendError> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(BackendError::InvalidStackSlot {
            function: "<wrapper>".to_string(),
            slot: 0,
            message: format!("wrapper slot alignment {} is invalid", alignment),
        });
    }
    let align_shift =
        u8::try_from(alignment.trailing_zeros()).map_err(|_| BackendError::InvalidStackSlot {
            function: "<wrapper>".to_string(),
            slot: 0,
            message: format!(
                "wrapper slot alignment {} exceeds supported range",
                alignment
            ),
        })?;
    Ok(builder.func.create_sized_stack_slot(ir::StackSlotData::new(
        ir::StackSlotKind::ExplicitSlot,
        size.max(1),
        align_shift,
    )))
}

fn lane_copy_size(total_size: u32, part_index: usize) -> u32 {
    total_size
        .saturating_sub((part_index as u32) * 8)
        .min(8)
        .max(1)
}

fn ptr_add_const(
    builder: &mut FunctionBuilder<'_>,
    pointer_type: ir::Type,
    base: ir::Value,
    offset: i64,
) -> ir::Value {
    if offset == 0 {
        return base;
    }
    let offset_value = builder.ins().iconst(pointer_type, offset);
    builder.ins().iadd(base, offset_value)
}

fn store_boundary_part_bytes(
    type_lowering: &MirTypeLowering,
    builder: &mut FunctionBuilder<'_>,
    module: &mut ObjectModule,
    pointer_type: ir::Type,
    dst_base: ir::Value,
    dst_offset: i64,
    value: ir::Value,
    value_ty: MirType,
    copy_size: u32,
) -> Result<(), BackendError> {
    let temp_slot = create_wrapper_stack_slot(builder, 8, 8)?;
    let temp_ptr = builder.ins().stack_addr(pointer_type, temp_slot, 0);
    let _ = type_lowering.lower_value_type(value_ty, "boundary aggregate part store")?;
    builder.ins().store(mem_flags(false), value, temp_ptr, 0);
    let dst_ptr = ptr_add_const(builder, pointer_type, dst_base, dst_offset);
    let size = builder.ins().iconst(pointer_type, i64::from(copy_size));
    builder.call_memcpy(module.target_config(), dst_ptr, temp_ptr, size);
    Ok(())
}

fn load_boundary_part_bytes(
    type_lowering: &MirTypeLowering,
    builder: &mut FunctionBuilder<'_>,
    module: &mut ObjectModule,
    pointer_type: ir::Type,
    src_base: ir::Value,
    src_offset: i64,
    value_ty: MirType,
    copy_size: u32,
) -> Result<ir::Value, BackendError> {
    let temp_slot = create_wrapper_stack_slot(builder, 8, 8)?;
    let temp_ptr = builder.ins().stack_addr(pointer_type, temp_slot, 0);
    let zero = builder.ins().iconst(types::I64, 0);
    builder.ins().store(mem_flags(false), zero, temp_ptr, 0);
    let src_ptr = ptr_add_const(builder, pointer_type, src_base, src_offset);
    let size = builder.ins().iconst(pointer_type, i64::from(copy_size));
    builder.call_memcpy(module.target_config(), temp_ptr, src_ptr, size);
    let clif_ty = type_lowering.lower_value_type(value_ty, "boundary aggregate part load")?;
    Ok(builder.ins().load(clif_ty, mem_flags(false), temp_ptr, 0))
}

fn collect_vreg_types(function: &MirFunction) -> Result<HashMap<VReg, MirType>, BackendError> {
    let mut types = HashMap::new();
    for (idx, param) in function.params.iter().copied().enumerate() {
        record_vreg_type(function, &mut types, VReg(idx as u32), param.ty)?;
    }

    for block in &function.blocks {
        for instruction in &block.instructions {
            match instruction {
                Instruction::Load { dst, .. }
                | Instruction::PtrLoad { dst, .. }
                | Instruction::Binary { dst, .. }
                | Instruction::Unary { dst, .. }
                | Instruction::Cmp { dst, .. }
                | Instruction::Cast { dst, .. }
                | Instruction::SlotAddr { dst, .. }
                | Instruction::GlobalAddr { dst, .. }
                | Instruction::PtrAdd { dst, .. }
                | Instruction::Copy { dst, .. } => {
                    record_vreg_type(function, &mut types, dst.reg, dst.ty)?;
                }
                Instruction::Call { dst, .. } | Instruction::CallIndirect { dst, .. } => {
                    if let Some(dst) = dst {
                        record_vreg_type(function, &mut types, dst.reg, dst.ty)?;
                    }
                }
                Instruction::Store { .. }
                | Instruction::PtrStore { .. }
                | Instruction::Memcpy { .. }
                | Instruction::Memset { .. } => {}
            }
        }
    }

    Ok(types)
}

fn record_vreg_type(
    function: &MirFunction,
    map: &mut HashMap<VReg, MirType>,
    reg: VReg,
    ty: MirType,
) -> Result<(), BackendError> {
    if let Some(existing) = map.get(&reg)
        && *existing != ty
    {
        return Err(BackendError::UnsupportedFunctionLowering {
            function: function.name.clone(),
            message: format!(
                "vreg v{} has conflicting types: {} vs {}",
                reg.0, existing, ty
            ),
        });
    }
    map.insert(reg, ty);
    Ok(())
}

fn stack_slot_data_for_mir_slot(
    function_name: &str,
    slot: &MirStackSlot,
) -> Result<ir::StackSlotData, BackendError> {
    if slot.size == 0 {
        return Err(BackendError::InvalidStackSlot {
            function: function_name.to_string(),
            slot: slot.id.0,
            message: "slot size must be greater than zero".to_string(),
        });
    }
    if slot.alignment == 0 {
        return Err(BackendError::InvalidStackSlot {
            function: function_name.to_string(),
            slot: slot.id.0,
            message: "slot alignment must be greater than zero".to_string(),
        });
    }
    if !slot.alignment.is_power_of_two() {
        return Err(BackendError::InvalidStackSlot {
            function: function_name.to_string(),
            slot: slot.id.0,
            message: format!("slot alignment {} is not a power of two", slot.alignment),
        });
    }

    let align_shift = u8::try_from(slot.alignment.trailing_zeros()).map_err(|_| {
        BackendError::InvalidStackSlot {
            function: function_name.to_string(),
            slot: slot.id.0,
            message: format!("slot alignment {} exceeds supported range", slot.alignment),
        }
    })?;

    Ok(ir::StackSlotData::new(
        ir::StackSlotKind::ExplicitSlot,
        slot.size,
        align_shift,
    ))
}

fn int_cc(kind: CmpKind, signed: bool) -> IntCC {
    match (kind, signed) {
        (CmpKind::Eq, _) => IntCC::Equal,
        (CmpKind::Ne, _) => IntCC::NotEqual,
        (CmpKind::Lt, true) => IntCC::SignedLessThan,
        (CmpKind::Le, true) => IntCC::SignedLessThanOrEqual,
        (CmpKind::Gt, true) => IntCC::SignedGreaterThan,
        (CmpKind::Ge, true) => IntCC::SignedGreaterThanOrEqual,
        (CmpKind::Lt, false) => IntCC::UnsignedLessThan,
        (CmpKind::Le, false) => IntCC::UnsignedLessThanOrEqual,
        (CmpKind::Gt, false) => IntCC::UnsignedGreaterThan,
        (CmpKind::Ge, false) => IntCC::UnsignedGreaterThanOrEqual,
    }
}

fn float_cc(kind: CmpKind) -> FloatCC {
    match kind {
        CmpKind::Eq => FloatCC::Equal,
        CmpKind::Ne => FloatCC::NotEqual,
        CmpKind::Lt => FloatCC::LessThan,
        CmpKind::Le => FloatCC::LessThanOrEqual,
        CmpKind::Gt => FloatCC::GreaterThan,
        CmpKind::Ge => FloatCC::GreaterThanOrEqual,
    }
}

fn normalize_switch_case_value(value: i64, discr_ty: ir::Type) -> u128 {
    let bits = discr_ty.bits();
    debug_assert!(bits > 0 && bits <= 128);
    let raw = (value as i128) as u128;
    if bits == 128 {
        raw
    } else {
        let mask = (1u128 << bits) - 1;
        raw & mask
    }
}

fn coerce_integer_value(
    builder: &mut FunctionBuilder<'_>,
    value: ir::Value,
    from_ty: ir::Type,
    to_ty: ir::Type,
) -> ir::Value {
    if from_ty == to_ty {
        return value;
    }

    let from_bits = from_ty.bits();
    let to_bits = to_ty.bits();
    if to_bits < from_bits {
        builder.ins().ireduce(to_ty, value)
    } else {
        builder.ins().sextend(to_ty, value)
    }
}

fn lower_const_value(
    builder: &mut FunctionBuilder<'_>,
    constant: MirConst,
    ty: ir::Type,
) -> Result<ir::Value, &'static str> {
    match constant {
        MirConst::IntConst(value) => {
            if ty.is_int() {
                Ok(builder.ins().iconst(ty, value))
            } else {
                Err("integer constant used in non-integer context")
            }
        }
        MirConst::FloatConst(value) => {
            if ty == types::F32 {
                Ok(builder.ins().f32const(Ieee32::with_float(value as f32)))
            } else if ty == types::F64 {
                Ok(builder.ins().f64const(Ieee64::with_float(value)))
            } else {
                Err("floating constant used in non-floating context")
            }
        }
        MirConst::ZeroConst => {
            if ty.is_int() {
                Ok(builder.ins().iconst(ty, 0))
            } else if ty == types::F32 {
                Ok(builder.ins().f32const(Ieee32::with_bits(0)))
            } else if ty == types::F64 {
                Ok(builder.ins().f64const(Ieee64::with_bits(0)))
            } else {
                Err("zero constant used in unsupported context")
            }
        }
    }
}

fn offset_to_i32(offset: i64) -> Result<i32, BackendError> {
    i32::try_from(offset).map_err(|_| BackendError::UnsupportedFunctionLowering {
        function: "<offset-conversion>".to_string(),
        message: format!("memory offset {} is out of i32 range", offset),
    })
}

fn mem_flags(_volatile: bool) -> MemFlags {
    // Cranelift does not expose a dedicated "volatile" bit in MemFlags.
    // Keep conservative default flags (no `can_move` / no readonly hints).
    MemFlags::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{isa, object};

    #[test]
    fn lowers_all_supported_value_types() {
        let type_lowering = MirTypeLowering;
        assert_eq!(
            type_lowering
                .lower_value_type(MirType::I8, "instruction operand")
                .expect("i8 should map"),
            types::I8
        );
        assert_eq!(
            type_lowering
                .lower_value_type(MirType::I16, "instruction operand")
                .expect("i16 should map"),
            types::I16
        );
        assert_eq!(
            type_lowering
                .lower_value_type(MirType::I32, "instruction operand")
                .expect("i32 should map"),
            types::I32
        );
        assert_eq!(
            type_lowering
                .lower_value_type(MirType::I64, "instruction operand")
                .expect("i64 should map"),
            types::I64
        );
        assert_eq!(
            type_lowering
                .lower_value_type(MirType::F32, "instruction operand")
                .expect("f32 should map"),
            types::F32
        );
        assert_eq!(
            type_lowering
                .lower_value_type(MirType::F64, "instruction operand")
                .expect("f64 should map"),
            types::F64
        );
        assert_eq!(
            type_lowering
                .lower_value_type(MirType::Ptr, "instruction operand")
                .expect("ptr should map to target pointer type"),
            types::I64
        );
    }

    #[test]
    fn rejects_void_for_value_positions() {
        let err = MirTypeLowering
            .lower_value_type(MirType::Void, "instruction result")
            .expect_err("void value type should be rejected outside function return type");

        assert!(matches!(
            err,
            BackendError::UnsupportedMirType {
                ty: MirType::Void,
                context: "instruction result"
            }
        ));
    }

    #[test]
    fn allows_void_only_as_function_return() {
        let signature = MirTypeLowering
            .lower_signature(CallConv::SystemV, &[], MirType::Void, false, "f")
            .expect("void return should be accepted");
        assert!(signature.returns.is_empty());

        let err = MirTypeLowering
            .lower_signature(
                CallConv::SystemV,
                &[MirAbiParam::new(MirType::Void)],
                MirType::I32,
                false,
                "f",
            )
            .expect_err("void parameter should be rejected");
        assert!(matches!(
            err,
            BackendError::UnsupportedMirType {
                ty: MirType::Void,
                context: "function parameter type"
            }
        ));
    }

    #[test]
    fn allows_variadic_signatures_with_fixed_params() {
        let signature = MirTypeLowering.lower_signature(
            CallConv::SystemV,
            &[MirAbiParam::new(MirType::I32)],
            MirType::I32,
            true,
            "printf",
        );
        let signature = signature.expect("variadic signatures should lower fixed params");
        assert_eq!(signature.params.len(), 1);
        assert_eq!(signature.params[0].value_type, types::I32);
        assert_eq!(signature.returns.len(), 1);
        assert_eq!(signature.returns[0].value_type, types::I32);
    }

    #[test]
    fn maps_stack_slot_size_and_alignment_to_clif_data() {
        let slot = MirStackSlot {
            id: SlotId(7),
            size: 32,
            alignment: 16,
            address_taken: true,
        };
        let data = stack_slot_data_for_mir_slot("f", &slot).expect("valid slot should map");
        assert_eq!(data.kind, ir::StackSlotKind::ExplicitSlot);
        assert_eq!(data.size, 32);
        assert_eq!(data.align_shift, 4);
    }

    #[test]
    fn rejects_invalid_stack_slot_alignment() {
        let slot = MirStackSlot {
            id: SlotId(1),
            size: 8,
            alignment: 3,
            address_taken: false,
        };
        let err = stack_slot_data_for_mir_slot("f", &slot)
            .expect_err("non power-of-two alignment should be rejected");
        assert!(matches!(
            err,
            BackendError::InvalidStackSlot {
                ref function,
                slot: 1,
                ..
            } if function == "f"
        ));
    }

    #[test]
    fn prepare_function_context_declares_stack_slots() {
        let isa = isa::build_default_isa().expect("target isa");
        let mut module = object::new_object_module(isa).expect("object module");
        let mut lowering = FunctionLoweringContext::new(&module);
        let function = MirFunction {
            name: "f".to_string(),
            linkage: crate::mir::ir::MirLinkage::External,
            params: vec![MirAbiParam::new(MirType::I32)],
            return_type: MirType::Void,
            boundary_sig: MirBoundarySignature::from_internal(
                &[MirAbiParam::new(MirType::I32)],
                MirType::Void,
                false,
            ),
            variadic: false,
            stack_slots: vec![
                MirStackSlot {
                    id: SlotId(1),
                    size: 8,
                    alignment: 8,
                    address_taken: false,
                },
                MirStackSlot {
                    id: SlotId(4),
                    size: 24,
                    alignment: 16,
                    address_taken: true,
                },
            ],
            blocks: vec![BasicBlock {
                id: BlockId(0),
                instructions: Vec::new(),
                terminator: Terminator::Ret(None),
            }],
            virtual_reg_counter: 0,
        };

        let prepared = lowering
            .prepare_function_context(&mut module, &function)
            .expect("function context should be prepared");
        assert_eq!(prepared.stack_slots.len(), 2);

        let slot_1 = prepared
            .stack_slots
            .get(&SlotId(1))
            .expect("slot id 1 should be declared");
        let slot_4 = prepared
            .stack_slots
            .get(&SlotId(4))
            .expect("slot id 4 should be declared");
        assert_eq!(
            lowering.clif_context.func.sized_stack_slots[*slot_1].size,
            8
        );
        assert_eq!(
            lowering.clif_context.func.sized_stack_slots[*slot_1].align_shift,
            3
        );
        assert_eq!(
            lowering.clif_context.func.sized_stack_slots[*slot_4].size,
            24
        );
        assert_eq!(
            lowering.clif_context.func.sized_stack_slots[*slot_4].align_shift,
            4
        );
    }
}
