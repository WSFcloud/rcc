use std::collections::{HashMap, HashSet};

use crate::frontend::sema::SemaResult;
use crate::frontend::sema::const_eval::{self, ConstEvalEnv, ConstExprContext};
use crate::frontend::sema::symbols::{
    DefinitionStatus, Linkage, ObjectStorageClass, Symbol, SymbolId, SymbolKind,
};
use crate::frontend::sema::typed_ast::{
    AssignOp as TypedAssignOp, BinaryOp as TypedBinaryOp, CaseValue, ConstValue, LabelId,
    TypedBlockItem, TypedDeclInit, TypedDeclaration, TypedExpr, TypedExprKind, TypedExternalDecl,
    TypedForInit, TypedFunctionDef, TypedInitItem, TypedInitializer, TypedStmt, TypedStmtKind,
    UnaryOp as TypedUnaryOp, ValueCategory,
};
use crate::frontend::sema::types::{TypeId, TypeKind, type_size_of};
use crate::mir::ir::{
    BasicBlock, BinaryOp, BlockId, CastKind, CmpDomain, CmpKind, Instruction, MirAbiParam,
    MirBoundaryParam, MirBoundaryReturn, MirBoundarySignature, MirConst, MirExternFunction,
    MirFunction, MirFunctionSig, MirGlobal, MirGlobalInit, MirLinkage, MirProgram, MirRelocation,
    MirRelocationTarget, MirType, Operand, SlotId, StackSlot, Terminator, TypedVReg,
    UnaryOp as MirUnaryOp,
};
use crate::mir::passes::run_pass_pipeline;

/// Lower typed sema output into MIR.
pub fn lower_to_mir(sema: &SemaResult) -> MirProgram {
    lower_to_mir_with_optimization(sema, 0)
}

/// Lower typed AST output into MIR and run passes for the requested optimization level.
pub fn lower_to_mir_with_optimization(sema: &SemaResult, optimization: u32) -> MirProgram {
    let mut cx = MirBuildContext::new(sema);
    cx.lower_translation_unit();
    let mut program = cx.finish();
    run_pass_pipeline(&mut program, optimization);
    program
}

/// Loop control-flow targets used by `break` / `continue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LoopContext {
    break_target: BlockId,
    continue_target: BlockId,
}

/// Switch lowering state, including deferred case/default dispatch targets.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SwitchContext {
    break_target: BlockId,
    dispatch_block: BlockId,
    default_target: Option<BlockId>,
    case_targets: Vec<(i64, BlockId)>,
}

/// Unified stack for breakable constructs, preserving nearest-scope semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlContext {
    Loop { break_target: BlockId },
    Switch { break_target: BlockId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SysvAggregateLaneClass {
    Integer,
    Sse,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SysvAggregateLane {
    class: Option<SysvAggregateLaneClass>,
    bytes_used: u32,
}

/// A lowered write/read location.
///
/// `Stack` targets a stack slot with byte offset; `Ptr` targets an explicit
/// pointer operand.
#[derive(Debug, Clone, PartialEq)]
enum MirPlace {
    Stack { slot: SlotId, offset: i64 },
    Ptr(Operand),
}

/// Stateful MIR lowering context.
pub struct MirBuildContext<'a> {
    sema: &'a SemaResult,
    program: MirProgram,
    mir_type_cache: HashMap<TypeId, MirType>,
    signed_int_cache: HashMap<TypeId, bool>,
    object_global_names: HashMap<SymbolId, String>,
    emitted_object_globals: HashSet<SymbolId>,
    string_literal_globals: HashMap<String, String>,
    file_scope_compound_globals: HashMap<(usize, usize), String>,
    next_synthetic_global_id: u32,
    current_function: Option<MirFunctionBuilder>,
}

impl<'a> MirBuildContext<'a> {
    fn new(sema: &'a SemaResult) -> Self {
        Self {
            sema,
            program: MirProgram::default(),
            mir_type_cache: HashMap::new(),
            signed_int_cache: HashMap::new(),
            object_global_names: HashMap::new(),
            emitted_object_globals: HashSet::new(),
            string_literal_globals: HashMap::new(),
            file_scope_compound_globals: HashMap::new(),
            next_synthetic_global_id: 0,
            current_function: None,
        }
    }

    fn finish(self) -> MirProgram {
        self.program
    }

    /// Lower the full translation unit into MIR program components.
    fn lower_translation_unit(&mut self) {
        self.collect_global_skeletons();
        self.collect_extern_functions();
        self.collect_function_skeletons();
    }

    /// Query a sema symbol by id.
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        self.sema.symbols.get(id)
    }

    /// Map a sema type id to MIR primitive type.
    pub fn map_type(&mut self, ty: TypeId) -> MirType {
        if let Some(mapped) = self.mir_type_cache.get(&ty).copied() {
            return mapped;
        }
        let mapped = match self.sema.types.get(ty).kind {
            TypeKind::Bool | TypeKind::Char | TypeKind::SignedChar | TypeKind::UnsignedChar => {
                MirType::I8
            }
            TypeKind::Short { .. } => MirType::I16,
            TypeKind::Int { .. } | TypeKind::Enum(_) => MirType::I32,
            TypeKind::Long { .. } | TypeKind::LongLong { .. } => MirType::I64,
            TypeKind::Float => MirType::F32,
            TypeKind::Double => MirType::F64,
            TypeKind::Pointer { .. }
            | TypeKind::Array { .. }
            | TypeKind::Function(_)
            | TypeKind::Record(_) => MirType::Ptr,
            TypeKind::Void => MirType::Void,
            TypeKind::Error => MirType::Void,
        };
        self.mir_type_cache.insert(ty, mapped);
        mapped
    }

    /// Check whether a sema integer-like type is signed.
    pub fn is_signed_integer(&mut self, ty: TypeId) -> bool {
        if let Some(value) = self.signed_int_cache.get(&ty).copied() {
            return value;
        }
        let signed = match self.sema.types.get(ty).kind {
            TypeKind::Char => true,
            TypeKind::SignedChar => true,
            TypeKind::UnsignedChar => false,
            TypeKind::Short { signed }
            | TypeKind::Int { signed }
            | TypeKind::Long { signed }
            | TypeKind::LongLong { signed } => signed,
            TypeKind::Enum(_) => true,
            _ => false,
        };
        self.signed_int_cache.insert(ty, signed);
        signed
    }

    /// Map a sema function type to MIR signature.
    pub fn map_function_sig(&mut self, ty: TypeId) -> Option<MirFunctionSig> {
        let fn_ty = self.extract_function_type(ty)?;
        let (params, return_type, _) = self.canonicalize_function_abi(&fn_ty);
        Some(MirFunctionSig {
            params,
            return_type,
            variadic: fn_ty.variadic,
        })
    }

    /// Map a sema function type to the source-level x64 SysV boundary ABI.
    pub fn map_boundary_sig(&mut self, ty: TypeId) -> Option<MirBoundarySignature> {
        let fn_ty = self.extract_function_type(ty)?;
        Some(MirBoundarySignature {
            params: fn_ty
                .params
                .iter()
                .map(|&param_ty| self.classify_boundary_param(param_ty))
                .collect(),
            return_type: self.classify_boundary_return(fn_ty.ret),
            variadic: fn_ty.variadic,
        })
    }

    fn classify_boundary_param(&mut self, ty: TypeId) -> MirBoundaryParam {
        if !self.is_aggregate_type(ty) {
            return MirBoundaryParam::Scalar(self.map_type(ty));
        }

        let (size, _) = self.stack_layout_of(ty);
        if size > 16 {
            return MirBoundaryParam::AggregateMemory {
                size,
                abi_size: abi_struct_stack_size(size),
            };
        }

        match self.classify_small_sysv_aggregate(ty, size) {
            Some(parts) => MirBoundaryParam::AggregateScalarized { parts, size },
            None => MirBoundaryParam::AggregateUnsupported { size },
        }
    }

    fn classify_boundary_return(&mut self, ty: TypeId) -> MirBoundaryReturn {
        if !self.is_aggregate_type(ty) {
            if self.map_type(ty) == MirType::Void {
                return MirBoundaryReturn::Void;
            }
            return MirBoundaryReturn::Scalar(self.map_type(ty));
        }

        let (size, _) = self.stack_layout_of(ty);
        if size > 16 {
            return MirBoundaryReturn::AggregateMemory { size };
        }

        match self.classify_small_sysv_aggregate(ty, size) {
            Some(parts) => MirBoundaryReturn::AggregateScalarized { parts, size },
            None => MirBoundaryReturn::AggregateUnsupported { size },
        }
    }

    fn classify_small_sysv_aggregate(&self, ty: TypeId, size: u32) -> Option<Vec<MirType>> {
        let mut lanes = [SysvAggregateLane::default(), SysvAggregateLane::default()];
        self.classify_type_into_sysv_lanes(ty, 0, &mut lanes)?;

        let lane_count = if size > 8 { 2 } else { 1 };
        let mut parts = Vec::with_capacity(lane_count);
        for lane in lanes.into_iter().take(lane_count) {
            let class = lane.class?;
            let part = match class {
                SysvAggregateLaneClass::Integer => MirType::I64,
                SysvAggregateLaneClass::Sse => MirType::F64,
            };
            parts.push(part);
        }
        Some(parts)
    }

    fn classify_type_into_sysv_lanes(
        &self,
        ty: TypeId,
        byte_offset: u32,
        lanes: &mut [SysvAggregateLane; 2],
    ) -> Option<()> {
        let ty_def = self.sema.types.get(ty);
        match &ty_def.kind {
            TypeKind::Bool
            | TypeKind::Char
            | TypeKind::SignedChar
            | TypeKind::UnsignedChar
            | TypeKind::Short { .. }
            | TypeKind::Int { .. }
            | TypeKind::Long { .. }
            | TypeKind::LongLong { .. }
            | TypeKind::Pointer { .. }
            | TypeKind::Enum(_) => {
                let size =
                    u32::try_from(type_size_of(ty, &self.sema.types, &self.sema.records)?).ok()?;
                self.mark_sysv_lanes(byte_offset, size, SysvAggregateLaneClass::Integer, lanes)
            }
            TypeKind::Float | TypeKind::Double => {
                let size =
                    u32::try_from(type_size_of(ty, &self.sema.types, &self.sema.records)?).ok()?;
                let lane_offset = byte_offset % 8;
                if lane_offset + size > 8 {
                    return None;
                }
                self.mark_sysv_lanes(byte_offset, size, SysvAggregateLaneClass::Sse, lanes)
            }
            TypeKind::Array { elem, len } => {
                let elem_size =
                    u32::try_from(type_size_of(*elem, &self.sema.types, &self.sema.records)?)
                        .ok()?;
                let len = match len {
                    crate::frontend::sema::types::ArrayLen::Known(len) => *len,
                    _ => return None,
                };
                for index in 0..len {
                    let index = u32::try_from(index).ok()?;
                    self.classify_type_into_sysv_lanes(
                        *elem,
                        byte_offset.checked_add(index.checked_mul(elem_size)?)?,
                        lanes,
                    )?;
                }
                Some(())
            }
            TypeKind::Record(record_id) => {
                let record = self.sema.records.get(*record_id);
                if !record.is_complete {
                    return None;
                }
                match record.kind {
                    crate::frontend::parser::ast::RecordKind::Struct => {
                        let mut field_offset = byte_offset;
                        for field in &record.fields {
                            if field.bit_width.is_some() {
                                return None;
                            }
                            self.classify_type_into_sysv_lanes(field.ty, field_offset, lanes)?;
                            let field_size = u32::try_from(type_size_of(
                                field.ty,
                                &self.sema.types,
                                &self.sema.records,
                            )?)
                            .ok()?;
                            field_offset = field_offset.checked_add(field_size)?;
                        }
                        Some(())
                    }
                    crate::frontend::parser::ast::RecordKind::Union => {
                        for field in &record.fields {
                            if field.bit_width.is_some() {
                                return None;
                            }
                            self.classify_type_into_sysv_lanes(field.ty, byte_offset, lanes)?;
                        }
                        Some(())
                    }
                }
            }
            TypeKind::Function(_) | TypeKind::Void | TypeKind::Error => None,
        }
    }

    fn mark_sysv_lanes(
        &self,
        byte_offset: u32,
        size: u32,
        class: SysvAggregateLaneClass,
        lanes: &mut [SysvAggregateLane; 2],
    ) -> Option<()> {
        if size == 0 {
            return Some(());
        }
        let end = byte_offset.checked_add(size)?;
        if end > 16 {
            return None;
        }

        let first_lane = usize::try_from(byte_offset / 8).ok()?;
        let last_lane = usize::try_from((end - 1) / 8).ok()?;
        for lane_index in first_lane..=last_lane {
            let lane_start = (lane_index as u32) * 8;
            let covered_end = end.min(lane_start + 8);
            let bytes_used = covered_end.checked_sub(lane_start)?;
            let lane = &mut lanes[lane_index];
            lane.bytes_used = lane.bytes_used.max(bytes_used);
            lane.class = Some(match (lane.class, class) {
                (None, new_class) => new_class,
                (Some(existing), new_class) if existing == new_class => existing,
                (Some(SysvAggregateLaneClass::Integer), _) => SysvAggregateLaneClass::Integer,
                (Some(SysvAggregateLaneClass::Sse), SysvAggregateLaneClass::Integer) => {
                    SysvAggregateLaneClass::Integer
                }
                (Some(SysvAggregateLaneClass::Sse), SysvAggregateLaneClass::Sse) => {
                    SysvAggregateLaneClass::Sse
                }
            });
        }
        Some(())
    }

    /// Enter one function lowering session.
    pub fn begin_function(
        &mut self,
        name: String,
        linkage: MirLinkage,
        params: Vec<MirAbiParam>,
        return_type: MirType,
        boundary_sig: MirBoundarySignature,
        variadic: bool,
        source_return_ty: TypeId,
    ) {
        debug_assert!(
            self.current_function.is_none(),
            "begin_function called with active function state"
        );
        self.current_function = Some(MirFunctionBuilder::new(
            name,
            linkage,
            params,
            return_type,
            boundary_sig,
            variadic,
            source_return_ty,
        ));
    }

    /// Finish current function and move it into the MIR program.
    pub fn end_function(&mut self) {
        let Some(func_builder) = self.current_function.take() else {
            return;
        };
        self.program.functions.push(func_builder.finish());
    }

    /// Allocate one basic block id in current function.
    pub fn alloc_block(&mut self) -> BlockId {
        self.current_function_mut().alloc_block()
    }

    /// Allocate one stack slot in current function.
    pub fn alloc_slot(&mut self, size: u32, alignment: u32) -> SlotId {
        self.current_function_mut().alloc_slot(size, alignment)
    }

    /// Allocate one typed virtual register in current function.
    pub fn alloc_vreg(&mut self, ty: MirType) -> TypedVReg {
        self.current_function_mut().alloc_vreg(ty)
    }

    /// Ensure one label has a pre-assigned block id.
    pub fn ensure_label_block(&mut self, label: LabelId) -> BlockId {
        self.current_function_mut().ensure_label_block(label)
    }

    /// Push loop control-flow context.
    pub fn push_loop_context(&mut self, break_target: BlockId, continue_target: BlockId) {
        let current = self.current_function_mut();
        current.loop_stack.push(LoopContext {
            break_target,
            continue_target,
        });
        current
            .control_stack
            .push(ControlContext::Loop { break_target });
    }

    /// Pop loop control-flow context.
    pub fn pop_loop_context(&mut self) -> Option<(BlockId, BlockId)> {
        let current = self.current_function_mut();
        let popped = current.loop_stack.pop();
        if popped.is_some() {
            let Some(ControlContext::Loop { .. }) = current.control_stack.pop() else {
                panic!("loop control stack out of sync");
            };
        }
        popped.map(|ctx| (ctx.break_target, ctx.continue_target))
    }

    /// Peek active loop control-flow context.
    pub fn current_loop_context(&mut self) -> Option<(BlockId, BlockId)> {
        self.current_function_mut()
            .loop_stack
            .last()
            .copied()
            .map(|ctx| (ctx.break_target, ctx.continue_target))
    }

    /// Push switch control-flow context.
    pub fn push_switch_context(&mut self, break_target: BlockId, default_target: Option<BlockId>) {
        self.push_switch_context_with_dispatch(break_target, BlockId(u32::MAX), default_target);
    }

    /// Pop switch control-flow context.
    pub fn pop_switch_context(&mut self) -> Option<(BlockId, Option<BlockId>)> {
        let current = self.current_function_mut();
        let popped = current.switch_stack.pop();
        if popped.is_some() {
            let Some(ControlContext::Switch { .. }) = current.control_stack.pop() else {
                panic!("switch control stack out of sync");
            };
        }
        popped.map(|ctx| (ctx.break_target, ctx.default_target))
    }

    /// Peek active switch control-flow context.
    pub fn current_switch_context(&mut self) -> Option<(BlockId, Option<BlockId>)> {
        self.current_function_mut()
            .switch_stack
            .last()
            .map(|ctx| (ctx.break_target, ctx.default_target))
    }

    fn current_function_mut(&mut self) -> &mut MirFunctionBuilder {
        self.current_function
            .as_mut()
            .expect("MIR function context is not active")
    }

    fn push_switch_context_with_dispatch(
        &mut self,
        break_target: BlockId,
        dispatch_block: BlockId,
        default_target: Option<BlockId>,
    ) {
        let current = self.current_function_mut();
        current.switch_stack.push(SwitchContext {
            break_target,
            dispatch_block,
            default_target,
            case_targets: Vec::new(),
        });
        current
            .control_stack
            .push(ControlContext::Switch { break_target });
    }

    /// Pre-collect file-scope and block-scope static/extern objects as MIR globals.
    fn collect_global_skeletons(&mut self) {
        let mut object_ids = HashSet::new();
        let mut init_by_symbol = HashMap::new();
        for item in &self.sema.typed_tu.items {
            match item {
                TypedExternalDecl::Declaration(decl) => {
                    for symbol_id in &decl.symbols {
                        let symbol = self.symbol(*symbol_id);
                        if symbol.kind() == SymbolKind::Object
                            && matches!(
                                symbol.object_storage_class(),
                                Some(ObjectStorageClass::FileScope | ObjectStorageClass::Extern)
                            )
                        {
                            object_ids.insert(*symbol_id);
                        }
                    }
                    for TypedDeclInit { symbol, init } in &decl.initializers {
                        let target = self.symbol(*symbol);
                        if target.kind() == SymbolKind::Object
                            && target.object_storage_class() == Some(ObjectStorageClass::FileScope)
                        {
                            init_by_symbol.insert(*symbol, init.clone());
                        }
                    }
                }
                TypedExternalDecl::Function(function) => {
                    Self::collect_declared_symbols_in_stmt(&function.body, &mut object_ids);
                }
            }
        }

        let mut ordered_ids: Vec<SymbolId> = object_ids.into_iter().collect();
        ordered_ids.sort_by_key(|id| id.0);
        for symbol_id in ordered_ids {
            if self.emitted_object_globals.contains(&symbol_id) {
                continue;
            }
            let (name, ty, linkage, status, storage) = {
                let symbol = self.symbol(symbol_id);
                (
                    symbol.name().to_string(),
                    symbol.ty(),
                    symbol.linkage(),
                    symbol.status(),
                    symbol.object_storage_class(),
                )
            };
            if !matches!(
                storage,
                Some(ObjectStorageClass::FileScope | ObjectStorageClass::Extern)
            ) {
                continue;
            }
            let size = type_size_of(ty, &self.sema.types, &self.sema.records).unwrap_or(0);
            let alignment = self.type_alignment_of(ty).unwrap_or(1);
            let init = if storage == Some(ObjectStorageClass::Extern)
                || status == DefinitionStatus::Declared
            {
                None
            } else if let Some(typed_init) = init_by_symbol.get(&symbol_id) {
                Some(self.lower_global_initializer(ty, typed_init))
            } else {
                Some(MirGlobalInit::Zero)
            };
            self.object_global_names.insert(symbol_id, name.clone());
            self.emitted_object_globals.insert(symbol_id);
            self.program.globals.push(MirGlobal {
                name,
                size,
                alignment,
                linkage: map_linkage(linkage),
                init,
            });
        }
    }

    /// Collect extern function declarations referenced in this translation unit.
    fn collect_extern_functions(&mut self) {
        let mut candidate_ids = HashSet::new();
        for item in &self.sema.typed_tu.items {
            match item {
                TypedExternalDecl::Declaration(decl) => {
                    for symbol_id in &decl.symbols {
                        candidate_ids.insert(*symbol_id);
                    }
                }
                TypedExternalDecl::Function(function) => {
                    Self::collect_declared_symbols_in_stmt(&function.body, &mut candidate_ids);
                }
            }
        }

        let mut ordered_ids: Vec<SymbolId> = candidate_ids.into_iter().collect();
        ordered_ids.sort_by_key(|id| id.0);
        for symbol_id in ordered_ids {
            let symbol = self.symbol(symbol_id);
            if symbol.kind() != SymbolKind::Function
                || symbol.status() == DefinitionStatus::Defined
                || symbol.linkage() != Linkage::External
            {
                continue;
            }
            let (name, ty) = (symbol.name().to_string(), symbol.ty());
            let Some(sig) = self.map_function_sig(ty) else {
                continue;
            };
            let Some(boundary_sig) = self.map_boundary_sig(ty) else {
                continue;
            };
            self.program.extern_functions.push(MirExternFunction {
                name,
                sig,
                boundary_sig,
            });
        }
    }

    fn collect_declared_symbols_in_stmt(stmt: &TypedStmt, out: &mut HashSet<SymbolId>) {
        match &stmt.kind {
            TypedStmtKind::Compound(items) => {
                for item in items {
                    match item {
                        TypedBlockItem::Declaration(decl) => {
                            for symbol_id in &decl.symbols {
                                out.insert(*symbol_id);
                            }
                        }
                        TypedBlockItem::Stmt(stmt) => {
                            Self::collect_declared_symbols_in_stmt(stmt, out)
                        }
                    }
                }
            }
            TypedStmtKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                Self::collect_declared_symbols_in_stmt(then_branch, out);
                if let Some(else_branch) = else_branch {
                    Self::collect_declared_symbols_in_stmt(else_branch, out);
                }
            }
            TypedStmtKind::Switch { body, .. }
            | TypedStmtKind::While { body, .. }
            | TypedStmtKind::DoWhile { body, .. }
            | TypedStmtKind::Label { stmt: body, .. }
            | TypedStmtKind::Case { stmt: body, .. }
            | TypedStmtKind::Default { stmt: body } => {
                Self::collect_declared_symbols_in_stmt(body, out)
            }
            TypedStmtKind::For { init, body, .. } => {
                if let Some(TypedForInit::Decl(decl)) = init {
                    for symbol_id in &decl.symbols {
                        out.insert(*symbol_id);
                    }
                }
                Self::collect_declared_symbols_in_stmt(body, out);
            }
            TypedStmtKind::Expr(_)
            | TypedStmtKind::Return(_)
            | TypedStmtKind::Break
            | TypedStmtKind::Continue
            | TypedStmtKind::Goto(_) => {}
        }
    }

    /// Lower all defined function bodies into MIR functions.
    fn collect_function_skeletons(&mut self) {
        let functions: Vec<TypedFunctionDef> = self
            .sema
            .typed_tu
            .items
            .iter()
            .filter_map(|item| match item {
                TypedExternalDecl::Function(function) => Some(function.clone()),
                TypedExternalDecl::Declaration(_) => None,
            })
            .collect();

        for function in &functions {
            self.lower_function_skeleton(function);
        }
    }

    /// Lower one function definition, including ABI canonicalization and body CFG.
    fn lower_function_skeleton(&mut self, function: &TypedFunctionDef) {
        let (name, symbol_ty, status, linkage) = {
            let symbol = self.symbol(function.symbol);
            (
                symbol.name().to_string(),
                symbol.ty(),
                symbol.status(),
                map_linkage(symbol.linkage()),
            )
        };
        if status != DefinitionStatus::Defined {
            return;
        }

        let Some(function_ty) = self.extract_function_type(symbol_ty) else {
            return;
        };
        let (params, return_type, has_sret) = self.canonicalize_function_abi(&function_ty);
        let boundary_sig = MirBoundarySignature {
            params: function_ty
                .params
                .iter()
                .map(|&param_ty| self.classify_boundary_param(param_ty))
                .collect(),
            return_type: self.classify_boundary_return(function_ty.ret),
            variadic: function_ty.variadic,
        };
        let param_symbols = self.collect_function_param_symbols(function, function_ty.params.len());

        self.begin_function(
            name,
            linkage,
            params,
            return_type,
            boundary_sig,
            function_ty.variadic,
            function_ty.ret,
        );
        self.prescan_labels(&function.body);

        if has_sret {
            let sret_slot = self.alloc_slot(8, 8);
            self.current_function_mut().set_sret_slot(sret_slot);
            let incoming_sret = self.alloc_vreg(MirType::Ptr);
            self.emit_entry_instruction(Instruction::Store {
                slot: sret_slot,
                offset: 0,
                value: Operand::VReg(incoming_sret.reg),
                ty: MirType::Ptr,
                volatile: false,
            });
        }

        for (idx, param_ty) in function_ty.params.iter().enumerate() {
            let is_aggregate = self.is_aggregate_type(*param_ty);
            let (size, alignment) = if is_aggregate {
                self.stack_layout_of(*param_ty)
            } else {
                let mapped = self.map_type(*param_ty);
                (
                    Self::mir_type_size_bytes(mapped).max(1),
                    self.type_alignment_of(*param_ty).unwrap_or(1),
                )
            };
            let slot = self.alloc_slot(size, alignment);
            if let Some(symbol_id) = param_symbols.get(idx).copied() {
                self.bind_symbol_slot(symbol_id, slot);
            }

            if is_aggregate {
                let incoming_ptr = self.alloc_vreg(MirType::Ptr);
                let dst_addr = self.alloc_vreg(MirType::Ptr);
                self.current_function_mut().mark_slot_address_taken(slot);
                self.emit_entry_instruction(Instruction::SlotAddr {
                    dst: dst_addr,
                    slot,
                });
                self.emit_entry_instruction(Instruction::Memcpy {
                    dst_ptr: Operand::VReg(dst_addr.reg),
                    src_ptr: Operand::VReg(incoming_ptr.reg),
                    size,
                });
            } else {
                // Current MIR has no explicit arg operand. Reserve one vreg per parameter
                // as the incoming ABI value and spill it into the slot in the entry block.
                let mir_ty = self.map_type(*param_ty);
                let incoming = self.alloc_vreg(mir_ty);
                self.emit_entry_instruction(Instruction::Store {
                    slot,
                    offset: 0,
                    value: Operand::VReg(incoming.reg),
                    ty: mir_ty,
                    volatile: false,
                });
            }
        }

        self.allocate_local_slots_from_stmt(&function.body);
        self.lower_stmt(&function.body);
        if !self.current_function_mut().is_current_block_terminated() {
            let return_type = self.current_function_mut().function.return_type;
            if return_type == MirType::Void {
                self.emit_current_terminator(Terminator::Ret(None));
            }
        }
        self.end_function();
    }

    fn collect_function_param_symbols(
        &self,
        function: &TypedFunctionDef,
        expected_count: usize,
    ) -> Vec<SymbolId> {
        let mut params = Vec::new();
        for idx in 0..self.sema.symbols.len() {
            let symbol_id = SymbolId(idx as u32);
            let symbol = self.sema.symbols.get(symbol_id);
            if symbol.kind() != SymbolKind::Object {
                continue;
            }
            if symbol.object_storage_class() != Some(ObjectStorageClass::Auto) {
                continue;
            }
            let span = symbol.decl_span();
            if span.start < function.body.span.start
                && span.start >= function.span.start
                && span.end <= function.body.span.start
            {
                params.push(symbol_id);
            }
        }
        params.truncate(expected_count);
        params
    }

    /// Pre-allocate basic blocks for labels so forward `goto` targets are valid.
    fn prescan_labels(&mut self, stmt: &TypedStmt) {
        match &stmt.kind {
            TypedStmtKind::Compound(items) => {
                for item in items {
                    if let TypedBlockItem::Stmt(stmt) = item {
                        self.prescan_labels(stmt);
                    }
                }
            }
            TypedStmtKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.prescan_labels(then_branch);
                if let Some(else_branch) = else_branch {
                    self.prescan_labels(else_branch);
                }
            }
            TypedStmtKind::Switch { body, .. }
            | TypedStmtKind::While { body, .. }
            | TypedStmtKind::DoWhile { body, .. } => self.prescan_labels(body),
            TypedStmtKind::For { body, .. } => self.prescan_labels(body),
            TypedStmtKind::Label { label, stmt } => {
                let _ = self.ensure_label_block(*label);
                self.prescan_labels(stmt);
            }
            TypedStmtKind::Case { stmt, .. } | TypedStmtKind::Default { stmt } => {
                self.prescan_labels(stmt)
            }
            TypedStmtKind::Expr(_)
            | TypedStmtKind::Return(_)
            | TypedStmtKind::Break
            | TypedStmtKind::Continue
            | TypedStmtKind::Goto(_) => {}
        }
    }

    /// Allocate stack slots for all local objects in the function body.
    fn allocate_local_slots_from_stmt(&mut self, stmt: &TypedStmt) {
        match &stmt.kind {
            TypedStmtKind::Compound(items) => {
                for item in items {
                    match item {
                        TypedBlockItem::Declaration(decl) => {
                            self.allocate_slots_for_declaration(decl)
                        }
                        TypedBlockItem::Stmt(stmt) => self.allocate_local_slots_from_stmt(stmt),
                    }
                }
            }
            TypedStmtKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.allocate_local_slots_from_stmt(then_branch);
                if let Some(else_branch) = else_branch {
                    self.allocate_local_slots_from_stmt(else_branch);
                }
            }
            TypedStmtKind::Switch { body, .. }
            | TypedStmtKind::While { body, .. }
            | TypedStmtKind::DoWhile { body, .. }
            | TypedStmtKind::Label { stmt: body, .. }
            | TypedStmtKind::Case { stmt: body, .. }
            | TypedStmtKind::Default { stmt: body } => self.allocate_local_slots_from_stmt(body),
            TypedStmtKind::For { init, body, .. } => {
                if let Some(TypedForInit::Decl(decl)) = init {
                    self.allocate_slots_for_declaration(decl);
                }
                self.allocate_local_slots_from_stmt(body);
            }
            TypedStmtKind::Expr(_)
            | TypedStmtKind::Return(_)
            | TypedStmtKind::Break
            | TypedStmtKind::Continue
            | TypedStmtKind::Goto(_) => {}
        }
    }

    fn allocate_slots_for_declaration(&mut self, decl: &TypedDeclaration) {
        for symbol_id in &decl.symbols {
            self.allocate_slot_for_local_symbol(*symbol_id);
        }
    }

    fn allocate_slot_for_local_symbol(&mut self, symbol_id: SymbolId) {
        if self.current_function_mut().has_symbol_slot(symbol_id) {
            return;
        }

        let (should_allocate, ty) = {
            let symbol = self.symbol(symbol_id);
            let should_allocate = symbol.kind() == SymbolKind::Object
                && matches!(
                    symbol.object_storage_class(),
                    Some(ObjectStorageClass::Auto | ObjectStorageClass::Register)
                );
            (should_allocate, symbol.ty())
        };
        if !should_allocate {
            return;
        }

        let (size, alignment) = self.stack_layout_of(ty);
        let slot = self.alloc_slot(size, alignment);
        self.bind_symbol_slot(symbol_id, slot);
    }

    fn bind_symbol_slot(&mut self, symbol_id: SymbolId, slot: SlotId) {
        self.current_function_mut()
            .bind_symbol_slot(symbol_id, slot);
    }

    fn lower_declaration_initializers(&mut self, decl: &TypedDeclaration) {
        let mut initialized_symbols = HashSet::new();
        for TypedDeclInit { symbol, init } in &decl.initializers {
            initialized_symbols.insert(*symbol);
            self.lower_single_declaration_initializer(*symbol, init);
        }

        for symbol_id in &decl.symbols {
            if initialized_symbols.contains(symbol_id) {
                continue;
            }
            let symbol = self.symbol(*symbol_id);
            if symbol.kind() == SymbolKind::Object
                && symbol.object_storage_class() == Some(ObjectStorageClass::Static)
            {
                self.materialize_block_static_global(*symbol_id, None);
            }
        }
    }

    fn lower_single_declaration_initializer(
        &mut self,
        symbol_id: SymbolId,
        init: &TypedInitializer,
    ) {
        let (kind, storage, ty) = {
            let symbol = self.symbol(symbol_id);
            (symbol.kind(), symbol.object_storage_class(), symbol.ty())
        };
        if kind != SymbolKind::Object {
            return;
        }

        match storage {
            Some(ObjectStorageClass::Auto | ObjectStorageClass::Register) => {
                self.ensure_current_block_for_fallthrough();
                if let Some(place) = self.lower_symbol_place(symbol_id) {
                    self.lower_local_initializer(place, ty, init);
                }
            }
            Some(ObjectStorageClass::Static) => {
                self.materialize_block_static_global(symbol_id, Some(init));
            }
            Some(ObjectStorageClass::FileScope | ObjectStorageClass::Extern) | None => {}
        }
    }

    fn lower_local_initializer(&mut self, place: MirPlace, ty: TypeId, init: &TypedInitializer) {
        match init {
            TypedInitializer::Expr(expr) => {
                if self.is_aggregate_type(ty) {
                    self.copy_aggregate_into_place(place, ty, expr);
                    return;
                }

                let rhs_raw = self.lower_expr_to_operand(expr);
                let rhs = self.cast_operand_to_type(rhs_raw, expr.ty, ty);
                self.store_to_place(place, rhs, ty);
            }
            TypedInitializer::ZeroInit { .. } => {
                if self.is_aggregate_type(ty) {
                    self.emit_memset_place(place, ty);
                } else {
                    let zero = self.zero_operand_for_type(ty);
                    self.store_to_place(place, zero, ty);
                }
            }
            TypedInitializer::Aggregate(items) => {
                self.lower_local_aggregate_initializer(place, ty, items);
            }
            TypedInitializer::SparseArray {
                elem_ty,
                total_len: _,
                entries,
            } => {
                self.emit_memset_place(place.clone(), ty);
                let elem_size = i64::from(self.type_size_u32(*elem_ty));
                if elem_size == 0 {
                    return;
                }
                for (index, item) in entries {
                    if self.is_zero_initializer_tree(&item.init) {
                        continue;
                    }
                    let offset = i64::try_from(*index).unwrap_or(i64::MAX) * elem_size;
                    let elem_place = self.place_with_byte_offset(place.clone(), offset);
                    self.lower_local_initializer(elem_place, *elem_ty, &item.init);
                }
            }
        }
    }

    fn lower_local_aggregate_initializer(
        &mut self,
        place: MirPlace,
        ty: TypeId,
        items: &[TypedInitItem],
    ) {
        self.emit_memset_place(place.clone(), ty);

        match self.sema.types.get(ty).kind.clone() {
            TypeKind::Array { elem, .. } => {
                let elem_size = i64::from(self.type_size_u32(elem));
                if elem_size == 0 {
                    return;
                }
                for (index, item) in items.iter().enumerate() {
                    if self.is_zero_initializer_tree(&item.init) {
                        continue;
                    }
                    let offset = i64::try_from(index).unwrap_or(i64::MAX) * elem_size;
                    let elem_place = self.place_with_byte_offset(place.clone(), offset);
                    self.lower_local_initializer(elem_place, elem, &item.init);
                }
            }
            TypeKind::Record(record_id) => {
                let record = self.sema.records.get(record_id);
                match record.kind {
                    crate::frontend::parser::ast::RecordKind::Struct => {
                        for (field_index, item) in items.iter().enumerate() {
                            if field_index >= record.fields.len()
                                || self.is_zero_initializer_tree(&item.init)
                            {
                                continue;
                            }
                            let field = &record.fields[field_index];
                            let offset = self
                                .record_field_offset(
                                    record_id,
                                    crate::frontend::sema::types::FieldId(field_index as u32),
                                )
                                .unwrap_or(0);
                            let field_place = self.place_with_byte_offset(place.clone(), offset);
                            self.lower_local_initializer(field_place, field.ty, &item.init);
                        }
                    }
                    crate::frontend::parser::ast::RecordKind::Union => {
                        for (field_index, item) in items.iter().enumerate() {
                            if field_index >= record.fields.len()
                                || self.is_zero_initializer_tree(&item.init)
                            {
                                continue;
                            }
                            let field = &record.fields[field_index];
                            self.lower_local_initializer(place.clone(), field.ty, &item.init);
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn emit_memset_place(&mut self, place: MirPlace, ty: TypeId) {
        let size = self.type_size_u32(ty);
        if size == 0 {
            return;
        }
        let dst_ptr = self.address_of_place(place);
        self.emit_current_instruction(Instruction::Memset {
            dst_ptr,
            value: Operand::Const(MirConst::ZeroConst),
            size,
        });
    }

    fn place_with_byte_offset(&mut self, place: MirPlace, byte_offset: i64) -> MirPlace {
        if byte_offset == 0 {
            return place;
        }
        match place {
            MirPlace::Stack { slot, offset } => MirPlace::Stack {
                slot,
                offset: offset + byte_offset,
            },
            MirPlace::Ptr(ptr) => {
                let addr = self.emit_ptr_add_const(ptr, byte_offset);
                MirPlace::Ptr(addr)
            }
        }
    }

    fn type_size_u32(&self, ty: TypeId) -> u32 {
        type_size_of(ty, &self.sema.types, &self.sema.records)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    }

    fn is_zero_initializer_tree(&self, init: &TypedInitializer) -> bool {
        match init {
            TypedInitializer::ZeroInit { .. } => true,
            TypedInitializer::Expr(expr) => match expr.const_value {
                Some(ConstValue::Int(0))
                | Some(ConstValue::UInt(0))
                | Some(ConstValue::NullPtr) => true,
                Some(ConstValue::FloatBits(bits)) => bits == 0,
                _ => false,
            },
            TypedInitializer::Aggregate(items) => items
                .iter()
                .all(|item| self.is_zero_initializer_tree(&item.init)),
            TypedInitializer::SparseArray { entries, .. } => entries
                .values()
                .all(|item| self.is_zero_initializer_tree(&item.init)),
        }
    }

    fn materialize_block_static_global(
        &mut self,
        symbol_id: SymbolId,
        init: Option<&TypedInitializer>,
    ) {
        if self.emitted_object_globals.contains(&symbol_id) {
            return;
        }
        let (name, ty) = {
            let symbol = self.symbol(symbol_id);
            (
                format!("__static_{}_{}", symbol.name(), symbol_id.0),
                symbol.ty(),
            )
        };
        let size = type_size_of(ty, &self.sema.types, &self.sema.records).unwrap_or(0);
        let alignment = self.type_alignment_of(ty).unwrap_or(1);

        self.object_global_names.insert(symbol_id, name.clone());
        self.emitted_object_globals.insert(symbol_id);
        let global_init = init
            .map(|value| self.lower_global_initializer(ty, value))
            .unwrap_or(MirGlobalInit::Zero);

        self.program.globals.push(MirGlobal {
            name,
            size,
            alignment,
            linkage: MirLinkage::Internal,
            init: Some(global_init),
        });
    }

    fn fresh_synthetic_global_name(&mut self, prefix: &str) -> String {
        let name = format!("__{}_{}", prefix, self.next_synthetic_global_id);
        self.next_synthetic_global_id += 1;
        name
    }

    fn ensure_string_literal_global_name(&mut self, text: &str) -> String {
        if let Some(name) = self.string_literal_globals.get(text) {
            return name.clone();
        }

        let name = loop {
            let candidate = format!(".str.{}", self.next_synthetic_global_id);
            self.next_synthetic_global_id += 1;
            if !self
                .program
                .globals
                .iter()
                .any(|global| global.name == candidate)
            {
                break candidate;
            }
        };
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        self.program.globals.push(MirGlobal {
            name: name.clone(),
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            alignment: 1,
            linkage: MirLinkage::Internal,
            init: Some(MirGlobalInit::Data(bytes)),
        });
        self.string_literal_globals
            .insert(text.to_string(), name.clone());
        name
    }

    fn ensure_file_scope_compound_literal_global_name(
        &mut self,
        key: (usize, usize),
        ty: TypeId,
        init: &TypedInitializer,
    ) -> String {
        if let Some(name) = self.file_scope_compound_globals.get(&key) {
            return name.clone();
        }

        let name = self.fresh_synthetic_global_name("compound");
        let size = type_size_of(ty, &self.sema.types, &self.sema.records).unwrap_or(0);
        let alignment = self.type_alignment_of(ty).unwrap_or(1);
        let global_init = self.lower_global_initializer(ty, init);
        self.program.globals.push(MirGlobal {
            name: name.clone(),
            size,
            alignment,
            linkage: MirLinkage::Internal,
            init: Some(global_init),
        });
        self.file_scope_compound_globals.insert(key, name.clone());
        name
    }

    fn emit_global_addr_operand(&mut self, global: String) -> Operand {
        let dst = self.alloc_vreg(MirType::Ptr);
        self.emit_current_instruction(Instruction::GlobalAddr { dst, global });
        Operand::VReg(dst.reg)
    }

    fn string_literal_place(&mut self, text: &str) -> MirPlace {
        let name = self.ensure_string_literal_global_name(text);
        MirPlace::Ptr(self.emit_global_addr_operand(name))
    }

    fn materialize_compound_literal_place(
        &mut self,
        ty: TypeId,
        init: &TypedInitializer,
        is_file_scope: bool,
        span_key: (usize, usize),
    ) -> MirPlace {
        if is_file_scope {
            let name = self.ensure_file_scope_compound_literal_global_name(span_key, ty, init);
            return MirPlace::Ptr(self.emit_global_addr_operand(name));
        }

        let slot = if let Some(slot) = self.current_function_mut().compound_literal_slot(span_key) {
            slot
        } else {
            let (size, alignment) = self.stack_layout_of(ty);
            let slot = self.alloc_slot(size, alignment);
            self.current_function_mut()
                .bind_compound_literal_slot(span_key, slot);
            slot
        };
        let place = MirPlace::Stack { slot, offset: 0 };
        self.lower_local_initializer(place.clone(), ty, init);
        place
    }

    fn copy_aggregate_into_place(&mut self, place: MirPlace, ty: TypeId, value: &TypedExpr) {
        let size = self.type_size_u32(ty);
        if size == 0 {
            return;
        }
        let src_ptr = self
            .lower_expr_address(value)
            .unwrap_or_else(|| self.lower_expr_to_operand(value));
        let dst_ptr = self.address_of_place(place);
        self.emit_current_instruction(Instruction::Memcpy {
            dst_ptr,
            src_ptr,
            size,
        });
    }

    fn emit_entry_instruction(&mut self, instruction: Instruction) {
        self.current_function_mut()
            .emit_instruction(BlockId(0), instruction);
    }

    fn emit_current_instruction(&mut self, instruction: Instruction) {
        self.current_function_mut()
            .emit_current_instruction(instruction);
    }

    fn emit_current_terminator(&mut self, terminator: Terminator) {
        self.current_function_mut()
            .emit_current_terminator(terminator);
    }

    fn set_current_block(&mut self, block: BlockId) {
        self.current_function_mut().set_current_block(block);
    }

    fn ensure_current_block_for_fallthrough(&mut self) {
        if self.current_function_mut().is_current_block_terminated() {
            let block = self.alloc_block();
            self.set_current_block(block);
        }
    }

    /// Lower one statement into MIR instructions/terminators.
    fn lower_stmt(&mut self, stmt: &TypedStmt) {
        match &stmt.kind {
            TypedStmtKind::Compound(items) => {
                for item in items {
                    match item {
                        TypedBlockItem::Declaration(decl) => {
                            self.lower_declaration_initializers(decl)
                        }
                        TypedBlockItem::Stmt(stmt) => self.lower_stmt(stmt),
                    }
                }
            }
            TypedStmtKind::Expr(Some(expr)) => {
                self.ensure_current_block_for_fallthrough();
                let _ = self.lower_expr_to_operand(expr);
            }
            TypedStmtKind::Expr(None) => {}
            TypedStmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.ensure_current_block_for_fallthrough();
                let cond_reg = self.lower_expr_to_condition(cond);
                let then_bb = self.alloc_block();
                let else_bb = self.alloc_block();
                let merge_bb = self.alloc_block();
                self.emit_current_terminator(Terminator::Branch {
                    cond: cond_reg.reg,
                    then_bb,
                    else_bb,
                });

                self.set_current_block(then_bb);
                self.lower_stmt(then_branch);
                if !self.current_function_mut().is_current_block_terminated() {
                    self.emit_current_terminator(Terminator::Jump(merge_bb));
                }

                self.set_current_block(else_bb);
                if let Some(else_branch) = else_branch {
                    self.lower_stmt(else_branch);
                }
                if !self.current_function_mut().is_current_block_terminated() {
                    self.emit_current_terminator(Terminator::Jump(merge_bb));
                }

                self.set_current_block(merge_bb);
            }
            TypedStmtKind::While { cond, body } => {
                self.ensure_current_block_for_fallthrough();
                let cond_bb = self.alloc_block();
                let body_bb = self.alloc_block();
                let exit_bb = self.alloc_block();
                self.emit_current_terminator(Terminator::Jump(cond_bb));

                self.set_current_block(cond_bb);
                let cond_reg = self.lower_expr_to_condition(cond);
                self.emit_current_terminator(Terminator::Branch {
                    cond: cond_reg.reg,
                    then_bb: body_bb,
                    else_bb: exit_bb,
                });

                self.push_loop_context(exit_bb, cond_bb);
                self.set_current_block(body_bb);
                self.lower_stmt(body);
                if !self.current_function_mut().is_current_block_terminated() {
                    self.emit_current_terminator(Terminator::Jump(cond_bb));
                }
                let _ = self.pop_loop_context();

                self.set_current_block(exit_bb);
            }
            TypedStmtKind::DoWhile { body, cond } => {
                self.ensure_current_block_for_fallthrough();
                let body_bb = self.alloc_block();
                let cond_bb = self.alloc_block();
                let exit_bb = self.alloc_block();
                self.emit_current_terminator(Terminator::Jump(body_bb));

                self.push_loop_context(exit_bb, cond_bb);
                self.set_current_block(body_bb);
                self.lower_stmt(body);
                if !self.current_function_mut().is_current_block_terminated() {
                    self.emit_current_terminator(Terminator::Jump(cond_bb));
                }

                self.set_current_block(cond_bb);
                let cond_reg = self.lower_expr_to_condition(cond);
                self.emit_current_terminator(Terminator::Branch {
                    cond: cond_reg.reg,
                    then_bb: body_bb,
                    else_bb: exit_bb,
                });
                let _ = self.pop_loop_context();

                self.set_current_block(exit_bb);
            }
            TypedStmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                self.ensure_current_block_for_fallthrough();
                if let Some(init) = init {
                    match init {
                        TypedForInit::Expr(expr) => {
                            let _ = self.lower_expr_to_operand(expr);
                        }
                        TypedForInit::Decl(decl) => self.lower_declaration_initializers(decl),
                    }
                }

                let cond_bb = self.alloc_block();
                let body_bb = self.alloc_block();
                let step_bb = self.alloc_block();
                let exit_bb = self.alloc_block();
                self.emit_current_terminator(Terminator::Jump(cond_bb));

                self.set_current_block(cond_bb);
                if let Some(cond_expr) = cond {
                    let cond_reg = self.lower_expr_to_condition(cond_expr);
                    self.emit_current_terminator(Terminator::Branch {
                        cond: cond_reg.reg,
                        then_bb: body_bb,
                        else_bb: exit_bb,
                    });
                } else {
                    self.emit_current_terminator(Terminator::Jump(body_bb));
                }

                self.push_loop_context(exit_bb, step_bb);
                self.set_current_block(body_bb);
                self.lower_stmt(body);
                if !self.current_function_mut().is_current_block_terminated() {
                    self.emit_current_terminator(Terminator::Jump(step_bb));
                }

                self.set_current_block(step_bb);
                if let Some(step_expr) = step {
                    let _ = self.lower_expr_to_operand(step_expr);
                }
                if !self.current_function_mut().is_current_block_terminated() {
                    self.emit_current_terminator(Terminator::Jump(cond_bb));
                }
                let _ = self.pop_loop_context();

                self.set_current_block(exit_bb);
            }
            TypedStmtKind::Switch { expr, body } => {
                self.ensure_current_block_for_fallthrough();
                let dispatch_bb = self.alloc_block();
                let body_bb = self.alloc_block();
                let merge_bb = self.alloc_block();
                let discr = self.lower_expr_to_vreg(expr, MirType::I32);
                self.emit_current_terminator(Terminator::Jump(dispatch_bb));

                self.push_switch_context_with_dispatch(merge_bb, dispatch_bb, None);
                self.set_current_block(body_bb);
                self.lower_stmt(body);
                if !self.current_function_mut().is_current_block_terminated() {
                    self.emit_current_terminator(Terminator::Jump(merge_bb));
                }
                let switch_ctx = self
                    .current_function_mut()
                    .switch_stack
                    .last()
                    .cloned()
                    .expect("switch context should exist");
                let _ = self.pop_switch_context();

                let cases = switch_ctx
                    .case_targets
                    .into_iter()
                    .map(|(value, target)| crate::mir::ir::SwitchCase { value, target })
                    .collect();
                let default = switch_ctx.default_target.unwrap_or(merge_bb);
                self.current_function_mut().emit_terminator_for_block(
                    switch_ctx.dispatch_block,
                    Terminator::Switch {
                        discr: discr.reg,
                        cases,
                        default,
                    },
                );
                self.set_current_block(merge_bb);
            }
            TypedStmtKind::Return(expr) => {
                self.ensure_current_block_for_fallthrough();
                if let Some(sret_slot) = self.current_function_mut().sret_slot() {
                    if let Some(ret_expr) = expr {
                        let dst_ptr = self.alloc_vreg(MirType::Ptr);
                        self.emit_current_instruction(Instruction::Load {
                            dst: dst_ptr,
                            slot: sret_slot,
                            offset: 0,
                            volatile: false,
                        });
                        let src_ptr = self
                            .lower_expr_address(ret_expr)
                            .unwrap_or_else(|| self.lower_expr_to_operand(ret_expr));
                        let size = self.type_size_u32(ret_expr.ty);
                        self.emit_current_instruction(Instruction::Memcpy {
                            dst_ptr: Operand::VReg(dst_ptr.reg),
                            src_ptr,
                            size,
                        });
                    }
                    self.emit_current_terminator(Terminator::Ret(None));
                } else {
                    let value = expr.as_ref().map(|expr| {
                        let raw = self.lower_expr_to_operand(expr);
                        let return_ty = self.current_function_mut().source_return_ty;
                        self.cast_operand_to_type(raw, expr.ty, return_ty)
                    });
                    self.emit_current_terminator(Terminator::Ret(value));
                }
            }
            TypedStmtKind::Break => {
                if let Some(target) = self.current_break_target() {
                    self.ensure_current_block_for_fallthrough();
                    self.emit_current_terminator(Terminator::Jump(target));
                }
            }
            TypedStmtKind::Continue => {
                if let Some(target) = self.current_continue_target() {
                    self.ensure_current_block_for_fallthrough();
                    self.emit_current_terminator(Terminator::Jump(target));
                }
            }
            TypedStmtKind::Goto(label) => {
                self.ensure_current_block_for_fallthrough();
                let target = self.ensure_label_block(*label);
                self.emit_current_terminator(Terminator::Jump(target));
            }
            TypedStmtKind::Label { label, stmt } => {
                let label_bb = self.ensure_label_block(*label);
                if !self.current_function_mut().is_current_block_terminated() {
                    self.emit_current_terminator(Terminator::Jump(label_bb));
                }
                self.set_current_block(label_bb);
                self.lower_stmt(stmt);
            }
            TypedStmtKind::Case { value, stmt } => {
                let case_value = match value {
                    CaseValue::Resolved(value) => *value,
                    CaseValue::Unresolved(_) => 0,
                };
                let case_bb = self.alloc_block();
                if !self.current_function_mut().is_current_block_terminated() {
                    self.emit_current_terminator(Terminator::Jump(case_bb));
                }
                if let Some(switch_ctx) = self.current_function_mut().switch_stack.last_mut() {
                    switch_ctx.case_targets.push((case_value, case_bb));
                }
                self.set_current_block(case_bb);
                self.lower_stmt(stmt);
            }
            TypedStmtKind::Default { stmt } => {
                let default_bb = self.alloc_block();
                if !self.current_function_mut().is_current_block_terminated() {
                    self.emit_current_terminator(Terminator::Jump(default_bb));
                }
                if let Some(switch_ctx) = self.current_function_mut().switch_stack.last_mut() {
                    switch_ctx.default_target = Some(default_bb);
                }
                self.set_current_block(default_bb);
                self.lower_stmt(stmt);
            }
        }
    }

    fn current_break_target(&mut self) -> Option<BlockId> {
        self.current_function_mut()
            .control_stack
            .last()
            .map(|ctx| match ctx {
                ControlContext::Loop { break_target } | ControlContext::Switch { break_target } => {
                    *break_target
                }
            })
    }

    fn current_continue_target(&mut self) -> Option<BlockId> {
        self.current_function_mut()
            .loop_stack
            .last()
            .map(|ctx| ctx.continue_target)
    }

    fn lower_expr_to_operand(&mut self, expr: &TypedExpr) -> Operand {
        self.lower_rvalue(expr)
    }

    fn lower_expr_to_vreg(&mut self, expr: &TypedExpr, fallback_ty: MirType) -> TypedVReg {
        let operand = self.lower_expr_to_operand(expr);
        let ty = match self.map_type(expr.ty) {
            MirType::Void => fallback_ty,
            mapped => mapped,
        };
        self.lower_operand_to_vreg(operand, ty)
    }

    fn lower_expr_to_condition(&mut self, expr: &TypedExpr) -> TypedVReg {
        let operand = self.lower_expr_to_operand(expr);
        self.lower_truthy_operand(expr.ty, operand)
    }

    fn lower_operand_to_vreg(&mut self, operand: Operand, ty: MirType) -> TypedVReg {
        match operand {
            Operand::VReg(reg) => TypedVReg { reg, ty },
            Operand::Const(value) => {
                let dst = self.alloc_vreg(ty);
                self.emit_current_instruction(Instruction::Copy {
                    dst,
                    src: Operand::Const(value),
                });
                dst
            }
            Operand::StackSlot(slot) => {
                let dst = self.alloc_vreg(ty);
                self.emit_current_instruction(Instruction::Load {
                    dst,
                    slot,
                    offset: 0,
                    volatile: false,
                });
                dst
            }
        }
    }

    fn lower_truthy_operand(&mut self, value_ty: TypeId, value: Operand) -> TypedVReg {
        let domain = if self.is_float_type(value_ty) {
            CmpDomain::Float
        } else if self.is_signed_integer(value_ty) {
            CmpDomain::Signed
        } else {
            CmpDomain::Unsigned
        };
        let cmp_ty = self.map_type(value_ty);
        let zero = self.zero_operand_for_type(value_ty);
        let dst = self.alloc_vreg(MirType::I8);
        self.emit_current_instruction(Instruction::Cmp {
            dst,
            kind: CmpKind::Ne,
            domain,
            lhs: value,
            rhs: zero,
            ty: cmp_ty,
        });
        dst
    }

    /// Lower an expression in rvalue context.
    ///
    /// Complex expressions are recursively lowered into MIR instructions and
    /// return either a vreg-backed operand or an immediate constant.
    fn lower_rvalue(&mut self, expr: &TypedExpr) -> Operand {
        if let Some(const_value) = expr.const_value {
            let can_lower_const = match const_value {
                ConstValue::Addr { .. } => {
                    // Address payloads from sema can also appear on scalar
                    // rvalues derived from lvalues (e.g. array indexing after
                    // implicit casts). Those must still be lowered via memory
                    // access, not as immediate pointer values.
                    self.map_type(expr.ty) == MirType::Ptr
                        && !matches!(expr.value_category, ValueCategory::LValue)
                }
                _ => true,
            };
            if can_lower_const && let Some(operand) = self.lower_const_operand(const_value) {
                return operand;
            }
        }

        match &expr.kind {
            TypedExprKind::Opaque => Operand::Const(MirConst::IntConst(0)),
            TypedExprKind::Literal(value) => Operand::Const(self.lower_const(*value)),
            TypedExprKind::StringLiteral(text) => {
                let place = self.string_literal_place(text);
                self.address_of_place(place)
            }
            TypedExprKind::SymbolRef(symbol_id) => self.lower_symbol_ref(*symbol_id, expr),
            TypedExprKind::Unary { op, operand } => self.lower_unary_rvalue(*op, operand, expr.ty),
            TypedExprKind::Binary { op, left, right } => {
                self.lower_binary_rvalue(*op, left, right, expr.ty)
            }
            TypedExprKind::Assign { op, lhs, rhs } => self.lower_assign_rvalue(*op, lhs, rhs),
            TypedExprKind::Conditional {
                cond,
                then_expr,
                else_expr,
            } => self.lower_conditional_rvalue(cond, then_expr, else_expr, expr.ty),
            TypedExprKind::Call { func, args } => self.lower_call_rvalue(func, args, expr.ty),
            TypedExprKind::Index { .. } | TypedExprKind::MemberAccess { .. } => {
                if let Some(place) = self.lower_place(expr) {
                    if matches!(
                        expr.value_category,
                        ValueCategory::ArrayDesignator | ValueCategory::FunctionDesignator
                    ) {
                        self.address_of_place(place)
                    } else {
                        self.load_from_place(place, expr.ty)
                    }
                } else {
                    self.lowering_bug("missing lvalue place for indexed/member rvalue", expr)
                }
            }
            TypedExprKind::Cast { expr: inner, to }
            | TypedExprKind::ImplicitCast { expr: inner, to } => {
                self.lower_cast_rvalue(inner, inner.ty, *to)
            }
            TypedExprKind::SizeofType { ty } => {
                let size = type_size_of(*ty, &self.sema.types, &self.sema.records).unwrap_or(0);
                Operand::Const(MirConst::IntConst(i64::try_from(size).unwrap_or(i64::MAX)))
            }
            TypedExprKind::SizeofExpr { expr: inner } => {
                let size =
                    type_size_of(inner.ty, &self.sema.types, &self.sema.records).unwrap_or(0);
                Operand::Const(MirConst::IntConst(i64::try_from(size).unwrap_or(i64::MAX)))
            }
            TypedExprKind::Comma { left, right } => {
                let _ = self.lower_expr_to_operand(left);
                self.lower_expr_to_operand(right)
            }
            TypedExprKind::CompoundLiteral {
                ty,
                init,
                is_file_scope,
            } => {
                let place = self.materialize_compound_literal_place(
                    *ty,
                    init,
                    *is_file_scope,
                    (expr.span.start, expr.span.end),
                );
                if matches!(
                    expr.value_category,
                    ValueCategory::ArrayDesignator | ValueCategory::FunctionDesignator
                ) {
                    self.address_of_place(place)
                } else {
                    self.load_from_place(place, expr.ty)
                }
            }
        }
    }

    fn lower_const_operand(&mut self, value: ConstValue) -> Option<Operand> {
        match value {
            ConstValue::Addr { symbol, offset } => Some(self.lower_symbol_address(symbol, offset)),
            other => Some(Operand::Const(self.lower_const(other))),
        }
    }

    fn lower_symbol_ref(&mut self, symbol_id: SymbolId, expr: &TypedExpr) -> Operand {
        if matches!(
            expr.value_category,
            ValueCategory::ArrayDesignator | ValueCategory::FunctionDesignator
        ) {
            return self.lower_symbol_address(symbol_id, 0);
        }

        let symbol_kind = self.symbol(symbol_id).kind();
        if symbol_kind == SymbolKind::Function {
            return self.lower_symbol_address(symbol_id, 0);
        }

        if let Some(place) = self.lower_symbol_place(symbol_id) {
            return self.load_from_place(place, expr.ty);
        }

        self.lowering_bug("missing storage for symbol reference", expr)
    }

    fn lower_unary_rvalue(
        &mut self,
        op: TypedUnaryOp,
        operand: &TypedExpr,
        result_ty: TypeId,
    ) -> Operand {
        match op {
            TypedUnaryOp::Plus => self.lower_expr_to_operand(operand),
            TypedUnaryOp::Minus => {
                let value = self.lower_expr_to_operand(operand);
                let ty = self.map_type(result_ty);
                let dst = self.alloc_vreg(ty);
                self.emit_current_instruction(Instruction::Unary {
                    dst,
                    op: MirUnaryOp::Neg,
                    operand: value,
                    ty,
                });
                Operand::VReg(dst.reg)
            }
            TypedUnaryOp::LogicalNot => {
                let cond = self.lower_expr_to_condition(operand);
                let cmp_dst = self.alloc_vreg(MirType::I8);
                self.emit_current_instruction(Instruction::Cmp {
                    dst: cmp_dst,
                    kind: CmpKind::Eq,
                    domain: CmpDomain::Unsigned,
                    lhs: Operand::VReg(cond.reg),
                    rhs: Operand::Const(MirConst::IntConst(0)),
                    ty: MirType::I8,
                });
                let result_mir_ty = self.map_type(result_ty);
                self.cast_operand_between_mir(
                    Operand::VReg(cmp_dst.reg),
                    MirType::I8,
                    false,
                    result_mir_ty,
                    false,
                )
            }
            TypedUnaryOp::BitwiseNot => {
                let value = self.lower_expr_to_operand(operand);
                let ty = self.map_type(result_ty);
                let dst = self.alloc_vreg(ty);
                self.emit_current_instruction(Instruction::Unary {
                    dst,
                    op: MirUnaryOp::Not,
                    operand: value,
                    ty,
                });
                Operand::VReg(dst.reg)
            }
            TypedUnaryOp::AddrOf => self.lower_expr_address(operand).unwrap_or_else(|| {
                self.lowering_bug("failed to compute address-of operand", operand)
            }),
            TypedUnaryOp::Deref => {
                if let Some(place) = self.lower_place(&TypedExpr {
                    kind: TypedExprKind::Unary {
                        op: TypedUnaryOp::Deref,
                        operand: Box::new(operand.clone()),
                    },
                    ty: result_ty,
                    value_category: ValueCategory::LValue,
                    const_value: None,
                    span: operand.span,
                }) {
                    self.load_from_place(place, result_ty)
                } else {
                    self.lowering_bug("failed to lower dereference place", operand)
                }
            }
            TypedUnaryOp::PreInc
            | TypedUnaryOp::PreDec
            | TypedUnaryOp::PostInc
            | TypedUnaryOp::PostDec => {
                let Some(place) = self.lower_place(operand) else {
                    self.lowering_bug("failed to lower increment/decrement place", operand);
                };
                let old_value = self.load_from_place(place.clone(), operand.ty);
                let new_value = if self.is_pointer_type(operand.ty) {
                    let one = Operand::Const(MirConst::IntConst(1));
                    let pointee = self.pointee_type(operand.ty).unwrap_or(operand.ty);
                    let byte_offset = self.scale_index_operand(one, MirType::I32, true, pointee);
                    let signed_delta = matches!(op, TypedUnaryOp::PreDec | TypedUnaryOp::PostDec);
                    let delta = if signed_delta {
                        self.negate_integer_operand(byte_offset, MirType::I64)
                    } else {
                        byte_offset
                    };
                    self.emit_ptr_add(old_value.clone(), delta)
                } else {
                    let ty = self.map_type(operand.ty);
                    let one = if ty.is_float() {
                        Operand::Const(MirConst::FloatConst(1.0))
                    } else {
                        Operand::Const(MirConst::IntConst(1))
                    };
                    let dst = self.alloc_vreg(ty);
                    self.emit_current_instruction(Instruction::Binary {
                        dst,
                        op: match (op, ty.is_float()) {
                            (TypedUnaryOp::PreDec | TypedUnaryOp::PostDec, true) => BinaryOp::FSub,
                            (TypedUnaryOp::PreDec | TypedUnaryOp::PostDec, false) => BinaryOp::Sub,
                            (_, true) => BinaryOp::FAdd,
                            (_, false) => BinaryOp::Add,
                        },
                        lhs: old_value.clone(),
                        rhs: one,
                        ty,
                    });
                    Operand::VReg(dst.reg)
                };
                self.store_to_place(place, new_value.clone(), operand.ty);
                if matches!(op, TypedUnaryOp::PostInc | TypedUnaryOp::PostDec) {
                    old_value
                } else {
                    new_value
                }
            }
        }
    }

    fn lower_binary_rvalue(
        &mut self,
        op: TypedBinaryOp,
        left: &TypedExpr,
        right: &TypedExpr,
        result_ty: TypeId,
    ) -> Operand {
        match op {
            TypedBinaryOp::LogicalAnd => self.lower_logical_and(left, right, result_ty),
            TypedBinaryOp::LogicalOr => self.lower_logical_or(left, right, result_ty),
            TypedBinaryOp::Eq
            | TypedBinaryOp::Ne
            | TypedBinaryOp::Lt
            | TypedBinaryOp::Le
            | TypedBinaryOp::Gt
            | TypedBinaryOp::Ge => self.lower_comparison(op, left, right, result_ty),
            TypedBinaryOp::Add | TypedBinaryOp::Sub => {
                if self.is_pointer_type(left.ty) && self.is_integer_type(right.ty) {
                    let add = op == TypedBinaryOp::Add;
                    return self.lower_pointer_add_sub(left, right, add, result_ty);
                }
                if self.is_pointer_type(right.ty) && self.is_integer_type(left.ty) {
                    let add = op == TypedBinaryOp::Add;
                    return self.lower_pointer_add_sub(right, left, add, result_ty);
                }
                if op == TypedBinaryOp::Sub
                    && self.is_pointer_type(left.ty)
                    && self.is_pointer_type(right.ty)
                {
                    return self.lower_pointer_subtract(left, right, result_ty);
                }
                self.lower_binary_arithmetic(op, left, right, result_ty)
            }
            TypedBinaryOp::Mul
            | TypedBinaryOp::Div
            | TypedBinaryOp::Mod
            | TypedBinaryOp::BitwiseAnd
            | TypedBinaryOp::BitwiseOr
            | TypedBinaryOp::BitwiseXor
            | TypedBinaryOp::Shl
            | TypedBinaryOp::Shr => self.lower_binary_arithmetic(op, left, right, result_ty),
        }
    }

    fn lower_assign_rvalue(
        &mut self,
        op: TypedAssignOp,
        lhs: &TypedExpr,
        rhs: &TypedExpr,
    ) -> Operand {
        let Some(place) = self.lower_place(lhs) else {
            self.lowering_bug("failed to lower assignment destination", lhs);
        };

        if self.is_aggregate_type(lhs.ty) {
            self.copy_aggregate_into_place(place.clone(), lhs.ty, rhs);
            return self.load_from_place(place, lhs.ty);
        }

        if op == TypedAssignOp::Assign {
            let rhs_raw = self.lower_expr_to_operand(rhs);
            let rhs_value = self.cast_operand_to_type(rhs_raw, rhs.ty, lhs.ty);
            self.store_to_place(place, rhs_value.clone(), lhs.ty);
            return rhs_value;
        }

        let lhs_value = self.load_from_place(place.clone(), lhs.ty);
        let rhs_value = self.lower_expr_to_operand(rhs);
        let updated = self.lower_compound_assignment_value(op, lhs, rhs, lhs_value, rhs_value);
        let stored = self.cast_operand_to_type(updated, lhs.ty, lhs.ty);
        self.store_to_place(place, stored.clone(), lhs.ty);
        stored
    }

    fn lower_compound_assignment_value(
        &mut self,
        op: TypedAssignOp,
        lhs_expr: &TypedExpr,
        rhs_expr: &TypedExpr,
        lhs_value: Operand,
        rhs_value: Operand,
    ) -> Operand {
        match op {
            TypedAssignOp::AddAssign | TypedAssignOp::SubAssign
                if self.is_pointer_type(lhs_expr.ty) && self.is_integer_type(rhs_expr.ty) =>
            {
                let pointee = self.pointee_type(lhs_expr.ty).unwrap_or(lhs_expr.ty);
                let rhs_mir_ty = self.map_type(rhs_expr.ty);
                let mut byte_offset =
                    self.scale_index_operand(rhs_value, rhs_mir_ty, true, pointee);
                if op == TypedAssignOp::SubAssign {
                    byte_offset = self.negate_integer_operand(byte_offset, MirType::I64);
                }
                self.emit_ptr_add(lhs_value, byte_offset)
            }
            _ => {
                let result_ty = lhs_expr.ty;
                let mir_ty = self.map_type(result_ty);
                let dst = self.alloc_vreg(mir_ty);
                let binary_op = match op {
                    TypedAssignOp::AddAssign => {
                        if mir_ty.is_float() {
                            BinaryOp::FAdd
                        } else {
                            BinaryOp::Add
                        }
                    }
                    TypedAssignOp::SubAssign => {
                        if mir_ty.is_float() {
                            BinaryOp::FSub
                        } else {
                            BinaryOp::Sub
                        }
                    }
                    TypedAssignOp::MulAssign => {
                        if mir_ty.is_float() {
                            BinaryOp::FMul
                        } else {
                            BinaryOp::Mul
                        }
                    }
                    TypedAssignOp::DivAssign => {
                        if mir_ty.is_float() {
                            BinaryOp::FDiv
                        } else if self.is_signed_integer(result_ty) {
                            BinaryOp::SDiv
                        } else {
                            BinaryOp::UDiv
                        }
                    }
                    TypedAssignOp::ModAssign => {
                        if mir_ty.is_float() {
                            BinaryOp::FRem
                        } else if self.is_signed_integer(result_ty) {
                            BinaryOp::SRem
                        } else {
                            BinaryOp::URem
                        }
                    }
                    TypedAssignOp::AndAssign => BinaryOp::And,
                    TypedAssignOp::OrAssign => BinaryOp::Or,
                    TypedAssignOp::XorAssign => BinaryOp::Xor,
                    TypedAssignOp::ShlAssign => BinaryOp::Shl,
                    TypedAssignOp::ShrAssign => {
                        if self.is_signed_integer(result_ty) {
                            BinaryOp::AShr
                        } else {
                            BinaryOp::LShr
                        }
                    }
                    TypedAssignOp::Assign => BinaryOp::Add,
                };
                let rhs_casted = self.cast_operand_to_type(rhs_value, rhs_expr.ty, lhs_expr.ty);
                self.emit_current_instruction(Instruction::Binary {
                    dst,
                    op: binary_op,
                    lhs: lhs_value,
                    rhs: rhs_casted,
                    ty: mir_ty,
                });
                Operand::VReg(dst.reg)
            }
        }
    }

    fn lower_binary_arithmetic(
        &mut self,
        op: TypedBinaryOp,
        left: &TypedExpr,
        right: &TypedExpr,
        result_ty: TypeId,
    ) -> Operand {
        let lhs = self.lower_expr_to_operand(left);
        let rhs = self.lower_expr_to_operand(right);
        let ty = self.map_type(result_ty);
        let dst = self.alloc_vreg(ty);
        let binary_op = match op {
            TypedBinaryOp::Add => {
                if ty.is_float() {
                    BinaryOp::FAdd
                } else {
                    BinaryOp::Add
                }
            }
            TypedBinaryOp::Sub => {
                if ty.is_float() {
                    BinaryOp::FSub
                } else {
                    BinaryOp::Sub
                }
            }
            TypedBinaryOp::Mul => {
                if ty.is_float() {
                    BinaryOp::FMul
                } else {
                    BinaryOp::Mul
                }
            }
            TypedBinaryOp::Div => {
                if ty.is_float() {
                    BinaryOp::FDiv
                } else if self.is_signed_integer(result_ty) {
                    BinaryOp::SDiv
                } else {
                    BinaryOp::UDiv
                }
            }
            TypedBinaryOp::Mod => {
                if ty.is_float() {
                    BinaryOp::FRem
                } else if self.is_signed_integer(result_ty) {
                    BinaryOp::SRem
                } else {
                    BinaryOp::URem
                }
            }
            TypedBinaryOp::BitwiseAnd => BinaryOp::And,
            TypedBinaryOp::BitwiseOr => BinaryOp::Or,
            TypedBinaryOp::BitwiseXor => BinaryOp::Xor,
            TypedBinaryOp::Shl => BinaryOp::Shl,
            TypedBinaryOp::Shr => {
                if self.is_signed_integer(left.ty) {
                    BinaryOp::AShr
                } else {
                    BinaryOp::LShr
                }
            }
            _ => BinaryOp::Add,
        };
        self.emit_current_instruction(Instruction::Binary {
            dst,
            op: binary_op,
            lhs,
            rhs,
            ty,
        });
        Operand::VReg(dst.reg)
    }

    fn lower_comparison(
        &mut self,
        op: TypedBinaryOp,
        left: &TypedExpr,
        right: &TypedExpr,
        result_ty: TypeId,
    ) -> Operand {
        let lhs = self.lower_expr_to_operand(left);
        let rhs = self.lower_expr_to_operand(right);
        let cmp_ty = self.map_type(left.ty);
        let domain = if self.is_float_type(left.ty) {
            CmpDomain::Float
        } else if self.is_signed_integer(left.ty) {
            CmpDomain::Signed
        } else {
            CmpDomain::Unsigned
        };
        let kind = match op {
            TypedBinaryOp::Eq => CmpKind::Eq,
            TypedBinaryOp::Ne => CmpKind::Ne,
            TypedBinaryOp::Lt => CmpKind::Lt,
            TypedBinaryOp::Le => CmpKind::Le,
            TypedBinaryOp::Gt => CmpKind::Gt,
            TypedBinaryOp::Ge => CmpKind::Ge,
            _ => CmpKind::Eq,
        };
        let cmp_dst = self.alloc_vreg(MirType::I8);
        self.emit_current_instruction(Instruction::Cmp {
            dst: cmp_dst,
            kind,
            domain,
            lhs,
            rhs,
            ty: cmp_ty,
        });
        let result_mir_ty = self.map_type(result_ty);
        self.cast_operand_between_mir(
            Operand::VReg(cmp_dst.reg),
            MirType::I8,
            false,
            result_mir_ty,
            false,
        )
    }

    fn lower_logical_and(
        &mut self,
        left: &TypedExpr,
        right: &TypedExpr,
        result_ty: TypeId,
    ) -> Operand {
        self.ensure_current_block_for_fallthrough();
        let (slot_size, slot_align) = self.stack_layout_of(result_ty);
        let result_slot = self.alloc_slot(slot_size, slot_align);
        let result_mir_ty = self.map_type(result_ty);
        let rhs_bb = self.alloc_block();
        let false_bb = self.alloc_block();
        let merge_bb = self.alloc_block();

        let lhs_cond = self.lower_expr_to_condition(left);
        self.emit_current_terminator(Terminator::Branch {
            cond: lhs_cond.reg,
            then_bb: rhs_bb,
            else_bb: false_bb,
        });

        self.set_current_block(false_bb);
        let false_val = self.cast_operand_between_mir(
            Operand::Const(MirConst::IntConst(0)),
            MirType::I8,
            false,
            result_mir_ty,
            false,
        );
        self.emit_current_instruction(Instruction::Store {
            slot: result_slot,
            offset: 0,
            value: false_val,
            ty: result_mir_ty,
            volatile: false,
        });
        self.emit_current_terminator(Terminator::Jump(merge_bb));

        self.set_current_block(rhs_bb);
        let rhs_cond = self.lower_expr_to_condition(right);
        let rhs_val = self.cast_operand_between_mir(
            Operand::VReg(rhs_cond.reg),
            MirType::I8,
            false,
            result_mir_ty,
            false,
        );
        self.emit_current_instruction(Instruction::Store {
            slot: result_slot,
            offset: 0,
            value: rhs_val,
            ty: result_mir_ty,
            volatile: false,
        });
        self.emit_current_terminator(Terminator::Jump(merge_bb));

        self.set_current_block(merge_bb);
        let dst = self.alloc_vreg(result_mir_ty);
        self.emit_current_instruction(Instruction::Load {
            dst,
            slot: result_slot,
            offset: 0,
            volatile: false,
        });
        Operand::VReg(dst.reg)
    }

    fn lower_logical_or(
        &mut self,
        left: &TypedExpr,
        right: &TypedExpr,
        result_ty: TypeId,
    ) -> Operand {
        self.ensure_current_block_for_fallthrough();
        let (slot_size, slot_align) = self.stack_layout_of(result_ty);
        let result_slot = self.alloc_slot(slot_size, slot_align);
        let result_mir_ty = self.map_type(result_ty);
        let true_bb = self.alloc_block();
        let rhs_bb = self.alloc_block();
        let merge_bb = self.alloc_block();

        let lhs_cond = self.lower_expr_to_condition(left);
        self.emit_current_terminator(Terminator::Branch {
            cond: lhs_cond.reg,
            then_bb: true_bb,
            else_bb: rhs_bb,
        });

        self.set_current_block(true_bb);
        let true_val = self.cast_operand_between_mir(
            Operand::Const(MirConst::IntConst(1)),
            MirType::I8,
            false,
            result_mir_ty,
            false,
        );
        self.emit_current_instruction(Instruction::Store {
            slot: result_slot,
            offset: 0,
            value: true_val,
            ty: result_mir_ty,
            volatile: false,
        });
        self.emit_current_terminator(Terminator::Jump(merge_bb));

        self.set_current_block(rhs_bb);
        let rhs_cond = self.lower_expr_to_condition(right);
        let rhs_val = self.cast_operand_between_mir(
            Operand::VReg(rhs_cond.reg),
            MirType::I8,
            false,
            result_mir_ty,
            false,
        );
        self.emit_current_instruction(Instruction::Store {
            slot: result_slot,
            offset: 0,
            value: rhs_val,
            ty: result_mir_ty,
            volatile: false,
        });
        self.emit_current_terminator(Terminator::Jump(merge_bb));

        self.set_current_block(merge_bb);
        let dst = self.alloc_vreg(result_mir_ty);
        self.emit_current_instruction(Instruction::Load {
            dst,
            slot: result_slot,
            offset: 0,
            volatile: false,
        });
        Operand::VReg(dst.reg)
    }

    fn lower_conditional_rvalue(
        &mut self,
        cond: &TypedExpr,
        then_expr: &TypedExpr,
        else_expr: &TypedExpr,
        result_ty: TypeId,
    ) -> Operand {
        self.ensure_current_block_for_fallthrough();
        let cond_vreg = self.lower_expr_to_condition(cond);
        let then_bb = self.alloc_block();
        let else_bb = self.alloc_block();
        let merge_bb = self.alloc_block();
        self.emit_current_terminator(Terminator::Branch {
            cond: cond_vreg.reg,
            then_bb,
            else_bb,
        });

        if self.is_aggregate_type(result_ty) {
            let (size, align) = self.stack_layout_of(result_ty);
            let result_slot = self.alloc_slot(size, align);
            let result_place = MirPlace::Stack {
                slot: result_slot,
                offset: 0,
            };

            self.set_current_block(then_bb);
            self.copy_aggregate_into_place(result_place.clone(), result_ty, then_expr);
            if !self.current_function_mut().is_current_block_terminated() {
                self.emit_current_terminator(Terminator::Jump(merge_bb));
            }

            self.set_current_block(else_bb);
            self.copy_aggregate_into_place(result_place.clone(), result_ty, else_expr);
            if !self.current_function_mut().is_current_block_terminated() {
                self.emit_current_terminator(Terminator::Jump(merge_bb));
            }

            self.set_current_block(merge_bb);
            return self.address_of_place(result_place);
        }

        let result_mir_ty = self.map_type(result_ty);
        let use_result_slot = result_mir_ty != MirType::Void;
        let result_slot = if use_result_slot {
            let (size, align) = self.stack_layout_of(result_ty);
            Some(self.alloc_slot(size, align))
        } else {
            None
        };

        self.set_current_block(then_bb);
        if let Some(slot) = result_slot {
            let val = self.lower_expr_to_operand(then_expr);
            self.emit_current_instruction(Instruction::Store {
                slot,
                offset: 0,
                value: val,
                ty: result_mir_ty,
                volatile: false,
            });
        } else {
            let _ = self.lower_expr_to_operand(then_expr);
        }
        if !self.current_function_mut().is_current_block_terminated() {
            self.emit_current_terminator(Terminator::Jump(merge_bb));
        }

        self.set_current_block(else_bb);
        if let Some(slot) = result_slot {
            let val = self.lower_expr_to_operand(else_expr);
            self.emit_current_instruction(Instruction::Store {
                slot,
                offset: 0,
                value: val,
                ty: result_mir_ty,
                volatile: false,
            });
        } else {
            let _ = self.lower_expr_to_operand(else_expr);
        }
        if !self.current_function_mut().is_current_block_terminated() {
            self.emit_current_terminator(Terminator::Jump(merge_bb));
        }

        self.set_current_block(merge_bb);
        if let Some(slot) = result_slot {
            let dst = self.alloc_vreg(result_mir_ty);
            self.emit_current_instruction(Instruction::Load {
                dst,
                slot,
                offset: 0,
                volatile: false,
            });
            Operand::VReg(dst.reg)
        } else {
            Operand::Const(MirConst::IntConst(0))
        }
    }

    /// Lower a function call and apply MIR-level canonical ABI rewriting.
    ///
    /// Aggregate by-value arguments are materialized in temporary stack slots
    /// and passed as pointers. Aggregate returns are lowered as hidden `sret`
    /// first argument and void call result.
    fn lower_call_rvalue(
        &mut self,
        func: &TypedExpr,
        args: &[TypedExpr],
        result_ty: TypeId,
    ) -> Operand {
        let function_ty = self.extract_function_type(func.ty);
        let function_sig = self.map_function_sig(func.ty);
        let boundary_sig = self.map_boundary_sig(func.ty);
        let function_param_tys = function_ty.as_ref().map(|fn_ty| fn_ty.params.clone());
        let function_ret_ty = function_ty.as_ref().map(|fn_ty| fn_ty.ret);
        let function_variadic = function_ty
            .as_ref()
            .map(|fn_ty| fn_ty.variadic)
            .unwrap_or(false);
        let fixed_param_count = function_param_tys.as_ref().map_or(0, std::vec::Vec::len);
        let mut lowered_args = Vec::with_capacity(args.len() + 1);
        for (idx, arg) in args.iter().enumerate() {
            let param_ty = function_param_tys
                .as_ref()
                .and_then(|params| params.get(idx))
                .copied();
            let aggregate_arg_ty = param_ty
                .filter(|ty| self.is_aggregate_type(*ty))
                .or_else(|| self.is_aggregate_type(arg.ty).then_some(arg.ty));

            if let Some(agg_ty) = aggregate_arg_ty {
                // Canonical ABI: aggregate by-value arguments are materialized
                // into a temporary stack slot and passed by address.
                let src_ptr = self
                    .lower_expr_address(arg)
                    .unwrap_or_else(|| self.lower_expr_to_operand(arg));
                let (size, alignment) = self.stack_layout_of(agg_ty);
                let abi_size = abi_struct_stack_size(size);
                let slot = self.alloc_slot(abi_size, alignment.max(8));
                let dst_ptr = self.address_of_place(MirPlace::Stack { slot, offset: 0 });
                self.emit_current_instruction(Instruction::Memcpy {
                    dst_ptr: dst_ptr.clone(),
                    src_ptr,
                    size,
                });
                lowered_args.push(dst_ptr);
            } else {
                let mut lowered = self.lower_expr_to_operand(arg);
                if function_variadic && idx >= fixed_param_count {
                    lowered = self.promote_variadic_arg(arg, lowered);
                    let promoted_ty = self.variadic_promoted_mir_type(arg.ty);
                    let promoted = self.lower_operand_to_vreg(lowered, promoted_ty);
                    lowered = Operand::VReg(promoted.reg);
                }
                lowered_args.push(lowered);
            }
        }

        let mut fixed_arg_count = function_variadic.then_some(fixed_param_count);
        let aggregate_ret_ty = function_ret_ty.unwrap_or(result_ty);
        let returns_aggregate = self.is_aggregate_type(aggregate_ret_ty);
        let result_mir_ty = self.map_type(result_ty);
        let mut aggregate_ret_addr = None;
        let dst = if returns_aggregate {
            let (size, alignment) = self.stack_layout_of(aggregate_ret_ty);
            let slot = self.alloc_slot(size, alignment);
            let sret_addr = self.address_of_place(MirPlace::Stack { slot, offset: 0 });
            lowered_args.insert(0, sret_addr.clone());
            if let Some(count) = &mut fixed_arg_count {
                *count += 1;
            }
            aggregate_ret_addr = Some(sret_addr);
            None
        } else if result_mir_ty == MirType::Void {
            None
        } else {
            Some(self.alloc_vreg(result_mir_ty))
        };

        if let Some(callee_name) = self.try_get_direct_callee_name(func) {
            self.emit_current_instruction(Instruction::Call {
                dst,
                callee: callee_name,
                args: lowered_args,
                fixed_arg_count,
            });
        } else {
            let callee_ptr = self.lower_expr_to_operand(func);
            let sig = function_sig.unwrap_or(MirFunctionSig {
                params: {
                    let mut params =
                        Vec::with_capacity(args.len() + usize::from(returns_aggregate));
                    if returns_aggregate {
                        params.push(MirAbiParam::struct_return());
                    }
                    for (idx, arg) in args.iter().enumerate() {
                        let param_ty = function_param_tys
                            .as_ref()
                            .and_then(|tys| tys.get(idx))
                            .copied();
                        if param_ty
                            .map(|ty| self.is_aggregate_type(ty))
                            .unwrap_or_else(|| self.is_aggregate_type(arg.ty))
                        {
                            let agg_ty = param_ty.unwrap_or(arg.ty);
                            let (size, _) = self.stack_layout_of(agg_ty);
                            params.push(MirAbiParam::struct_argument(abi_struct_stack_size(size)));
                        } else {
                            params
                                .push(MirAbiParam::new(self.map_type(param_ty.unwrap_or(arg.ty))));
                        }
                    }
                    params
                },
                return_type: if returns_aggregate {
                    MirType::Void
                } else {
                    result_mir_ty
                },
                variadic: false,
            });
            self.emit_current_instruction(Instruction::CallIndirect {
                dst,
                callee_ptr,
                args: lowered_args,
                sig,
                boundary_sig,
                fixed_arg_count,
            });
        }

        if let Some(addr) = aggregate_ret_addr {
            addr
        } else if let Some(dst) = dst {
            Operand::VReg(dst.reg)
        } else {
            Operand::Const(MirConst::IntConst(0))
        }
    }

    fn variadic_promoted_mir_type(&mut self, ty: TypeId) -> MirType {
        match self.map_type(ty) {
            MirType::I8 | MirType::I16 => MirType::I32,
            MirType::F32 => MirType::F64,
            other => other,
        }
    }

    fn promote_variadic_arg(&mut self, arg: &TypedExpr, value: Operand) -> Operand {
        let from_mir = self.map_type(arg.ty);
        let to_mir = self.variadic_promoted_mir_type(arg.ty);
        let from_signed = self.is_signed_integer(arg.ty);
        self.cast_operand_between_mir(value, from_mir, from_signed, to_mir, false)
    }

    fn try_get_direct_callee_name(&self, expr: &TypedExpr) -> Option<String> {
        match &expr.kind {
            TypedExprKind::SymbolRef(symbol_id) => {
                let symbol = self.symbol(*symbol_id);
                (symbol.kind() == SymbolKind::Function).then(|| symbol.name().to_string())
            }
            TypedExprKind::ImplicitCast { expr, .. } | TypedExprKind::Cast { expr, .. } => {
                self.try_get_direct_callee_name(expr)
            }
            _ => None,
        }
    }

    fn lower_cast_rvalue(&mut self, inner: &TypedExpr, from_ty: TypeId, to_ty: TypeId) -> Operand {
        if matches!(self.sema.types.get(to_ty).kind, TypeKind::Void) {
            let _ = self.lower_expr_to_operand(inner);
            return Operand::Const(MirConst::IntConst(0));
        }

        if matches!(
            inner.value_category,
            ValueCategory::ArrayDesignator | ValueCategory::FunctionDesignator
        ) && self.is_pointer_type(to_ty)
            && let Some(addr) = self.lower_expr_address(inner)
        {
            return self.cast_operand_to_type(addr, from_ty, to_ty);
        }

        let src = self.lower_expr_to_operand(inner);
        self.cast_operand_to_type(src, from_ty, to_ty)
    }

    fn lower_pointer_add_sub(
        &mut self,
        ptr_expr: &TypedExpr,
        int_expr: &TypedExpr,
        add: bool,
        _result_ty: TypeId,
    ) -> Operand {
        let base_ptr = self.lower_expr_to_operand(ptr_expr);
        let index = self.lower_expr_to_operand(int_expr);
        let pointee = self.pointee_type(ptr_expr.ty).unwrap_or(ptr_expr.ty);
        let index_mir_ty = self.map_type(int_expr.ty);
        let index_signed = self.is_signed_integer(int_expr.ty);
        let mut byte_offset = self.scale_index_operand(index, index_mir_ty, index_signed, pointee);
        if !add {
            byte_offset = self.negate_integer_operand(byte_offset, MirType::I64);
        }
        self.emit_ptr_add(base_ptr, byte_offset)
    }

    fn lower_pointer_subtract(
        &mut self,
        left: &TypedExpr,
        right: &TypedExpr,
        result_ty: TypeId,
    ) -> Operand {
        let lhs_ptr = self.lower_expr_to_operand(left);
        let rhs_ptr = self.lower_expr_to_operand(right);
        let lhs_i64 =
            self.cast_operand_between_mir(lhs_ptr, MirType::Ptr, false, MirType::I64, false);
        let rhs_i64 =
            self.cast_operand_between_mir(rhs_ptr, MirType::Ptr, false, MirType::I64, false);
        let diff_bytes_dst = self.alloc_vreg(MirType::I64);
        self.emit_current_instruction(Instruction::Binary {
            dst: diff_bytes_dst,
            op: BinaryOp::Sub,
            lhs: lhs_i64,
            rhs: rhs_i64,
            ty: MirType::I64,
        });
        let pointee = self.pointee_type(left.ty).unwrap_or(left.ty);
        let elem_size = type_size_of(pointee, &self.sema.types, &self.sema.records)
            .and_then(|n| i64::try_from(n).ok())
            .unwrap_or(1);
        let diff_elems = if elem_size != 1 {
            let div_dst = self.alloc_vreg(MirType::I64);
            self.emit_current_instruction(Instruction::Binary {
                dst: div_dst,
                op: BinaryOp::SDiv,
                lhs: Operand::VReg(diff_bytes_dst.reg),
                rhs: Operand::Const(MirConst::IntConst(elem_size)),
                ty: MirType::I64,
            });
            Operand::VReg(div_dst.reg)
        } else {
            Operand::VReg(diff_bytes_dst.reg)
        };
        let result_mir_ty = self.map_type(result_ty);
        self.cast_operand_between_mir(diff_elems, MirType::I64, true, result_mir_ty, false)
    }

    fn scale_index_operand(
        &mut self,
        index: Operand,
        index_mir_ty: MirType,
        index_signed: bool,
        pointee: TypeId,
    ) -> Operand {
        let idx64 =
            self.cast_operand_between_mir(index, index_mir_ty, index_signed, MirType::I64, false);
        let elem_size = type_size_of(pointee, &self.sema.types, &self.sema.records)
            .and_then(|n| i64::try_from(n).ok())
            .unwrap_or(1);
        if elem_size == 1 {
            return idx64;
        }
        let dst = self.alloc_vreg(MirType::I64);
        self.emit_current_instruction(Instruction::Binary {
            dst,
            op: BinaryOp::Mul,
            lhs: idx64,
            rhs: Operand::Const(MirConst::IntConst(elem_size)),
            ty: MirType::I64,
        });
        Operand::VReg(dst.reg)
    }

    fn negate_integer_operand(&mut self, operand: Operand, ty: MirType) -> Operand {
        let dst = self.alloc_vreg(ty);
        self.emit_current_instruction(Instruction::Unary {
            dst,
            op: MirUnaryOp::Neg,
            operand,
            ty,
        });
        Operand::VReg(dst.reg)
    }

    fn lower_place(&mut self, expr: &TypedExpr) -> Option<MirPlace> {
        match &expr.kind {
            TypedExprKind::SymbolRef(symbol_id) => self.lower_symbol_place(*symbol_id),
            TypedExprKind::CompoundLiteral {
                ty,
                init,
                is_file_scope,
            } => Some(self.materialize_compound_literal_place(
                *ty,
                init,
                *is_file_scope,
                (expr.span.start, expr.span.end),
            )),
            TypedExprKind::Unary {
                op: TypedUnaryOp::Deref,
                operand,
            } => {
                let ptr = self.lower_expr_to_operand(operand);
                Some(MirPlace::Ptr(ptr))
            }
            TypedExprKind::Index { base, index } => {
                let (ptr_expr, int_expr, pointee) =
                    if let Some(pointee) = self.pointee_type(base.ty) {
                        (base.as_ref(), index.as_ref(), pointee)
                    } else if let Some(pointee) = self.pointee_type(index.ty) {
                        (index.as_ref(), base.as_ref(), pointee)
                    } else {
                        return None;
                    };
                let base_ptr = self.lower_expr_to_operand(ptr_expr);
                let index_op = self.lower_expr_to_operand(int_expr);
                let index_mir_ty = self.map_type(int_expr.ty);
                let index_signed = self.is_signed_integer(int_expr.ty);
                let byte_offset =
                    self.scale_index_operand(index_op, index_mir_ty, index_signed, pointee);
                let ptr = self.emit_ptr_add(base_ptr, byte_offset);
                Some(MirPlace::Ptr(ptr))
            }
            TypedExprKind::MemberAccess { base, field, deref } => {
                let field_offset = if *deref {
                    let TypeKind::Pointer { pointee } = self.sema.types.get(base.ty).kind else {
                        return None;
                    };
                    let TypeKind::Record(record_id) = self.sema.types.get(pointee).kind else {
                        return None;
                    };
                    self.record_field_offset(record_id, *field)?
                } else {
                    let TypeKind::Record(record_id) = self.sema.types.get(base.ty).kind else {
                        return None;
                    };
                    self.record_field_offset(record_id, *field)?
                };

                if *deref {
                    let base_ptr = self.lower_expr_to_operand(base);
                    let ptr = self.emit_ptr_add_const(base_ptr, field_offset);
                    Some(MirPlace::Ptr(ptr))
                } else {
                    if let Some(base_place) = self.lower_place(base) {
                        match base_place {
                            MirPlace::Stack { slot, offset } => Some(MirPlace::Stack {
                                slot,
                                offset: offset + field_offset,
                            }),
                            MirPlace::Ptr(ptr) => {
                                let addr = self.emit_ptr_add_const(ptr, field_offset);
                                Some(MirPlace::Ptr(addr))
                            }
                        }
                    } else {
                        let base_ptr = self.lower_expr_address(base)?;
                        let ptr = self.emit_ptr_add_const(base_ptr, field_offset);
                        Some(MirPlace::Ptr(ptr))
                    }
                }
            }
            TypedExprKind::ImplicitCast { expr: inner, .. }
            | TypedExprKind::Cast { expr: inner, .. } => self.lower_place(inner),
            _ => None,
        }
    }

    fn lower_symbol_place(&mut self, symbol_id: SymbolId) -> Option<MirPlace> {
        if let Some(slot) = self.current_function_mut().slot_for_symbol(symbol_id) {
            return Some(MirPlace::Stack { slot, offset: 0 });
        }

        let (kind, storage) = {
            let symbol = self.symbol(symbol_id);
            (symbol.kind(), symbol.object_storage_class())
        };
        if kind == SymbolKind::Object
            && matches!(
                storage,
                Some(
                    ObjectStorageClass::FileScope
                        | ObjectStorageClass::Static
                        | ObjectStorageClass::Extern
                )
            )
            && let Some(name) = self.global_name_for_symbol(symbol_id)
        {
            let addr = self.alloc_vreg(MirType::Ptr);
            self.emit_current_instruction(Instruction::GlobalAddr {
                dst: addr,
                global: name,
            });
            return Some(MirPlace::Ptr(Operand::VReg(addr.reg)));
        }

        None
    }

    fn lower_expr_address(&mut self, expr: &TypedExpr) -> Option<Operand> {
        if let Some(ConstValue::Addr { symbol, offset }) = expr.const_value {
            return Some(self.lower_symbol_address(symbol, offset));
        }
        match &expr.kind {
            TypedExprKind::SymbolRef(symbol_id) => {
                let symbol = self.symbol(*symbol_id);
                if symbol.kind() == SymbolKind::Function {
                    return Some(self.lower_symbol_address(*symbol_id, 0));
                }
            }
            TypedExprKind::StringLiteral(text) => {
                let place = self.string_literal_place(text);
                return Some(self.address_of_place(place));
            }
            TypedExprKind::CompoundLiteral {
                ty,
                init,
                is_file_scope,
            } => {
                let place = self.materialize_compound_literal_place(
                    *ty,
                    init,
                    *is_file_scope,
                    (expr.span.start, expr.span.end),
                );
                return Some(self.address_of_place(place));
            }
            _ => {}
        }

        if self.is_aggregate_type(expr.ty) {
            return Some(self.lower_expr_to_operand(expr));
        }
        let place = self.lower_place(expr)?;
        Some(self.address_of_place(place))
    }

    fn lower_symbol_address(&mut self, symbol_id: SymbolId, offset: i64) -> Operand {
        if let Some(slot) = self.current_function_mut().slot_for_symbol(symbol_id) {
            let base = self.address_of_place(MirPlace::Stack { slot, offset: 0 });
            if offset != 0 {
                return self.emit_ptr_add_const(base, offset);
            }
            return base;
        }

        let name = self
            .global_name_for_symbol(symbol_id)
            .unwrap_or_else(|| self.symbol(symbol_id).name().to_string());
        let addr = self.alloc_vreg(MirType::Ptr);
        self.emit_current_instruction(Instruction::GlobalAddr {
            dst: addr,
            global: name,
        });
        if offset == 0 {
            Operand::VReg(addr.reg)
        } else {
            self.emit_ptr_add_const(Operand::VReg(addr.reg), offset)
        }
    }

    fn global_name_for_symbol(&self, symbol_id: SymbolId) -> Option<String> {
        if let Some(name) = self.object_global_names.get(&symbol_id) {
            return Some(name.clone());
        }
        let symbol = self.symbol(symbol_id);
        if symbol.kind() == SymbolKind::Function {
            return Some(symbol.name().to_string());
        }
        if symbol.kind() == SymbolKind::Object
            && matches!(
                symbol.object_storage_class(),
                Some(ObjectStorageClass::FileScope | ObjectStorageClass::Extern)
            )
        {
            return Some(symbol.name().to_string());
        }
        None
    }

    fn address_of_place(&mut self, place: MirPlace) -> Operand {
        match place {
            MirPlace::Stack { slot, offset } => {
                self.current_function_mut().mark_slot_address_taken(slot);
                let addr = self.alloc_vreg(MirType::Ptr);
                self.emit_current_instruction(Instruction::SlotAddr { dst: addr, slot });
                if offset == 0 {
                    Operand::VReg(addr.reg)
                } else {
                    self.emit_ptr_add_const(Operand::VReg(addr.reg), offset)
                }
            }
            MirPlace::Ptr(ptr) => ptr,
        }
    }

    fn load_from_place(&mut self, place: MirPlace, ty: TypeId) -> Operand {
        if self.is_aggregate_type(ty) {
            return self.address_of_place(place);
        }
        let mir_ty = self.map_type(ty);
        let volatile = self.is_volatile_type(ty);
        let dst = self.alloc_vreg(mir_ty);
        match place {
            MirPlace::Stack { slot, offset } => self.emit_current_instruction(Instruction::Load {
                dst,
                slot,
                offset,
                volatile,
            }),
            MirPlace::Ptr(ptr) => self.emit_current_instruction(Instruction::PtrLoad {
                dst,
                ptr,
                ty: mir_ty,
                volatile,
            }),
        }
        Operand::VReg(dst.reg)
    }

    fn store_to_place(&mut self, place: MirPlace, value: Operand, ty: TypeId) {
        if self.is_aggregate_type(ty) {
            return;
        }
        let mir_ty = self.map_type(ty);
        let volatile = self.is_volatile_type(ty);
        match place {
            MirPlace::Stack { slot, offset } => self.emit_current_instruction(Instruction::Store {
                slot,
                offset,
                value,
                ty: mir_ty,
                volatile,
            }),
            MirPlace::Ptr(ptr) => self.emit_current_instruction(Instruction::PtrStore {
                ptr,
                value,
                ty: mir_ty,
                volatile,
            }),
        }
    }

    fn emit_ptr_add(&mut self, base: Operand, byte_offset: Operand) -> Operand {
        let dst = self.alloc_vreg(MirType::Ptr);
        self.emit_current_instruction(Instruction::PtrAdd {
            dst,
            base,
            byte_offset,
        });
        Operand::VReg(dst.reg)
    }

    fn emit_ptr_add_const(&mut self, base: Operand, byte_offset: i64) -> Operand {
        self.emit_ptr_add(base, Operand::Const(MirConst::IntConst(byte_offset)))
    }

    fn cast_operand_to_type(&mut self, src: Operand, from_ty: TypeId, to_ty: TypeId) -> Operand {
        let from_mir = self.map_type(from_ty);
        let to_mir = self.map_type(to_ty);
        if from_mir == to_mir {
            return src;
        }

        if matches!(self.sema.types.get(to_ty).kind, TypeKind::Bool) {
            let truthy = self.lower_truthy_operand(from_ty, src);
            return Operand::VReg(truthy.reg);
        }

        let from_signed = self.is_signed_integer(from_ty);
        let to_signed = self.is_signed_integer(to_ty);
        self.cast_operand_between_mir(src, from_mir, from_signed, to_mir, to_signed)
    }

    /// Emit an explicit MIR cast between primitive MIR types when needed.
    fn cast_operand_between_mir(
        &mut self,
        src: Operand,
        from_mir: MirType,
        from_signed: bool,
        to_mir: MirType,
        to_signed: bool,
    ) -> Operand {
        if from_mir == to_mir {
            return src;
        }
        if to_mir == MirType::Void {
            return Operand::Const(MirConst::IntConst(0));
        }

        if from_mir == MirType::Ptr && to_mir == MirType::Ptr {
            let dst = self.alloc_vreg(MirType::Ptr);
            self.emit_current_instruction(Instruction::Copy { dst, src });
            return Operand::VReg(dst.reg);
        }

        let kind = match (from_mir, to_mir) {
            (from, to) if from.is_integer() && to.is_integer() => {
                let from_bits = Self::mir_int_bits(from);
                let to_bits = Self::mir_int_bits(to);
                if to_bits < from_bits {
                    CastKind::Trunc
                } else if from_signed {
                    CastKind::SExt
                } else {
                    CastKind::ZExt
                }
            }
            (from, to) if from.is_integer() && to.is_float() => {
                if from_signed {
                    CastKind::SIToF
                } else {
                    CastKind::UIToF
                }
            }
            (from, to) if from.is_float() && to.is_integer() => {
                if to_signed {
                    CastKind::FToSI
                } else {
                    CastKind::FToUI
                }
            }
            (MirType::F32, MirType::F64) => CastKind::FExt,
            (MirType::F64, MirType::F32) => CastKind::FTrunc,
            (MirType::Ptr, to) if to.is_integer() => CastKind::PToI,
            (from, MirType::Ptr) if from.is_integer() => CastKind::IToP,
            _ => {
                let dst = self.alloc_vreg(to_mir);
                self.emit_current_instruction(Instruction::Copy { dst, src });
                return Operand::VReg(dst.reg);
            }
        };
        let dst = self.alloc_vreg(to_mir);
        self.emit_current_instruction(Instruction::Cast {
            dst,
            kind,
            src,
            from_ty: from_mir,
            to_ty: to_mir,
        });
        Operand::VReg(dst.reg)
    }

    fn record_field_offset(
        &self,
        record_id: crate::frontend::sema::types::RecordId,
        field_id: crate::frontend::sema::types::FieldId,
    ) -> Option<i64> {
        let record = self.sema.records.get(record_id);
        let field_index = field_id.0 as usize;
        if field_index >= record.fields.len() {
            return None;
        }
        match record.kind {
            crate::frontend::parser::ast::RecordKind::Struct => {
                let mut offset = 0i64;
                for field in record.fields.iter().take(field_index) {
                    let size = i64::try_from(type_size_of(
                        field.ty,
                        &self.sema.types,
                        &self.sema.records,
                    )?)
                    .ok()?;
                    offset = offset.checked_add(size)?;
                }
                Some(offset)
            }
            crate::frontend::parser::ast::RecordKind::Union => Some(0),
        }
    }

    fn pointee_type(&self, ty: TypeId) -> Option<TypeId> {
        match self.sema.types.get(ty).kind {
            TypeKind::Pointer { pointee } => Some(pointee),
            _ => None,
        }
    }

    fn is_integer_type(&self, ty: TypeId) -> bool {
        matches!(
            self.sema.types.get(ty).kind,
            TypeKind::Bool
                | TypeKind::Char
                | TypeKind::SignedChar
                | TypeKind::UnsignedChar
                | TypeKind::Short { .. }
                | TypeKind::Int { .. }
                | TypeKind::Long { .. }
                | TypeKind::LongLong { .. }
                | TypeKind::Enum(_)
        )
    }

    fn is_float_type(&self, ty: TypeId) -> bool {
        matches!(
            self.sema.types.get(ty).kind,
            TypeKind::Float | TypeKind::Double
        )
    }

    fn is_pointer_type(&self, ty: TypeId) -> bool {
        matches!(self.sema.types.get(ty).kind, TypeKind::Pointer { .. })
    }

    fn is_aggregate_type(&self, ty: TypeId) -> bool {
        matches!(
            self.sema.types.get(ty).kind,
            TypeKind::Record(_) | TypeKind::Array { .. }
        )
    }

    fn is_volatile_type(&self, ty: TypeId) -> bool {
        self.sema.types.get(ty).quals.is_volatile
    }

    fn zero_operand_for_type(&mut self, ty: TypeId) -> Operand {
        match self.map_type(ty) {
            MirType::F32 | MirType::F64 => Operand::Const(MirConst::FloatConst(0.0)),
            _ => Operand::Const(MirConst::IntConst(0)),
        }
    }

    fn mir_int_bits(ty: MirType) -> u8 {
        match ty {
            MirType::I8 => 8,
            MirType::I16 => 16,
            MirType::I32 => 32,
            MirType::I64 => 64,
            _ => 0,
        }
    }

    fn mir_type_size_bytes(ty: MirType) -> u32 {
        match ty {
            MirType::I8 => 1,
            MirType::I16 => 2,
            MirType::I32 | MirType::F32 => 4,
            MirType::I64 | MirType::F64 | MirType::Ptr => 8,
            MirType::Void => 0,
        }
    }

    /// Lower a typed global initializer into bytes and relocations.
    fn lower_global_initializer(&mut self, ty: TypeId, init: &TypedInitializer) -> MirGlobalInit {
        if matches!(init, TypedInitializer::ZeroInit { .. }) {
            return MirGlobalInit::Zero;
        }

        let total_size = usize::try_from(self.type_size_u32(ty)).unwrap_or(0);
        if total_size == 0 {
            return MirGlobalInit::Data(Vec::new());
        }

        let mut bytes = vec![0u8; total_size];
        let mut relocations = Vec::new();
        self.encode_global_initializer_at(ty, init, 0, &mut bytes, &mut relocations);

        let all_zero = bytes.iter().all(|byte| *byte == 0);
        if relocations.is_empty() {
            if all_zero {
                MirGlobalInit::Zero
            } else {
                MirGlobalInit::Data(bytes)
            }
        } else {
            MirGlobalInit::RelocatedData { bytes, relocations }
        }
    }

    fn encode_global_initializer_at(
        &mut self,
        ty: TypeId,
        init: &TypedInitializer,
        base_offset: usize,
        bytes: &mut [u8],
        relocations: &mut Vec<MirRelocation>,
    ) {
        match init {
            TypedInitializer::ZeroInit { .. } => {}
            TypedInitializer::Expr(expr) => {
                self.encode_global_expr_at(ty, expr, base_offset, bytes, relocations);
            }
            TypedInitializer::Aggregate(items) => match self.sema.types.get(ty).kind.clone() {
                TypeKind::Array { elem, .. } => {
                    let elem_size = usize::try_from(self.type_size_u32(elem)).unwrap_or(0);
                    if elem_size == 0 {
                        return;
                    }
                    for (index, item) in items.iter().enumerate() {
                        let offset = base_offset.saturating_add(index.saturating_mul(elem_size));
                        self.encode_global_initializer_at(
                            elem,
                            &item.init,
                            offset,
                            bytes,
                            relocations,
                        );
                    }
                }
                TypeKind::Record(record_id) => {
                    let record = self.sema.records.get(record_id);
                    match record.kind {
                        crate::frontend::parser::ast::RecordKind::Struct => {
                            for (field_index, item) in items.iter().enumerate() {
                                if field_index >= record.fields.len() {
                                    break;
                                }
                                let field = &record.fields[field_index];
                                let field_offset = self
                                    .record_field_offset(
                                        record_id,
                                        crate::frontend::sema::types::FieldId(field_index as u32),
                                    )
                                    .and_then(|v| usize::try_from(v).ok())
                                    .unwrap_or(0);
                                self.encode_global_initializer_at(
                                    field.ty,
                                    &item.init,
                                    base_offset.saturating_add(field_offset),
                                    bytes,
                                    relocations,
                                );
                            }
                        }
                        crate::frontend::parser::ast::RecordKind::Union => {
                            for (field_index, item) in items.iter().enumerate() {
                                if field_index >= record.fields.len() {
                                    break;
                                }
                                let field = &record.fields[field_index];
                                self.encode_global_initializer_at(
                                    field.ty,
                                    &item.init,
                                    base_offset,
                                    bytes,
                                    relocations,
                                );
                            }
                        }
                    }
                }
                _ => {
                    if let Some(first) = items.first() {
                        self.encode_global_initializer_at(
                            ty,
                            &first.init,
                            base_offset,
                            bytes,
                            relocations,
                        );
                    }
                }
            },
            TypedInitializer::SparseArray {
                elem_ty,
                total_len: _,
                entries,
            } => {
                let elem_size = usize::try_from(self.type_size_u32(*elem_ty)).unwrap_or(0);
                if elem_size == 0 {
                    return;
                }
                for (index, item) in entries {
                    let offset = base_offset.saturating_add(index.saturating_mul(elem_size));
                    self.encode_global_initializer_at(
                        *elem_ty,
                        &item.init,
                        offset,
                        bytes,
                        relocations,
                    );
                }
            }
        }
    }

    fn encode_global_expr_at(
        &mut self,
        ty: TypeId,
        expr: &TypedExpr,
        base_offset: usize,
        bytes: &mut [u8],
        relocations: &mut Vec<MirRelocation>,
    ) {
        if matches!(self.sema.types.get(ty).kind, TypeKind::Pointer { .. })
            && let Some((target, addend)) = self.try_global_address_relocation(expr)
        {
            self.write_bytes_at(base_offset, &0u64.to_le_bytes(), bytes);
            relocations.push(MirRelocation {
                offset: u64::try_from(base_offset).unwrap_or(u64::MAX),
                target,
                addend,
            });
            return;
        }

        let Some(value) = self.global_const_value_for_expr(expr, ty) else {
            self.lowering_bug(
                "failed to encode constant global initializer expression",
                expr,
            );
        };

        let ty_kind = self.sema.types.get(ty).kind.clone();
        match ty_kind {
            TypeKind::Float => {
                if let Some(raw) = self.const_to_f32_bits(value) {
                    self.write_bytes_at(base_offset, &raw.to_le_bytes(), bytes);
                }
            }
            TypeKind::Double => {
                if let Some(raw) = self.const_to_f64_bits(value) {
                    self.write_bytes_at(base_offset, &raw.to_le_bytes(), bytes);
                }
            }
            TypeKind::Pointer { .. } => match value {
                ConstValue::NullPtr => {
                    self.write_bytes_at(base_offset, &0u64.to_le_bytes(), bytes);
                }
                ConstValue::Addr { symbol, offset } => {
                    self.write_bytes_at(base_offset, &0u64.to_le_bytes(), bytes);
                    relocations.push(MirRelocation {
                        offset: u64::try_from(base_offset).unwrap_or(u64::MAX),
                        target: self.global_relocation_target(symbol),
                        addend: offset,
                    });
                }
                ConstValue::Int(v) => {
                    self.write_bytes_at(base_offset, &(v as u64).to_le_bytes(), bytes);
                }
                ConstValue::UInt(v) => {
                    self.write_bytes_at(base_offset, &v.to_le_bytes(), bytes);
                }
                ConstValue::FloatBits(_) => {}
            },
            _ => {
                let size = usize::try_from(self.type_size_u32(ty)).unwrap_or(0);
                if size == 0 {
                    return;
                }
                let integer = match value {
                    ConstValue::Int(v) => v as u64,
                    ConstValue::UInt(v) => v,
                    ConstValue::NullPtr => 0,
                    ConstValue::Addr { .. } => 0,
                    ConstValue::FloatBits(bits) => bits,
                };
                let le = integer.to_le_bytes();
                self.write_bytes_at(base_offset, &le[..size.min(le.len())], bytes);
            }
        }
    }

    fn try_global_address_relocation(
        &mut self,
        expr: &TypedExpr,
    ) -> Option<(MirRelocationTarget, i64)> {
        if let Some(ConstValue::Addr { symbol, offset }) = expr.const_value {
            return Some((self.global_relocation_target(symbol), offset));
        }

        match &expr.kind {
            TypedExprKind::SymbolRef(symbol_id)
                if matches!(
                    expr.value_category,
                    ValueCategory::ArrayDesignator | ValueCategory::FunctionDesignator
                ) =>
            {
                Some((self.global_relocation_target(*symbol_id), 0))
            }
            TypedExprKind::StringLiteral(text) => Some((
                MirRelocationTarget::Global(self.ensure_string_literal_global_name(text)),
                0,
            )),
            TypedExprKind::CompoundLiteral {
                ty,
                init,
                is_file_scope: true,
            } => Some((
                MirRelocationTarget::Global(self.ensure_file_scope_compound_literal_global_name(
                    (expr.span.start, expr.span.end),
                    *ty,
                    init,
                )),
                0,
            )),
            TypedExprKind::ImplicitCast { expr: inner, .. }
            | TypedExprKind::Cast { expr: inner, .. } => self.try_global_address_relocation(inner),
            TypedExprKind::Conditional {
                cond,
                then_expr,
                else_expr,
            } => {
                let env = ConstEvalEnv {
                    types: &self.sema.types,
                    records: &self.sema.records,
                };
                let cond_value =
                    const_eval::eval_const_expr(cond, ConstExprContext::IntegerConstant, &env)
                        .ok()
                        .and_then(Self::const_value_to_i64)?;
                if cond_value != 0 {
                    self.try_global_address_relocation(then_expr)
                } else {
                    self.try_global_address_relocation(else_expr)
                }
            }
            _ => None,
        }
    }

    fn global_const_value_for_expr(
        &self,
        expr: &TypedExpr,
        target_ty: TypeId,
    ) -> Option<ConstValue> {
        if let Some(value) = expr.const_value {
            return Some(value);
        }
        let env = ConstEvalEnv {
            types: &self.sema.types,
            records: &self.sema.records,
        };
        if self.is_pointer_type(target_ty)
            && let Ok(value) =
                const_eval::eval_const_expr(expr, ConstExprContext::AddressConstant, &env)
        {
            return Some(value);
        }
        const_eval::eval_const_expr(expr, ConstExprContext::ArithmeticConstant, &env).ok()
    }

    fn const_to_f64_bits(&self, value: ConstValue) -> Option<u64> {
        match value {
            ConstValue::FloatBits(bits) => Some(bits),
            ConstValue::Int(v) => Some((v as f64).to_bits()),
            ConstValue::UInt(v) => Some((v as f64).to_bits()),
            ConstValue::NullPtr | ConstValue::Addr { .. } => None,
        }
    }

    fn const_to_f32_bits(&self, value: ConstValue) -> Option<u32> {
        match value {
            ConstValue::FloatBits(bits) => Some((f64::from_bits(bits) as f32).to_bits()),
            ConstValue::Int(v) => Some((v as f32).to_bits()),
            ConstValue::UInt(v) => Some((v as f32).to_bits()),
            ConstValue::NullPtr | ConstValue::Addr { .. } => None,
        }
    }

    fn const_value_to_i64(value: ConstValue) -> Option<i64> {
        match value {
            ConstValue::Int(v) => Some(v),
            ConstValue::UInt(v) => i64::try_from(v).ok(),
            ConstValue::FloatBits(_) | ConstValue::NullPtr | ConstValue::Addr { .. } => None,
        }
    }

    fn global_relocation_target(&self, symbol_id: SymbolId) -> MirRelocationTarget {
        let symbol = self.symbol(symbol_id);
        let name = self
            .global_name_for_symbol(symbol_id)
            .unwrap_or_else(|| symbol.name().to_string());
        if symbol.kind() == SymbolKind::Function {
            MirRelocationTarget::Function(name)
        } else {
            MirRelocationTarget::Global(name)
        }
    }

    fn write_bytes_at(&self, offset: usize, src: &[u8], bytes: &mut [u8]) {
        if offset >= bytes.len() {
            return;
        }
        let write_len = src.len().min(bytes.len() - offset);
        bytes[offset..offset + write_len].copy_from_slice(&src[..write_len]);
    }

    fn lower_const(&mut self, value: ConstValue) -> MirConst {
        match value {
            ConstValue::Int(value) => MirConst::IntConst(value),
            ConstValue::UInt(value) => MirConst::IntConst(value as i64),
            ConstValue::FloatBits(bits) => MirConst::FloatConst(f64::from_bits(bits)),
            ConstValue::NullPtr => MirConst::IntConst(0),
            ConstValue::Addr { .. } => MirConst::IntConst(0),
        }
    }

    fn lowering_bug(&self, message: &str, expr: &TypedExpr) -> ! {
        panic!(
            "MIR lowering bug: {message}; expr kind = {:?}, ty = {:?}, value_category = {:?}, span = {:?}",
            expr.kind, expr.ty, expr.value_category, expr.span
        );
    }

    fn stack_layout_of(&self, ty: TypeId) -> (u32, u32) {
        let size = type_size_of(ty, &self.sema.types, &self.sema.records)
            .and_then(|size| u32::try_from(size).ok())
            .unwrap_or(0)
            .max(1);
        let alignment = self.type_alignment_of(ty).unwrap_or(1).max(1);
        (size, alignment)
    }

    fn canonicalize_function_abi(
        &mut self,
        fn_ty: &crate::frontend::sema::types::FunctionType,
    ) -> (Vec<MirAbiParam>, MirType, bool) {
        let has_sret = self.is_aggregate_type(fn_ty.ret);
        let mut params = Vec::with_capacity(fn_ty.params.len() + usize::from(has_sret));
        if has_sret {
            params.push(MirAbiParam::struct_return());
        }
        for param_ty in &fn_ty.params {
            if self.is_aggregate_type(*param_ty) {
                let (size, _) = self.stack_layout_of(*param_ty);
                params.push(MirAbiParam::struct_argument(abi_struct_stack_size(size)));
            } else {
                params.push(MirAbiParam::new(self.map_type(*param_ty)));
            }
        }
        let return_type = if has_sret {
            MirType::Void
        } else {
            self.map_type(fn_ty.ret)
        };
        (params, return_type, has_sret)
    }

    fn extract_function_type(
        &self,
        ty: TypeId,
    ) -> Option<crate::frontend::sema::types::FunctionType> {
        match &self.sema.types.get(ty).kind {
            TypeKind::Function(function_ty) => Some(function_ty.clone()),
            TypeKind::Pointer { pointee } => match &self.sema.types.get(*pointee).kind {
                TypeKind::Function(function_ty) => Some(function_ty.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn type_alignment_of(&self, ty: TypeId) -> Option<u32> {
        let align = match &self.sema.types.get(ty).kind {
            TypeKind::Bool | TypeKind::Char | TypeKind::SignedChar | TypeKind::UnsignedChar => 1,
            TypeKind::Short { .. } => 2,
            TypeKind::Int { .. } | TypeKind::Enum(_) | TypeKind::Float => 4,
            TypeKind::Long { .. } | TypeKind::LongLong { .. } | TypeKind::Double => 8,
            TypeKind::Pointer { .. } | TypeKind::Function(_) => 8,
            TypeKind::Array { elem, .. } => return self.type_alignment_of(*elem),
            TypeKind::Record(record_id) => {
                let record = self.sema.records.get(*record_id);
                if !record.is_complete {
                    return None;
                }
                let mut max_align = 1u32;
                for field in &record.fields {
                    max_align = max_align.max(self.type_alignment_of(field.ty).unwrap_or(1));
                }
                max_align
            }
            TypeKind::Void | TypeKind::Error => return None,
        };
        Some(align)
    }
}

fn abi_struct_stack_size(size: u32) -> u32 {
    size.max(1).next_multiple_of(8)
}

fn map_linkage(linkage: Linkage) -> MirLinkage {
    match linkage {
        Linkage::Internal => MirLinkage::Internal,
        Linkage::External => MirLinkage::External,
        Linkage::None => unreachable!("file-scope MIR globals should never have no linkage"),
    }
}

/// Per-function mutable MIR construction state.
struct MirFunctionBuilder {
    function: MirFunction,
    source_return_ty: TypeId,
    next_block_id: u32,
    next_slot_id: u32,
    label_blocks: HashMap<LabelId, BlockId>,
    symbol_slots: HashMap<SymbolId, SlotId>,
    compound_literal_slots: HashMap<(usize, usize), SlotId>,
    current_block: BlockId,
    terminated_blocks: HashSet<BlockId>,
    loop_stack: Vec<LoopContext>,
    switch_stack: Vec<SwitchContext>,
    control_stack: Vec<ControlContext>,
    sret_slot: Option<SlotId>,
}

impl MirFunctionBuilder {
    /// Create a new function builder with an initialized entry block (`bb0`).
    fn new(
        name: String,
        linkage: MirLinkage,
        params: Vec<MirAbiParam>,
        return_type: MirType,
        boundary_sig: MirBoundarySignature,
        variadic: bool,
        source_return_ty: TypeId,
    ) -> Self {
        let mut builder = Self {
            function: MirFunction {
                name,
                linkage,
                params,
                return_type,
                boundary_sig,
                variadic,
                stack_slots: Vec::new(),
                blocks: Vec::new(),
                virtual_reg_counter: 0,
            },
            source_return_ty,
            next_block_id: 0,
            next_slot_id: 0,
            label_blocks: HashMap::new(),
            symbol_slots: HashMap::new(),
            compound_literal_slots: HashMap::new(),
            current_block: BlockId(0),
            terminated_blocks: HashSet::new(),
            loop_stack: Vec::new(),
            switch_stack: Vec::new(),
            control_stack: Vec::new(),
            sret_slot: None,
        };
        // Keep the MIR invariant that blocks[0] is the entry block.
        let _ = builder.alloc_block();
        builder.current_block = BlockId(0);
        builder
    }

    /// Finalize and return the built MIR function.
    fn finish(self) -> MirFunction {
        self.function
    }

    /// Allocate one basic block with default `unreachable` terminator.
    fn alloc_block(&mut self) -> BlockId {
        let block_id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        self.function.blocks.push(BasicBlock {
            id: block_id,
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        });
        block_id
    }

    /// Allocate one stack slot in the current function frame.
    fn alloc_slot(&mut self, size: u32, alignment: u32) -> SlotId {
        let slot_id = SlotId(self.next_slot_id);
        self.next_slot_id += 1;
        self.function.stack_slots.push(StackSlot {
            id: slot_id,
            size,
            alignment,
            address_taken: false,
        });
        slot_id
    }

    /// Allocate one typed virtual register.
    fn alloc_vreg(&mut self, ty: MirType) -> TypedVReg {
        self.function.alloc_vreg(ty)
    }

    /// Get or create the basic block assigned to a source-level label.
    fn ensure_label_block(&mut self, label: LabelId) -> BlockId {
        if let Some(block_id) = self.label_blocks.get(&label).copied() {
            return block_id;
        }
        let block_id = self.alloc_block();
        self.label_blocks.insert(label, block_id);
        block_id
    }

    fn bind_symbol_slot(&mut self, symbol_id: SymbolId, slot: SlotId) {
        self.symbol_slots.insert(symbol_id, slot);
    }

    fn has_symbol_slot(&self, symbol_id: SymbolId) -> bool {
        self.symbol_slots.contains_key(&symbol_id)
    }

    fn slot_for_symbol(&self, symbol_id: SymbolId) -> Option<SlotId> {
        self.symbol_slots.get(&symbol_id).copied()
    }

    fn compound_literal_slot(&self, key: (usize, usize)) -> Option<SlotId> {
        self.compound_literal_slots.get(&key).copied()
    }

    fn bind_compound_literal_slot(&mut self, key: (usize, usize), slot: SlotId) {
        self.compound_literal_slots.insert(key, slot);
    }

    fn set_sret_slot(&mut self, slot: SlotId) {
        self.sret_slot = Some(slot);
    }

    fn sret_slot(&self) -> Option<SlotId> {
        self.sret_slot
    }

    /// Record that a stack slot address escaped through `slot_addr`.
    fn mark_slot_address_taken(&mut self, slot: SlotId) {
        let slot_index = usize::try_from(slot.0).expect("slot index conversion failed");
        if let Some(stack_slot) = self.function.stack_slots.get_mut(slot_index) {
            stack_slot.address_taken = true;
        }
    }

    /// Append an instruction to a specific block.
    fn emit_instruction(&mut self, block_id: BlockId, instruction: Instruction) {
        let block_index = usize::try_from(block_id.0).expect("block index conversion failed");
        self.function
            .blocks
            .get_mut(block_index)
            .expect("invalid block id for emit_instruction")
            .instructions
            .push(instruction);
    }

    /// Append an instruction to the current block.
    fn emit_current_instruction(&mut self, instruction: Instruction) {
        self.emit_instruction(self.current_block, instruction);
    }

    /// Set the terminator for a specific block.
    fn emit_terminator_for_block(&mut self, block_id: BlockId, terminator: Terminator) {
        let block_index = usize::try_from(block_id.0).expect("block index conversion failed");
        self.function
            .blocks
            .get_mut(block_index)
            .expect("invalid block id for emit_terminator_for_block")
            .terminator = terminator;
        self.terminated_blocks.insert(block_id);
    }

    /// Set the terminator for the current block.
    fn emit_current_terminator(&mut self, terminator: Terminator) {
        self.emit_terminator_for_block(self.current_block, terminator);
    }

    /// Check whether the current block already has a terminating instruction.
    fn is_current_block_terminated(&self) -> bool {
        self.terminated_blocks.contains(&self.current_block)
    }

    /// Switch instruction emission to another basic block.
    fn set_current_block(&mut self, block: BlockId) {
        self.current_block = block;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::lexer::lexer_from_source;
    use crate::frontend::parser::parse;
    use crate::frontend::sema;
    use crate::frontend::sema::symbols::SymbolArena;
    use crate::frontend::sema::typed_ast::TypedTranslationUnit;
    use crate::frontend::sema::types::{EnumArena, Qualifiers, RecordArena, Type, TypeArena};
    use chumsky::input::{Input, Stream};

    fn empty_sema_result() -> SemaResult {
        SemaResult {
            typed_tu: TypedTranslationUnit { items: Vec::new() },
            types: TypeArena::new(),
            symbols: SymbolArena::default(),
            records: RecordArena::default(),
            enums: EnumArena::default(),
        }
    }

    fn analyze_source(src: &str) -> SemaResult {
        let tokens = lexer_from_source(src);
        let stream = Stream::from_iter(tokens)
            .map((src.len()..src.len()).into(), |(token, span)| (token, span));
        let tu = parse::parse(stream).expect("test source should parse");
        sema::analyze("test.c", src, &tu).expect("sema should succeed")
    }

    #[test]
    fn lower_to_mir_provides_entry_point_and_returns_program() {
        let sema = empty_sema_result();
        let program = lower_to_mir(&sema);
        assert!(program.globals.is_empty());
        assert!(program.functions.is_empty());
        assert!(program.extern_functions.is_empty());
    }

    #[test]
    fn function_allocators_are_monotonic() {
        let mut sema = empty_sema_result();
        let void_ty = sema.types.intern(Type {
            kind: TypeKind::Void,
            quals: Qualifiers::default(),
        });
        let mut cx = MirBuildContext::new(&sema);

        cx.begin_function(
            "f".to_string(),
            MirLinkage::Internal,
            Vec::new(),
            MirType::Void,
            MirBoundarySignature::from_internal(&[], MirType::Void, false),
            false,
            void_ty,
        );
        let bb1 = cx.alloc_block();
        let bb2 = cx.alloc_block();
        let slot0 = cx.alloc_slot(4, 4);
        let slot1 = cx.alloc_slot(8, 8);
        let v0 = cx.alloc_vreg(MirType::I32);
        let v1 = cx.alloc_vreg(MirType::Ptr);
        let label_bb = cx.ensure_label_block(LabelId(7));
        cx.end_function();

        assert_eq!(bb1, BlockId(1));
        assert_eq!(bb2, BlockId(2));
        assert_eq!(slot0, SlotId(0));
        assert_eq!(slot1, SlotId(1));
        assert_eq!(v0.reg.0, 0);
        assert_eq!(v1.reg.0, 1);
        assert_eq!(label_bb, BlockId(3));

        let program = cx.finish();
        let func = &program.functions[0];
        assert_eq!(func.blocks.len(), 4);
        assert_eq!(func.stack_slots.len(), 2);
        assert_eq!(func.virtual_reg_counter, 2);
    }

    #[test]
    fn control_flow_context_stacks_are_lifo() {
        let mut sema = empty_sema_result();
        let void_ty = sema.types.intern(Type {
            kind: TypeKind::Void,
            quals: Qualifiers::default(),
        });
        let mut cx = MirBuildContext::new(&sema);
        cx.begin_function(
            "f".to_string(),
            MirLinkage::Internal,
            Vec::new(),
            MirType::Void,
            MirBoundarySignature::from_internal(&[], MirType::Void, false),
            false,
            void_ty,
        );

        cx.push_loop_context(BlockId(10), BlockId(11));
        cx.push_switch_context(BlockId(20), Some(BlockId(21)));
        assert_eq!(cx.current_loop_context(), Some((BlockId(10), BlockId(11))));
        assert_eq!(
            cx.current_switch_context(),
            Some((BlockId(20), Some(BlockId(21))))
        );
        assert_eq!(
            cx.pop_switch_context(),
            Some((BlockId(20), Some(BlockId(21))))
        );
        assert_eq!(cx.pop_loop_context(), Some((BlockId(10), BlockId(11))));
        assert_eq!(cx.pop_loop_context(), None);
        assert_eq!(cx.pop_switch_context(), None);
    }

    #[test]
    fn map_type_and_signedness_use_cached_rules() {
        let mut sema = empty_sema_result();
        let int_ty = sema.types.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let uint_ty = sema.types.intern(Type {
            kind: TypeKind::Int { signed: false },
            quals: Qualifiers::default(),
        });
        let ptr_ty = sema.types.intern(Type {
            kind: TypeKind::Pointer { pointee: int_ty },
            quals: Qualifiers::default(),
        });

        let mut cx = MirBuildContext::new(&sema);
        assert_eq!(cx.map_type(int_ty), MirType::I32);
        assert_eq!(cx.map_type(ptr_ty), MirType::Ptr);
        assert!(cx.is_signed_integer(int_ty));
        assert!(!cx.is_signed_integer(uint_ty));
    }

    #[test]
    fn collects_extern_functions_and_global_skeletons() {
        let sema = analyze_source(
            r#"
            extern int printf(const char *fmt, ...);
            static int sg = 42;
            int g;
            extern int eg;
            int defined_fn(int x) { return x; }
        "#,
        );

        let program = lower_to_mir(&sema);
        assert_eq!(program.functions.len(), 1);
        assert_eq!(program.functions[0].name, "defined_fn");
        assert_eq!(program.functions[0].linkage, MirLinkage::External);
        assert_eq!(
            program.functions[0].params,
            vec![MirAbiParam::new(MirType::I32)]
        );
        assert_eq!(program.functions[0].return_type, MirType::I32);
        assert!(!program.functions[0].variadic);
        assert_eq!(program.functions[0].stack_slots.len(), 1);
        assert_eq!(program.extern_functions.len(), 1);
        assert_eq!(program.extern_functions[0].name, "printf");
        assert_eq!(
            program.extern_functions[0].sig.params,
            vec![MirAbiParam::new(MirType::Ptr)]
        );
        assert_eq!(program.extern_functions[0].sig.return_type, MirType::I32);
        assert!(program.extern_functions[0].sig.variadic);

        assert_eq!(program.globals.len(), 3);
        let sg = program
            .globals
            .iter()
            .find(|global| global.name == "sg")
            .expect("missing sg");
        let g = program
            .globals
            .iter()
            .find(|global| global.name == "g")
            .expect("missing g");
        let eg = program
            .globals
            .iter()
            .find(|global| global.name == "eg")
            .expect("missing eg");

        assert_eq!(sg.size, 4);
        assert_eq!(sg.alignment, 4);
        assert_eq!(sg.linkage, MirLinkage::Internal);
        assert!(matches!(sg.init, Some(MirGlobalInit::Data(ref bytes)) if bytes == &[42, 0, 0, 0]));

        assert_eq!(g.size, 4);
        assert_eq!(g.alignment, 4);
        assert_eq!(g.linkage, MirLinkage::External);
        assert!(matches!(g.init, Some(MirGlobalInit::Zero)));

        assert_eq!(eg.size, 4);
        assert_eq!(eg.alignment, 4);
        assert_eq!(eg.linkage, MirLinkage::External);
        assert!(eg.init.is_none());
    }

    #[test]
    fn lowers_static_function_with_internal_linkage() {
        let sema = analyze_source(
            r#"
            static int hidden(int x) { return x + 1; }
        "#,
        );
        let program = lower_to_mir(&sema);
        let hidden = program
            .functions
            .iter()
            .find(|func| func.name == "hidden")
            .expect("missing hidden function");
        assert_eq!(hidden.linkage, MirLinkage::Internal);
    }

    #[test]
    fn lowers_global_initializers_to_data_and_relocations() {
        let sema = analyze_source(
            r#"
            int g = 3;
            int *p = &g;
        "#,
        );
        let program = lower_to_mir(&sema);

        let g = program
            .globals
            .iter()
            .find(|global| global.name == "g")
            .expect("missing g");
        let p = program
            .globals
            .iter()
            .find(|global| global.name == "p")
            .expect("missing p");

        assert!(matches!(
            g.init,
            Some(MirGlobalInit::Data(ref bytes)) if bytes == &[3, 0, 0, 0]
        ));
        assert!(matches!(
            p.init,
            Some(MirGlobalInit::RelocatedData { ref bytes, ref relocations })
                if bytes.len() == 8
                && relocations.len() == 1
                && matches!(
                    relocations[0].target,
                    crate::mir::ir::MirRelocationTarget::Global(ref name) if name == "g"
                )
                && relocations[0].addend == 0
        ));
    }

    #[test]
    fn lowers_designator_address_initializers_to_relocations() {
        let sema = analyze_source(
            r#"
            int a[2];
            int *p = a;

            int f(void) { return 0; }
            int (*fp)(void) = f;
        "#,
        );
        let program = lower_to_mir(&sema);

        let p = program
            .globals
            .iter()
            .find(|global| global.name == "p")
            .expect("missing global p");
        let fp = program
            .globals
            .iter()
            .find(|global| global.name == "fp")
            .expect("missing global fp");

        assert!(matches!(
            p.init,
            Some(MirGlobalInit::RelocatedData { ref bytes, ref relocations })
                if bytes.len() == 8
                && relocations.len() == 1
                && matches!(
                    relocations[0].target,
                    MirRelocationTarget::Global(ref name) if name == "a"
                )
                && relocations[0].addend == 0
        ));
        assert!(matches!(
            fp.init,
            Some(MirGlobalInit::RelocatedData { ref bytes, ref relocations })
                if bytes.len() == 8
                && relocations.len() == 1
                && matches!(
                    relocations[0].target,
                    MirRelocationTarget::Function(ref name) if name == "f"
                )
                && relocations[0].addend == 0
        ));
    }

    #[test]
    fn function_skeleton_allocates_param_and_local_slots_and_prescans_labels() {
        let sema = analyze_source(
            r#"
            int f(int a, int b) {
                int x;
                register int y;
                static int s;
            L1:
                if (a) goto L1;
                return x + y + b + s;
            }
        "#,
        );
        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("function f should be lowered");

        // 2 params + 2 local auto/register objects; block-scope static is not a stack slot.
        assert_eq!(func.stack_slots.len(), 4);
        assert!(
            func.stack_slots
                .iter()
                .all(|slot| slot.size == 4 && slot.alignment == 4)
        );
        // Entry plus one pre-scanned label block.
        assert!(func.blocks.len() >= 2);
        // Param spill strategy emits store instructions in entry block.
        let param_stores = func.blocks[0]
            .instructions
            .iter()
            .filter(|inst| matches!(inst, Instruction::Store { .. }))
            .count();
        assert_eq!(param_stores, 2);
    }

    #[test]
    fn lowers_loop_switch_break_continue_control_flow() {
        let sema = analyze_source(
            r#"
            int g(int x) {
                int acc = 0;
                while (x) {
                    switch (x) {
                        case 1:
                            break;
                        default:
                            acc = acc + 1;
                    }
                    x = x - 1;
                    continue;
                }
                return acc;
            }
        "#,
        );

        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "g")
            .expect("function g should be lowered");

        assert!(func.blocks.len() >= 6);
        assert!(
            func.blocks
                .iter()
                .any(|block| matches!(block.terminator, Terminator::Branch { .. }))
        );
        assert!(
            func.blocks
                .iter()
                .any(|block| matches!(block.terminator, Terminator::Switch { .. }))
        );
        assert!(
            func.blocks
                .iter()
                .any(|block| matches!(block.terminator, Terminator::Ret(_)))
        );
    }

    #[test]
    fn lowers_short_circuit_with_call() {
        let sema = analyze_source(
            r#"
            extern int foo(int);
            int g(int x, int y) {
                return (x && foo(y)) || y;
            }
        "#,
        );

        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "g")
            .expect("function g should be lowered");

        let instructions: Vec<&Instruction> = func
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();

        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::Call { callee, .. } if callee == "foo"
        )));
        let branch_count = func
            .blocks
            .iter()
            .filter(|block| matches!(block.terminator, Terminator::Branch { .. }))
            .count();
        assert!(branch_count >= 2);
    }

    #[test]
    fn lowers_pointer_index_and_deref_with_ptr_add_and_ptr_load() {
        let sema = analyze_source(
            r#"
            int f(int *p, int i) {
                return p[i + 1] + *(p + i);
            }
        "#,
        );

        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("function f should be lowered");

        let instructions: Vec<&Instruction> = func
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();

        assert!(
            instructions
                .iter()
                .any(|inst| matches!(inst, Instruction::PtrAdd { .. }))
        );
        assert!(
            instructions
                .iter()
                .any(|inst| matches!(inst, Instruction::PtrLoad { .. }))
        );
    }

    #[test]
    fn lowers_global_constant_index_rvalue_via_ptr_load() {
        let sema = analyze_source("int a[2]; int f(void) { return a[0]; }");
        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("function f should be lowered");

        let instructions: Vec<&Instruction> = func
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();

        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::PtrLoad {
                ty: MirType::I32,
                ..
            }
        )));
    }

    #[test]
    fn lowers_local_constant_index_rvalue_via_ptr_load() {
        let sema = analyze_source("int f(void) { int a[2] = {0}; return a[0]; }");
        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("function f should be lowered");

        let instructions: Vec<&Instruction> = func
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();

        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::PtrLoad {
                ty: MirType::I32,
                ..
            }
        )));
    }

    #[test]
    fn lowers_string_array_constant_index_to_loaded_element_value() {
        let sema = analyze_source(r#"int f(void) { char s[] = "abc"; return s[1]; }"#);
        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("function f should be lowered");

        let instructions: Vec<&Instruction> = func
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();

        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::PtrLoad {
                ty: MirType::I8,
                ..
            }
        )));
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::Cast {
                from_ty: MirType::I8,
                to_ty: MirType::I32,
                ..
            }
        )));
    }

    #[test]
    fn array_index_arithmetic_uses_loaded_values_not_addresses() {
        let sema = analyze_source(
            r#"
            int main(void) {
                int b[2] = {1, 5};
                int x = b[1] + 2;
                return x + b[0];
            }
        "#,
        );
        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "main")
            .expect("function main should be lowered");

        let instructions: Vec<&Instruction> = func
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();

        let ptr_load_count = instructions
            .iter()
            .filter(|inst| {
                matches!(
                    inst,
                    Instruction::PtrLoad {
                        ty: MirType::I32,
                        ..
                    }
                )
            })
            .count();
        assert!(
            ptr_load_count >= 2,
            "expected ptr_load for both b[1] and b[0] rvalue uses"
        );

        let mut pointer_regs = std::collections::HashSet::new();
        for inst in &instructions {
            match inst {
                Instruction::SlotAddr { dst, .. }
                | Instruction::GlobalAddr { dst, .. }
                | Instruction::PtrAdd { dst, .. } => {
                    pointer_regs.insert(dst.reg);
                }
                _ => {}
            }
        }

        for inst in &instructions {
            if let Instruction::Binary {
                ty: MirType::I32,
                lhs,
                rhs,
                ..
            } = inst
            {
                if let Operand::VReg(reg) = lhs {
                    assert!(
                        !pointer_regs.contains(reg),
                        "i32 binary op should not consume pointer-producing vreg on lhs"
                    );
                }
                if let Operand::VReg(reg) = rhs {
                    assert!(
                        !pointer_regs.contains(reg),
                        "i32 binary op should not consume pointer-producing vreg on rhs"
                    );
                }
            }
        }
    }

    #[test]
    fn lowers_local_initializers_and_block_static_to_global() {
        let sema = analyze_source(
            r#"
            int f(int n) {
                int x = 7;
                int arr[4] = {1, 0, 3};
                static int s = 5;
                return x + arr[2] + n + s;
            }
        "#,
        );
        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("function f should be lowered");
        let instructions: Vec<&Instruction> = func
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();

        assert!(
            instructions
                .iter()
                .any(|inst| matches!(inst, Instruction::Memset { .. }))
        );
        assert!(
            instructions
                .iter()
                .any(|inst| matches!(inst, Instruction::Store { .. }))
        );
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::GlobalAddr { global, .. } if global.starts_with("__static_s_")
        )));

        let static_global = program
            .globals
            .iter()
            .find(|global| global.name.starts_with("__static_s_"))
            .expect("missing lowered block-static global");
        assert_eq!(static_global.linkage, MirLinkage::Internal);
        assert!(matches!(
            static_global.init,
            Some(MirGlobalInit::Data(ref bytes)) if bytes == &[5, 0, 0, 0]
        ));
    }

    #[test]
    fn lowers_volatile_object_accesses_with_volatile_memory_ops() {
        let sema = analyze_source(
            r#"
            int f(volatile int *p) {
                volatile int x = 1;
                *p = x;
                return *p + x;
            }
        "#,
        );
        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("function f should be lowered");

        let instructions: Vec<&Instruction> = func
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();

        assert!(
            instructions
                .iter()
                .any(|inst| matches!(inst, Instruction::Store { volatile: true, .. }))
        );
        assert!(
            instructions
                .iter()
                .any(|inst| matches!(inst, Instruction::Load { volatile: true, .. }))
        );
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::PtrStore {
                volatile: true,
                ty: MirType::I32,
                ..
            }
        )));
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::PtrLoad {
                volatile: true,
                ty: MirType::I32,
                ..
            }
        )));
    }

    #[test]
    fn preserves_variadic_metadata_for_function_definitions() {
        let sema = analyze_source("int f(int x, ...) { return x; }");
        let program = lower_to_mir(&sema);
        let func = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("function f should be lowered");

        assert_eq!(func.params, vec![MirAbiParam::new(MirType::I32)]);
        assert_eq!(func.return_type, MirType::I32);
        assert!(func.variadic);

        let dump = crate::mir::display::dump(&program);
        assert!(dump.contains("fn @f(i32, ...) -> i32 {"));
    }

    #[test]
    fn map_function_sig_handles_prototype_types() {
        let sema = analyze_source("double f(short a, unsigned long b, ...);");
        let f_symbol_ty = (0..sema.symbols.len())
            .map(|idx| sema.symbols.get(SymbolId(idx as u32)))
            .find(|symbol| symbol.name() == "f")
            .expect("function f should exist")
            .ty();

        let mut cx = MirBuildContext::new(&sema);
        let sig = cx
            .map_function_sig(f_symbol_ty)
            .expect("function type should map to MIR signature");

        assert_eq!(
            sig.params,
            vec![
                MirAbiParam::new(MirType::I16),
                MirAbiParam::new(MirType::I64)
            ]
        );
        assert_eq!(sig.return_type, MirType::F64);
        assert!(sig.variadic);
    }

    #[test]
    fn map_function_sig_canonicalizes_aggregate_abi() {
        let sema = analyze_source(
            r#"
            struct Pair { int x; int y; };
            struct Pair f(struct Pair p);
        "#,
        );
        let f_symbol_ty = (0..sema.symbols.len())
            .map(|idx| sema.symbols.get(SymbolId(idx as u32)))
            .find(|symbol| symbol.name() == "f")
            .expect("function f should exist")
            .ty();

        let mut cx = MirBuildContext::new(&sema);
        let sig = cx
            .map_function_sig(f_symbol_ty)
            .expect("function type should map to MIR signature");

        assert_eq!(
            sig.params,
            vec![
                MirAbiParam::struct_return(),
                MirAbiParam::struct_argument(8)
            ]
        );
        assert_eq!(sig.return_type, MirType::Void);
        assert!(!sig.variadic);
    }

    #[test]
    fn lowers_aggregate_call_and_return_with_sret_and_argument_copy() {
        let sema = analyze_source(
            r#"
            struct Pair { int x; int y; };

            struct Pair id(struct Pair p) {
                return p;
            }

            struct Pair wrap(struct Pair p) {
                return id(p);
            }
        "#,
        );
        let program = lower_to_mir(&sema);
        let id = program
            .functions
            .iter()
            .find(|func| func.name == "id")
            .expect("id should be lowered");
        let wrap = program
            .functions
            .iter()
            .find(|func| func.name == "wrap")
            .expect("wrap should be lowered");

        assert_eq!(
            id.params,
            vec![
                MirAbiParam::struct_return(),
                MirAbiParam::struct_argument(8)
            ]
        );
        assert_eq!(id.return_type, MirType::Void);
        assert_eq!(
            wrap.params,
            vec![
                MirAbiParam::struct_return(),
                MirAbiParam::struct_argument(8)
            ]
        );
        assert_eq!(wrap.return_type, MirType::Void);

        let id_memcpy_count = id
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .filter(|inst| matches!(inst, Instruction::Memcpy { .. }))
            .count();
        assert!(id_memcpy_count >= 2);
        assert!(
            id.blocks
                .iter()
                .any(|block| matches!(block.terminator, Terminator::Ret(None)))
        );

        let mut memcpy_before_id_call = 0usize;
        let mut saw_call = false;
        for inst in wrap
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
        {
            if matches!(inst, Instruction::Memcpy { .. }) {
                memcpy_before_id_call += 1;
            }
            if let Instruction::Call {
                callee, dst, args, ..
            } = inst
                && callee == "id"
            {
                saw_call = true;
                assert!(dst.is_none());
                assert_eq!(args.len(), 2);
                // One memcpy is for param spill in entry; another one must be for
                // by-value aggregate argument materialization at call-site.
                assert!(memcpy_before_id_call >= 2);
            }
        }
        assert!(saw_call);
    }

    #[test]
    fn lowers_call_indirect_with_canonicalized_aggregate_abi() {
        let sema = analyze_source(
            r#"
            struct Pair { int x; int y; };

            struct Pair invoke(struct Pair (*fp)(struct Pair), struct Pair p) {
                return fp(p);
            }
        "#,
        );
        let program = lower_to_mir(&sema);
        let invoke = program
            .functions
            .iter()
            .find(|func| func.name == "invoke")
            .expect("invoke should be lowered");

        assert_eq!(
            invoke.params,
            vec![
                MirAbiParam::struct_return(),
                MirAbiParam::new(MirType::Ptr),
                MirAbiParam::struct_argument(8),
            ]
        );
        assert_eq!(invoke.return_type, MirType::Void);

        let mut saw_call_indirect = false;
        for inst in invoke
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
        {
            if let Instruction::CallIndirect {
                dst,
                args,
                sig,
                boundary_sig,
                fixed_arg_count,
                ..
            } = inst
            {
                saw_call_indirect = true;
                assert!(dst.is_none());
                assert_eq!(args.len(), 2);
                assert_eq!(
                    sig.params,
                    vec![
                        MirAbiParam::struct_return(),
                        MirAbiParam::struct_argument(8)
                    ]
                );
                assert_eq!(sig.return_type, MirType::Void);
                assert!(!sig.variadic);
                assert_eq!(
                    boundary_sig.as_ref(),
                    Some(&MirBoundarySignature {
                        params: vec![MirBoundaryParam::AggregateScalarized {
                            parts: vec![MirType::I64],
                            size: 8,
                        }],
                        return_type: MirBoundaryReturn::AggregateScalarized {
                            parts: vec![MirType::I64],
                            size: 8,
                        },
                        variadic: false,
                    })
                );
                assert!(fixed_arg_count.is_none());
            }
        }
        assert!(saw_call_indirect);
    }

    #[test]
    fn break_target_prefers_innermost_breakable_context() {
        let mut sema = empty_sema_result();
        let void_ty = sema.types.intern(Type {
            kind: TypeKind::Void,
            quals: Qualifiers::default(),
        });
        let mut cx = MirBuildContext::new(&sema);
        cx.begin_function(
            "f".to_string(),
            MirLinkage::Internal,
            Vec::new(),
            MirType::Void,
            MirBoundarySignature::from_internal(&[], MirType::Void, false),
            false,
            void_ty,
        );

        cx.push_switch_context(BlockId(20), None);
        cx.push_loop_context(BlockId(10), BlockId(11));
        assert_eq!(cx.current_break_target(), Some(BlockId(10)));
        assert_eq!(cx.current_continue_target(), Some(BlockId(11)));

        let _ = cx.pop_loop_context();
        assert_eq!(cx.current_break_target(), Some(BlockId(20)));
    }

    #[test]
    fn lowers_string_literals_to_internal_globals_and_relocations() {
        let sema = analyze_source(
            r#"
            const char *s = "hi";
            extern int puts(const char *);
            int f(void) { return puts("hi"); }
        "#,
        );
        let program = lower_to_mir(&sema);

        let string_globals: Vec<_> = program
            .globals
            .iter()
            .filter(|global| global.name.starts_with(".str."))
            .collect();
        assert_eq!(
            string_globals.len(),
            1,
            "string literal globals should be deduplicated by literal content"
        );
        let string_global = string_globals[0];
        assert_eq!(string_global.linkage, MirLinkage::Internal);
        assert!(matches!(
            string_global.init,
            Some(MirGlobalInit::Data(ref bytes)) if bytes == b"hi\0"
        ));

        let s = program
            .globals
            .iter()
            .find(|global| global.name == "s")
            .expect("missing global s");
        assert!(matches!(
            s.init,
            Some(MirGlobalInit::RelocatedData { ref relocations, .. })
                if relocations.len() == 1
                && matches!(
                    relocations[0].target,
                    MirRelocationTarget::Global(ref name) if name == &string_global.name
                )
        ));

        let f = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("missing function f");
        let instructions: Vec<&Instruction> = f
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::GlobalAddr { global, .. } if global == &string_global.name
        )));
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::Call { callee, args, .. }
                if callee == "puts"
                && !matches!(args.first(), Some(Operand::Const(MirConst::IntConst(0))))
        )));
    }

    #[test]
    fn lowers_aggregate_conditional_member_access() {
        let sema = analyze_source(
            r#"
            struct Pair { int x; int y; };
            struct Pair a, b;
            int f(int c) { return (c ? a : b).y; }
        "#,
        );
        let program = lower_to_mir(&sema);
        let f = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("missing function f");
        let instructions: Vec<&Instruction> = f
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();

        assert!(
            instructions
                .iter()
                .any(|inst| matches!(inst, Instruction::Memcpy { .. }))
        );
        assert!(
            instructions
                .iter()
                .any(|inst| matches!(inst, Instruction::PtrAdd { .. }))
        );
        assert!(
            instructions
                .iter()
                .any(|inst| matches!(inst, Instruction::PtrLoad { .. }))
        );
        assert!(
            f.blocks
                .iter()
                .any(|block| matches!(block.terminator, Terminator::Ret(Some(Operand::VReg(_)))))
        );
    }

    #[test]
    fn return_values_are_cast_to_function_return_type() {
        let sema = analyze_source("int f(short x) { return x; }");
        let program = lower_to_mir(&sema);
        let f = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("missing function f");

        let cast_dst = f
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .find_map(|inst| match inst {
                Instruction::Cast { dst, to_ty, .. } if *to_ty == MirType::I32 => Some(*dst),
                _ => None,
            })
            .expect("missing return cast to i32");

        assert!(f.blocks.iter().any(|block| matches!(
            block.terminator,
            Terminator::Ret(Some(Operand::VReg(reg))) if reg == cast_dst.reg
        )));
    }

    #[test]
    fn collects_block_scope_extern_objects_into_globals() {
        let sema = analyze_source("int f(void) { extern int g; return g; }");
        let program = lower_to_mir(&sema);

        let g = program
            .globals
            .iter()
            .find(|global| global.name == "g")
            .expect("missing extern object skeleton for g");
        assert_eq!(g.size, 4);
        assert_eq!(g.alignment, 4);
        assert_eq!(g.linkage, MirLinkage::External);
        assert!(g.init.is_none());

        let f = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("missing function f");
        assert!(
            f.blocks
                .iter()
                .flat_map(|block| block.instructions.iter())
                .any(
                    |inst| matches!(inst, Instruction::GlobalAddr { global, .. } if global == "g")
                )
        );
    }

    #[test]
    fn lowers_block_scope_scalar_compound_literal_via_stack_slot() {
        let sema = analyze_source("int f(void) { return (int){7}; }");
        let program = lower_to_mir(&sema);
        let f = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("missing function f");

        assert_eq!(f.stack_slots.len(), 1);
        let instructions: Vec<&Instruction> = f
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::Store {
                value: Operand::Const(MirConst::IntConst(7)),
                ty: MirType::I32,
                ..
            }
        )));
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::Load { dst, offset: 0, .. } if dst.ty == MirType::I32
        )));
    }

    #[test]
    fn lowers_block_scope_aggregate_compound_literal_member_access() {
        let sema = analyze_source(
            r#"
            struct Pair { int x; int y; };
            int f(void) { return ((struct Pair){1, 2}).y; }
        "#,
        );
        let program = lower_to_mir(&sema);
        let f = program
            .functions
            .iter()
            .find(|func| func.name == "f")
            .expect("missing function f");

        assert_eq!(f.stack_slots.len(), 1);
        let instructions: Vec<&Instruction> = f
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .collect();
        assert!(
            instructions
                .iter()
                .any(|inst| matches!(inst, Instruction::Memset { size: 8, .. }))
        );
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::Store {
                offset: 0,
                value: Operand::Const(MirConst::IntConst(1)),
                ty: MirType::I32,
                ..
            }
        )));
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::Store {
                offset: 4,
                value: Operand::Const(MirConst::IntConst(2)),
                ty: MirType::I32,
                ..
            }
        )));
        assert!(instructions.iter().any(|inst| matches!(
            inst,
            Instruction::Load {
                dst,
                offset: 4,
                ..
            } if dst.ty == MirType::I32
        )));
    }

    #[test]
    fn lowers_file_scope_compound_literal_pointer_initializer() {
        let sema = analyze_source("int *p = (int[]){1, 2};");
        let program = lower_to_mir(&sema);

        let compound = program
            .globals
            .iter()
            .find(|global| global.name.starts_with("__compound_"))
            .expect("missing synthetic compound literal global");
        assert_eq!(compound.linkage, MirLinkage::Internal);
        assert!(matches!(
            compound.init,
            Some(MirGlobalInit::Data(ref bytes)) if bytes == &[1, 0, 0, 0, 2, 0, 0, 0]
        ));

        let p = program
            .globals
            .iter()
            .find(|global| global.name == "p")
            .expect("missing global p");
        assert!(matches!(
            p.init,
            Some(MirGlobalInit::RelocatedData { ref relocations, .. })
                if relocations.len() == 1
                && matches!(
                    relocations[0].target,
                    MirRelocationTarget::Global(ref name) if name == &compound.name
                )
        ));
    }
}
