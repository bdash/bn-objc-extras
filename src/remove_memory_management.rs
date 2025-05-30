use binaryninja::{
    architecture::{Architecture, Register as _, RegisterInfo as _},
    binary_view::BinaryViewExt as _,
    low_level_il::{
        LowLevelILRegisterKind,
        expression::{ExpressionHandler, LowLevelILExpression, ValueExpr},
        function::{FunctionForm, FunctionMutability},
        instruction::{InstructionHandler, LowLevelILInstruction, LowLevelInstructionIndex},
        lifting::LowLevelILLabel,
    },
    workflow::AnalysisContext,
};

use bn_bdash_extras::llil::match_instr;

// j_ prefixes are for stub functions in the dyld shared cache.
// The prefix is added by Binary Ninja's shared cache workflow.
const IGNORABLE_MEMORY_MANAGEMENT_FUNCTIONS: &[&[u8]] = &[
    b"_objc_autorelease",
    b"_objc_autoreleaseReturnValue",
    b"_objc_release",
    b"_objc_retain",
    b"_objc_retainAutorelease",
    b"_objc_retainAutoreleaseReturnValue",
    b"_objc_retainAutoreleasedReturnValue",
    b"_objc_retainBlock",
    b"_objc_unsafeClaimAutoreleasedReturnValue",
    b"j__objc_autorelease",
    b"j__objc_autoreleaseReturnValue",
    b"j__objc_release",
    b"j__objc_retain",
    b"j__objc_retainAutorelease",
    b"j__objc_retainAutoreleaseReturnValue",
    b"j__objc_retainAutoreleasedReturnValue",
    b"j__objc_retainBlock",
    b"j__objc_unsafeClaimAutoreleasedReturnValue",
];

fn is_call_to_ignorable_memory_management_function<'func, M, F>(
    view: &binaryninja::binary_view::BinaryView,
    instr: &'func LowLevelILInstruction<'func, M, F>,
) -> bool
where
    M: FunctionMutability + std::fmt::Debug,
    F: FunctionForm + std::fmt::Debug,
    LowLevelILInstruction<'func, M, F>: InstructionHandler<'func, M, F>,
    LowLevelILExpression<'func, M, F, ValueExpr>: ExpressionHandler<'func, M, F>,
{
    let target = match_instr! {
        instr,
        Call(ConstPtr(address)) | TailCall(ConstPtr(address)) => address,
        Goto(target) => target.address(),
        _ => return false,
    };
    let Some(symbol) = view.symbol_by_address(target) else {
        return false;
    };
    IGNORABLE_MEMORY_MANAGEMENT_FUNCTIONS.contains(&symbol.full_name().to_bytes())
}

pub(crate) fn action(analysis_context: &AnalysisContext) {
    let Some(llil) = (unsafe { analysis_context.llil_function() }) else {
        return;
    };

    let func = analysis_context.function();

    let Some(link_register) = func.arch().link_reg() else {
        return;
    };
    let link_register_size = link_register.info().size();
    let link_register = LowLevelILRegisterKind::Arch(link_register);

    let mut did_replace = false;
    for idx in 0..=llil.instruction_count() {
        let Some(instr) = llil.instruction_from_index(LowLevelInstructionIndex(idx)) else {
            continue;
        };

        // TODO: Detect calls to `objc_release` that are immediately after a load of a struct field.
        // It might be preferable to leave those in place since otherwise the load is left behind.
        if !is_call_to_ignorable_memory_management_function(&analysis_context.view(), &instr) {
            continue;
        }

        match_instr! {
            instr,
            TailCall(_) => unsafe {
                llil.set_current_address(instr.address());
                llil.replace_expression(
                    instr.expr_idx(),
                    llil.ret(llil.reg(link_register_size, link_register)),
                );
            },
            Call(_) => unsafe {
                llil.set_current_address(instr.address());
                llil.replace_expression(instr.expr_idx(), llil.nop());
            },
            Goto(_) if idx == 0 => unsafe {
                // If the `objc_retain` is the first instruction in the function, this function
                // must only contain the call to the memory management function since when the
                // memory management function returns, it will return to this function's caller.
                llil.set_current_address(instr.address());
                    llil.replace_expression(
                        instr.expr_idx(),
                        llil.ret(llil.reg(link_register_size, link_register)),
                    );
            },
            Goto(_) => {
                // The shared cache workflow inlines calls to stub functions, which causes them
                // to show up as a `lr = <next instruction>; goto <stub function instruction>;` sequence.
                // We need to remove the load of `lr`  and update the `goto` to jump to the next instruction.

                let Some(prev) =
                    llil.instruction_from_index(LowLevelInstructionIndex(idx - 1_usize))
                else {
                    continue;
                };

                let target = match_instr!{
                    prev,
                    SetReg(reg, ConstPtr(target)) if *reg == link_register => target,
                    _ => continue,
                };

                let Some(LowLevelInstructionIndex(target_idx)) = llil.instruction_index_at(target) else {   
                    continue;
                };

                let mut label = LowLevelILLabel::new();
                label.operand = target_idx;

                unsafe {
                    llil.set_current_address(prev.address());
                    llil.replace_expression(prev.expr_idx(), llil.nop());
                    llil.set_current_address(instr.address());
                    llil.replace_expression(instr.expr_idx(), llil.goto(&mut label));
                }
            }
            _ => {}
        }

        func.add_tag(
            &crate::tag_type_for_view(&analysis_context.view()),
            "Removed memory management call",
            Some(instr.address()),
            false,
            None,
        );
        did_replace = true;
    }

    if did_replace {
        llil.generate_ssa_form();
    }
}
