use binaryninja::{
    architecture::{Architecture, Register, RegisterInfo},
    binary_view::BinaryViewExt,
    logger::Logger,
    low_level_il::{
        expression::{
            ExpressionHandler, LowLevelILExpression, ValueExpr,
        }, function::{FunctionForm, FunctionMutability}, instruction::{
            InstructionHandler, LowLevelILInstruction,
            LowLevelInstructionIndex,
        }, lifting::LowLevelILLabel, LowLevelILRegister
    },
    rc::Ref,
    workflow::{Activity, AnalysisContext, Workflow},
};
use log::LevelFilter;

mod llil;

const OBJC_EXTRAS_ACTIVITY_NAME: &str = "bdash.objc-extras";
const OBJC_EXTRAS_ACTIVITY_CONFIG: &str = r#"{
    "name" : "bdash.objc-extras",
    "title" : "Extra Objective-C processing",
    "description": "",
    "eligibility": {
        "auto": {}
    }
}"#;

const IGNORABLE_FUNCTIONS: &[&str] = &[
    "_objc_retain",
    "_objc_release",
    "_objc_autorelease",
    "_objc_autoreleaseReturnValue",
    "_objc_retainAutoreleasedReturnValue",
    "j__objc_retain",
    "j__objc_release",
    "j__objc_autorelease",
    "j__objc_autoreleaseReturnValue",
    "j__objc_retainAutoreleasedReturnValue",
];

fn is_call_to_ignorable_function<'func, A, M, F>(
    view: &binaryninja::binary_view::BinaryView,
    instr: &'func LowLevelILInstruction<'func, A, M, F>,
) -> bool
where
    A: 'func + Architecture + std::fmt::Debug,
    M: FunctionMutability + std::fmt::Debug,
    F: FunctionForm + std::fmt::Debug,
    LowLevelILInstruction<'func, A, M, F>: InstructionHandler<'func, A, M, F>,
    LowLevelILExpression<'func, A, M, F, ValueExpr>: ExpressionHandler<'func, A, M, F>,
{
    use llil::{Expression::*, Instruction::*};

    let target = match instr.into() {
        Call(ConstPtr(address)) | TailCall(ConstPtr(address)) => address,
        Goto(target) => target.address().clone(),
        _ => return false,
    };

    let Some(symbol) = view.symbol_by_address(target) else {
        log::info!("No symbol found for address {:#0x}", target);
        return false;
    };

    if IGNORABLE_FUNCTIONS.contains(&symbol.full_name().as_str()) {
        log::warn!("Ignoring call to {}", symbol.full_name());
        return true;
    }
    return false;
}

fn process_objc_extras(analysis_context: &AnalysisContext) {
    let Some(llil) = (unsafe { analysis_context.llil_function() }) else {
        return;
    };

    let mut did_replace = false;
    let func = analysis_context.function();

    let Some(link_register) = func.arch().link_reg() else {
        return;
    };
    let link_register_size = link_register.info().size();
    let link_register = LowLevelILRegister::ArchReg(link_register);

    for idx in 0..=llil.instruction_count() {
        let Some(instr) = llil.instruction_from_index(LowLevelInstructionIndex(idx as usize))
        else {
            continue;
        };

        // TODO: Detect calls to `objc_release` that are immediately after a load of a struct field.
        // It might be preferable to leave those in place since otherwise the load is left behind.
        if !is_call_to_ignorable_function(&analysis_context.view(), &instr) {
            continue;
        }

        use llil::Instruction::*;
        use llil::Expression::*;
        match (&instr).into() {
            TailCall(_) => unsafe {
                llil.replace_expression(
                    instr.expr_idx(),
                    llil.ret(llil.reg(link_register_size, link_register)),
                );
            },
            Call(_) => unsafe {
                llil.replace_expression(instr.expr_idx(), llil.nop());
            },
            Goto(_) => {
                // The shared cache workflow inlines calls to stub functions, which causes them
                // to show up as a `lr = <next instruction>; goto <stub function instruction>;` sequence.
                // We need to remove the load of `lr`  and update the `goto` to jump to the next instruction.

                let Some(prev) =
                    llil.instruction_from_index(LowLevelInstructionIndex(idx - 1 as usize))
                else {
                    continue;
                };

                let target = match (&prev).into() {
                    SetReg(reg, ConstPtr(target)) if reg == link_register => target,
                    _ => continue,
                };
                let mut label = llil.label_for_address(target).unwrap_or_else(|| {
                    let mut label = LowLevelILLabel::new();
                    label.operand = llil.instruction_index_at(target).unwrap().0;
                    label
                });
                unsafe {
                    llil.replace_expression(prev.expr_idx(), llil.nop());
                    llil.replace_expression(instr.expr_idx(), llil.goto(&mut label));
                }
            }
            _ => {}
        }

        let tag_type = analysis_context
            .view()
            .tag_type_by_name("Objective-C Extras")
            .unwrap_or_else(|| {
                analysis_context
                    .view()
                    .create_tag_type("Objective-C Extras", "OC")
            });
        func.add_tag(
            &tag_type,
            "Eliminated Obj-C runtime call",
            Some(instr.address()),
            false,
            None,
        );
        did_replace = true;
    }

    if did_replace {
        llil.generate_ssa_form();
    }
    analysis_context.set_lifted_il_function(&llil);
}

fn register_activity(workflow: Ref<Workflow>) {
    let workflow = workflow.clone_to(workflow.name());
    let activity = Activity::new_with_action(OBJC_EXTRAS_ACTIVITY_CONFIG, process_objc_extras);
    workflow.register_activity(&activity).unwrap();
    workflow.insert(
        "core.function.generateMediumLevelIL",
        [OBJC_EXTRAS_ACTIVITY_NAME],
    );
    workflow.register().unwrap();
}

#[unsafe(no_mangle)]
pub extern "C" fn CorePluginDependencies() {
    use binaryninja::add_optional_plugin_dependency;
    add_optional_plugin_dependency("workflow_objc");
    add_optional_plugin_dependency("sharedcache");
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "C" fn CorePluginInit() -> bool {
    Logger::new("Obj-C Extras")
        .with_level(LevelFilter::Debug)
        .init();

    register_activity(Workflow::instance("core.function.metaAnalysis"));
    register_activity(Workflow::instance("core.function.objectiveC"));
    register_activity(Workflow::instance("core.function.sharedCache"));

    true
}
