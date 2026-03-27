use std::collections::{HashMap, HashSet};

use crate::mir::ir::{
    BinaryOp, BlockId, CmpKind, Instruction, MirFunction, MirProgram, Operand, Terminator,
};
use crate::mir::optimization::run_optimization_passes;

/// Run the MIR pass pipeline for every function in the program.
///
/// `optimization == 0` runs only mandatory lowering/canonicalization passes.
/// `optimization > 0` additionally runs optimization passes.
pub fn run_pass_pipeline(program: &mut MirProgram, optimization: u32) {
    for function in &mut program.functions {
        run_lowering_passes(function);
        if optimization > 0 {
            run_optimization_passes(function);
        }
    }
}

/// Run mandatory post-lowering cleanup passes.
fn run_lowering_passes(function: &mut MirFunction) {
    normalize_commutative_operands(function);
    eliminate_unreachable_blocks(function);
}

/// Canonicalize commutative operations so constants are placed on the RHS.
///
/// This makes downstream pattern matching simpler and deterministic.
fn normalize_commutative_operands(function: &mut MirFunction) -> bool {
    let mut changed = false;
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            match instruction {
                Instruction::Binary { op, lhs, rhs, .. }
                    if is_commutative_binary(*op)
                        && matches!(lhs, Operand::Const(_))
                        && !matches!(rhs, Operand::Const(_)) =>
                {
                    std::mem::swap(lhs, rhs);
                    changed = true;
                }
                Instruction::Cmp { kind, lhs, rhs, .. }
                    if matches!(kind, CmpKind::Eq | CmpKind::Ne)
                        && matches!(lhs, Operand::Const(_))
                        && !matches!(rhs, Operand::Const(_)) =>
                {
                    std::mem::swap(lhs, rhs);
                    changed = true;
                }
                _ => {}
            }
        }
    }
    changed
}

/// Remove blocks that are not reachable from the entry block.
///
/// After pruning, block ids are compacted and terminator targets are remapped.
fn eliminate_unreachable_blocks(function: &mut MirFunction) -> bool {
    if function.blocks.is_empty() {
        return false;
    }

    let mut reachable = HashSet::new();
    let mut worklist = vec![function.blocks[0].id];
    let block_map = block_index_map(function);

    while let Some(block_id) = worklist.pop() {
        if !reachable.insert(block_id) {
            continue;
        }
        let Some(&block_index) = block_map.get(&block_id) else {
            continue;
        };
        for succ in terminator_successors(&function.blocks[block_index].terminator) {
            worklist.push(succ);
        }
    }

    if reachable.len() == function.blocks.len() {
        return false;
    }

    let mut old_to_new = HashMap::new();
    let mut next_id = 0u32;
    for block in &function.blocks {
        if reachable.contains(&block.id) {
            old_to_new.insert(block.id, BlockId(next_id));
            next_id += 1;
        }
    }

    let mut new_blocks = Vec::with_capacity(reachable.len());
    for mut block in function.blocks.drain(..) {
        if !reachable.contains(&block.id) {
            continue;
        }
        let new_id = *old_to_new
            .get(&block.id)
            .expect("reachable block should be remapped");
        rewrite_terminator_block_ids(&mut block.terminator, &old_to_new);
        block.id = new_id;
        new_blocks.push(block);
    }
    function.blocks = new_blocks;
    true
}

/// Return whether a binary op is safe to reorder for canonicalization.
fn is_commutative_binary(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add
            | BinaryOp::Mul
            | BinaryOp::And
            | BinaryOp::Or
            | BinaryOp::Xor
            | BinaryOp::FAdd
            | BinaryOp::FMul
    )
}

/// Build a quick `BlockId -> index` map for random access in traversal.
fn block_index_map(function: &MirFunction) -> HashMap<BlockId, usize> {
    function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.id, index))
        .collect()
}

/// Collect successor blocks from a terminator.
fn terminator_successors(terminator: &Terminator) -> Vec<BlockId> {
    match terminator {
        Terminator::Jump(target) => vec![*target],
        Terminator::Branch {
            then_bb, else_bb, ..
        } => vec![*then_bb, *else_bb],
        Terminator::Switch { cases, default, .. } => {
            let mut targets = Vec::with_capacity(cases.len() + 1);
            targets.push(*default);
            targets.extend(cases.iter().map(|case| case.target));
            targets
        }
        Terminator::Ret(_) | Terminator::Unreachable => Vec::new(),
    }
}

/// Rewrite all block-id references inside one terminator using `old_to_new`.
fn rewrite_terminator_block_ids(
    terminator: &mut Terminator,
    old_to_new: &HashMap<BlockId, BlockId>,
) {
    match terminator {
        Terminator::Jump(target) => *target = remap_block_id(*target, old_to_new),
        Terminator::Branch {
            then_bb, else_bb, ..
        } => {
            *then_bb = remap_block_id(*then_bb, old_to_new);
            *else_bb = remap_block_id(*else_bb, old_to_new);
        }
        Terminator::Switch { cases, default, .. } => {
            *default = remap_block_id(*default, old_to_new);
            for case in cases {
                case.target = remap_block_id(case.target, old_to_new);
            }
        }
        Terminator::Ret(_) | Terminator::Unreachable => {}
    }
}

/// Remap one block id, assuming all reachable targets are present.
fn remap_block_id(block_id: BlockId, old_to_new: &HashMap<BlockId, BlockId>) -> BlockId {
    *old_to_new
        .get(&block_id)
        .expect("reachable terminator target should be remapped")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::ir::{
        BasicBlock, Instruction, MirConst, MirFunction, MirProgram, MirType, Operand, TypedVReg,
        VReg,
    };

    fn empty_function(blocks: Vec<BasicBlock>) -> MirFunction {
        MirFunction {
            name: "f".to_string(),
            params: Vec::new(),
            return_type: MirType::I32,
            variadic: false,
            stack_slots: Vec::new(),
            blocks,
            virtual_reg_counter: 0,
        }
    }

    #[test]
    fn lowering_passes_remove_unreachable_blocks() {
        let mut function = empty_function(vec![
            BasicBlock {
                id: BlockId(0),
                instructions: Vec::new(),
                terminator: Terminator::Jump(BlockId(1)),
            },
            BasicBlock {
                id: BlockId(1),
                instructions: Vec::new(),
                terminator: Terminator::Ret(Some(Operand::Const(MirConst::IntConst(0)))),
            },
            BasicBlock {
                id: BlockId(2),
                instructions: vec![Instruction::Copy {
                    dst: TypedVReg {
                        reg: VReg(0),
                        ty: MirType::I32,
                    },
                    src: Operand::Const(MirConst::IntConst(7)),
                }],
                terminator: Terminator::Unreachable,
            },
        ]);

        assert!(eliminate_unreachable_blocks(&mut function));
        assert_eq!(function.blocks.len(), 2);
        assert_eq!(function.blocks[0].id, BlockId(0));
        assert_eq!(function.blocks[1].id, BlockId(1));
    }
}
