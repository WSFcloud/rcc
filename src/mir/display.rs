use std::fmt::{self, Write};

use crate::mir::ir::{
    BasicBlock, BinaryOp, BlockId, CastKind, CmpDomain, CmpKind, Instruction, MirConst,
    MirExternFunction, MirFunction, MirFunctionSig, MirGlobal, MirGlobalInit, MirLinkage,
    MirProgram, MirRelocation, MirRelocationTarget, MirType, Operand, SlotId, StackSlot,
    SwitchCase, Terminator, TypedVReg, UnaryOp, VReg,
};

/// Dump MIR program into a deterministic textual form.
#[must_use]
pub fn dump(program: &MirProgram) -> String {
    program.to_string()
}

impl fmt::Display for MirProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first_section = true;

        if !self.globals.is_empty() {
            for global in &self.globals {
                writeln!(f, "{global}")?;
            }
            first_section = false;
        }

        if !self.extern_functions.is_empty() {
            if !first_section {
                writeln!(f)?;
            }
            for func in &self.extern_functions {
                writeln!(f, "{func}")?;
            }
            first_section = false;
        }

        if !self.functions.is_empty() {
            if !first_section {
                writeln!(f)?;
            }

            for (idx, func) in self.functions.iter().enumerate() {
                if idx > 0 {
                    writeln!(f)?;
                }
                write!(f, "{func}")?;
                writeln!(f)?;
            }
        }

        Ok(())
    }
}

impl fmt::Display for MirGlobal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.init {
            Some(init) => write!(
                f,
                "global @{}: {} bytes, align {}, {}, init = {}",
                self.name, self.size, self.alignment, self.linkage, init
            ),
            None => write!(
                f,
                "extern @{}: {} bytes, align {}",
                self.name, self.size, self.alignment
            ),
        }
    }
}

impl fmt::Display for MirGlobalInit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data(bytes) => {
                write!(f, "bytes[")?;
                write_hex_bytes(f, bytes)?;
                write!(f, "]")
            }
            Self::RelocatedData { bytes, relocations } => {
                write!(f, "reloc(bytes[")?;
                write_hex_bytes(f, bytes)?;
                write!(f, "], [")?;
                for (i, reloc) in relocations.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{reloc}")?;
                }
                write!(f, "])")
            }
            Self::Zero => write!(f, "zeroinit"),
        }
    }
}

impl fmt::Display for MirRelocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.offset, self.target)?;
        if self.addend != 0 {
            if self.addend > 0 {
                write!(f, " + {}", self.addend)
            } else {
                write!(f, " - {}", self.addend.unsigned_abs())
            }
        } else {
            Ok(())
        }
    }
}

impl fmt::Display for MirRelocationTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global(name) | Self::Function(name) => write!(f, "@{name}"),
        }
    }
}

impl fmt::Display for MirExternFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "extern @{}{}", self.name, SignatureDisplay(&self.sig))
    }
}

impl fmt::Display for MirFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "fn @{}{} {{",
            self.name,
            SignatureDisplay::from_parts(&self.params, self.return_type, false)
        )?;

        if !self.stack_slots.is_empty() {
            writeln!(f)?;
            writeln!(f, "  slots:")?;
            for slot in &self.stack_slots {
                writeln!(f, "    {slot}")?;
            }
        }

        if !self.blocks.is_empty() {
            writeln!(f)?;
            for (idx, block) in self.blocks.iter().enumerate() {
                if idx > 0 {
                    writeln!(f)?;
                }
                write!(f, "{block}")?;
            }
        }

        write!(f, "\n}}")
    }
}

impl fmt::Display for StackSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} bytes, align {}",
            self.id, self.size, self.alignment
        )?;
        if self.address_taken {
            write!(f, ", address_taken")?;
        }
        Ok(())
    }
}

impl fmt::Display for BasicBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}:", self.id)?;
        for inst in &self.instructions {
            writeln!(f, "  {inst}")?;
        }
        write!(f, "  {}", self.terminator)
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load {
                dst,
                slot,
                offset,
                volatile,
            } => {
                write_dst(f, dst)?;
                if *volatile {
                    write!(f, "load.volatile.{} ", dst.ty)?;
                } else {
                    write!(f, "load.{} ", dst.ty)?;
                }
                write_slot_address(f, *slot, *offset)
            }
            Self::Store {
                slot,
                offset,
                value,
                ty,
                volatile,
            } => {
                if *volatile {
                    write!(f, "store.volatile.{ty} ")?;
                } else {
                    write!(f, "store.{ty} ")?;
                }
                write_slot_address(f, *slot, *offset)?;
                write!(f, ", {value}")
            }
            Self::PtrLoad {
                dst,
                ptr,
                ty,
                volatile,
            } => {
                write_dst(f, dst)?;
                if *volatile {
                    write!(f, "load.volatile.{ty} [{ptr}]")
                } else {
                    write!(f, "load.{ty} [{ptr}]")
                }
            }
            Self::PtrStore {
                ptr,
                value,
                ty,
                volatile,
            } => {
                if *volatile {
                    write!(f, "store.volatile.{ty} [{ptr}], {value}")
                } else {
                    write!(f, "store.{ty} [{ptr}], {value}")
                }
            }
            Self::Memcpy {
                dst_ptr,
                src_ptr,
                size,
            } => write!(f, "memcpy {dst_ptr}, {src_ptr}, {size}"),
            Self::Memset {
                dst_ptr,
                value,
                size,
            } => write!(f, "memset {dst_ptr}, {value}, {size}"),
            Self::Binary {
                dst,
                op,
                lhs,
                rhs,
                ty,
            } => {
                write_dst(f, dst)?;
                write!(f, "{}.{ty} {lhs}, {rhs}", op.mnemonic())
            }
            Self::Unary {
                dst,
                op,
                operand,
                ty,
            } => {
                write_dst(f, dst)?;
                write!(f, "{}.{ty} {operand}", op.mnemonic())
            }
            Self::Cmp {
                dst,
                kind,
                domain,
                lhs,
                rhs,
                ty,
            } => {
                write_dst(f, dst)?;
                write!(f, "cmp.{}.{ty} {lhs}, {rhs}", cmp_mnemonic(*kind, *domain))
            }
            Self::Cast {
                dst,
                kind,
                src,
                from_ty,
                to_ty,
            } => {
                write_dst(f, dst)?;
                write!(f, "{}.{}.{} {src}", kind.mnemonic(), from_ty, to_ty)
            }
            Self::Call {
                dst,
                callee,
                args,
                fixed_arg_count,
            } => {
                if let Some(dst) = dst {
                    write_dst(f, dst)?;
                }
                write!(f, "call @{callee}(")?;
                write_args(f, args)?;
                write!(f, ")")?;
                if let Some(count) = fixed_arg_count {
                    write!(f, ", fixed_args {count}")?;
                }
                Ok(())
            }
            Self::CallIndirect {
                dst,
                callee_ptr,
                args,
                sig,
                fixed_arg_count,
            } => {
                if let Some(dst) = dst {
                    write_dst(f, dst)?;
                }
                write!(
                    f,
                    "call_indirect {callee_ptr} : {} (",
                    SignatureDisplay(sig)
                )?;
                write_args(f, args)?;
                write!(f, ")")?;
                if let Some(count) = fixed_arg_count {
                    write!(f, ", fixed_args {count}")?;
                }
                Ok(())
            }
            Self::SlotAddr { dst, slot } => {
                write_dst(f, dst)?;
                write!(f, "slot_addr {slot}")
            }
            Self::GlobalAddr { dst, global } => {
                write_dst(f, dst)?;
                write!(f, "global_addr @{global}")
            }
            Self::PtrAdd {
                dst,
                base,
                byte_offset,
            } => {
                write_dst(f, dst)?;
                write!(f, "ptr_add.ptr {base}, {byte_offset}")
            }
            Self::Copy { dst, src } => {
                write_dst(f, dst)?;
                write!(f, "copy {src}")
            }
        }
    }
}

impl fmt::Display for Terminator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Jump(target) => write!(f, "jump {target}"),
            Self::Branch {
                cond,
                then_bb,
                else_bb,
            } => write!(f, "branch {cond}, {then_bb}, {else_bb}"),
            Self::Switch {
                discr,
                cases,
                default,
            } => {
                write!(f, "switch {discr}, [")?;
                for (idx, case) in cases.iter().enumerate() {
                    if idx > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{case}")?;
                }
                write!(f, "], {default}")
            }
            Self::Ret(value) => match value {
                Some(v) => write!(f, "ret {v}"),
                None => write!(f, "ret"),
            },
            Self::Unreachable => write!(f, "unreachable"),
        }
    }
}

impl fmt::Display for SwitchCase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.value, self.target)
    }
}

impl fmt::Display for MirLinkage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::External => write!(f, "external"),
            Self::Internal => write!(f, "internal"),
        }
    }
}

impl fmt::Display for MirType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::I8 => write!(f, "i8"),
            Self::I16 => write!(f, "i16"),
            Self::I32 => write!(f, "i32"),
            Self::I64 => write!(f, "i64"),
            Self::F32 => write!(f, "f32"),
            Self::F64 => write!(f, "f64"),
            Self::Ptr => write!(f, "ptr"),
            Self::Void => write!(f, "void"),
        }
    }
}

impl fmt::Display for MirConst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IntConst(v) => write!(f, "{v}"),
            Self::FloatConst(v) => write!(f, "{v}"),
            Self::ZeroConst => write!(f, "0"),
        }
    }
}

impl fmt::Display for Operand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VReg(reg) => write!(f, "{reg}"),
            Self::Const(value) => write!(f, "{value}"),
            Self::StackSlot(slot) => write!(f, "{slot}"),
        }
    }
}

impl fmt::Display for VReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "%{}", self.0)
    }
}

impl fmt::Display for SlotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "${}", self.0)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bb{}", self.0)
    }
}

impl BinaryOp {
    fn mnemonic(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::SDiv => "sdiv",
            Self::UDiv => "udiv",
            Self::SRem => "srem",
            Self::URem => "urem",
            Self::And => "and",
            Self::Or => "or",
            Self::Xor => "xor",
            Self::Shl => "shl",
            Self::AShr => "ashr",
            Self::LShr => "lshr",
            Self::FAdd => "fadd",
            Self::FSub => "fsub",
            Self::FMul => "fmul",
            Self::FDiv => "fdiv",
            Self::FRem => "frem",
        }
    }
}

impl UnaryOp {
    fn mnemonic(self) -> &'static str {
        match self {
            Self::Neg => "neg",
            Self::Not => "not",
        }
    }
}

impl CastKind {
    fn mnemonic(self) -> &'static str {
        match self {
            Self::Trunc => "trunc",
            Self::ZExt => "zext",
            Self::SExt => "sext",
            Self::IToF => "itof",
            Self::FToI => "ftoi",
            Self::FExt => "fext",
            Self::FTrunc => "ftrunc",
            Self::PToI => "ptoi",
            Self::IToP => "itop",
        }
    }
}

fn cmp_mnemonic(kind: CmpKind, domain: CmpDomain) -> &'static str {
    match kind {
        CmpKind::Eq => "eq",
        CmpKind::Ne => "ne",
        CmpKind::Lt => match domain {
            CmpDomain::Signed => "slt",
            CmpDomain::Unsigned => "ult",
            CmpDomain::Float => "flt",
        },
        CmpKind::Le => match domain {
            CmpDomain::Signed => "sle",
            CmpDomain::Unsigned => "ule",
            CmpDomain::Float => "fle",
        },
        CmpKind::Gt => match domain {
            CmpDomain::Signed => "sgt",
            CmpDomain::Unsigned => "ugt",
            CmpDomain::Float => "fgt",
        },
        CmpKind::Ge => match domain {
            CmpDomain::Signed => "sge",
            CmpDomain::Unsigned => "uge",
            CmpDomain::Float => "fge",
        },
    }
}

fn write_hex_bytes(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            write!(f, ", ")?;
        }
        write!(f, "0x{b:02X}")?;
    }
    Ok(())
}

fn write_dst(f: &mut fmt::Formatter<'_>, dst: &TypedVReg) -> fmt::Result {
    write!(f, "{} = ", dst.reg)
}

fn write_slot_address(f: &mut fmt::Formatter<'_>, slot: SlotId, offset: i64) -> fmt::Result {
    if offset == 0 {
        write!(f, "[{slot}]")
    } else if offset > 0 {
        write!(f, "[{slot} + {offset}]")
    } else {
        write!(f, "[{slot} - {}]", offset.unsigned_abs())
    }
}

fn write_args(f: &mut fmt::Formatter<'_>, args: &[Operand]) -> fmt::Result {
    for (idx, arg) in args.iter().enumerate() {
        if idx > 0 {
            write!(f, ", ")?;
        }
        write!(f, "{arg}")?;
    }
    Ok(())
}

struct SignatureDisplay<'a>(&'a MirFunctionSig);

impl<'a> SignatureDisplay<'a> {
    fn from_parts(params: &'a [MirType], return_type: MirType, variadic: bool) -> String {
        let mut sig = String::new();
        sig.push('(');
        for (idx, ty) in params.iter().enumerate() {
            if idx > 0 {
                sig.push_str(", ");
            }
            let _ = write!(sig, "{ty}");
        }
        if variadic {
            if !params.is_empty() {
                sig.push_str(", ");
            }
            sig.push_str("...");
        }
        let _ = write!(sig, ") -> {return_type}");
        sig
    }
}

impl fmt::Display for SignatureDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sig = self.0;
        write!(
            f,
            "{}",
            SignatureDisplay::from_parts(&sig.params, sig.return_type, sig.variadic)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::dump;
    use crate::mir::ir::{
        BasicBlock, BinaryOp, BlockId, Instruction, MirExternFunction, MirFunction, MirFunctionSig,
        MirGlobal, MirGlobalInit, MirLinkage, MirProgram, MirType, Operand, SlotId, StackSlot,
        Terminator, TypedVReg, VReg,
    };

    #[test]
    fn dump_contains_expected_sections() {
        let program = MirProgram {
            globals: vec![MirGlobal {
                name: "counter".to_string(),
                size: 4,
                alignment: 4,
                linkage: MirLinkage::Internal,
                init: Some(MirGlobalInit::Zero),
            }],
            extern_functions: vec![MirExternFunction {
                name: "printf".to_string(),
                sig: MirFunctionSig {
                    params: vec![MirType::Ptr],
                    return_type: MirType::I32,
                    variadic: true,
                },
            }],
            functions: vec![MirFunction {
                name: "add".to_string(),
                params: vec![MirType::I32, MirType::I32],
                return_type: MirType::I32,
                stack_slots: vec![StackSlot {
                    id: SlotId(0),
                    size: 4,
                    alignment: 4,
                    address_taken: false,
                }],
                blocks: vec![BasicBlock {
                    id: BlockId(0),
                    instructions: vec![Instruction::Binary {
                        dst: TypedVReg {
                            reg: VReg(0),
                            ty: MirType::I32,
                        },
                        op: BinaryOp::Add,
                        lhs: Operand::VReg(VReg(1)),
                        rhs: Operand::Const(crate::mir::ir::MirConst::IntConst(2)),
                        ty: MirType::I32,
                    }],
                    terminator: Terminator::Ret(Some(Operand::VReg(VReg(0)))),
                }],
                virtual_reg_counter: 1,
            }],
        };

        let output = dump(&program);
        assert!(output.contains("global @counter: 4 bytes, align 4, internal, init = zeroinit"));
        assert!(output.contains("extern @printf(ptr, ...) -> i32"));
        assert!(output.contains("fn @add(i32, i32) -> i32 {"));
        assert!(output.contains("%0 = add.i32 %1, 2"));
        assert!(output.contains("ret %0"));
    }

    #[test]
    fn volatile_and_addresses_are_formatted() {
        let func = MirFunction {
            name: "mem".to_string(),
            params: vec![],
            return_type: MirType::Void,
            stack_slots: vec![],
            blocks: vec![BasicBlock {
                id: BlockId(0),
                instructions: vec![
                    Instruction::Load {
                        dst: TypedVReg {
                            reg: VReg(0),
                            ty: MirType::I32,
                        },
                        slot: SlotId(1),
                        offset: 8,
                        volatile: false,
                    },
                    Instruction::Store {
                        slot: SlotId(1),
                        offset: -4,
                        value: Operand::VReg(VReg(0)),
                        ty: MirType::I32,
                        volatile: true,
                    },
                ],
                terminator: Terminator::Ret(None),
            }],
            virtual_reg_counter: 1,
        };

        let text = func.to_string();
        assert!(text.contains("%0 = load.i32 [$1 + 8]"));
        assert!(text.contains("store.volatile.i32 [$1 - 4], %0"));
    }
}
