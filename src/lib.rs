use binaryninja::{
    architecture::{Architecture, Register, RegisterInfo},
    binary_view::BinaryViewExt,
    confidence::Conf,
    logger::Logger,
    low_level_il::{
        LowLevelILRegister,
        expression::{ExpressionHandler, LowLevelILExpression, ValueExpr},
        function::{FunctionForm, FunctionMutability},
        instruction::{InstructionHandler, LowLevelILInstruction, LowLevelInstructionIndex},
        lifting::LowLevelILLabel,
    },
    medium_level_il::{
        MediumLevelILFunction, MediumLevelILLiftedInstruction, MediumLevelILLiftedInstructionKind,
        operation::{Constant, LiftedCallSsa, LiftedLoadSsa},
    },
    rc::Ref,
    types::Type,
    variable::RegisterValueType,
    workflow::{Activity, AnalysisContext, Workflow},
};
use log::LevelFilter;

mod activity;
mod llil;

const OBJC_REMOVE_MEMORY_MANAGMENT_ACTIVITY_NAME: &str = "bdash.objc-remove-memory-management";
const OBJC_TYPE_PROPAGATION_ACTIVITY_NAME: &str = "bdash.objc-type-propagation";

fn tag_type_for_view(
    view: &binaryninja::binary_view::BinaryView,
) -> Ref<binaryninja::tags::TagType> {
    view.tag_type_by_name("Objective-C Extras")
        .unwrap_or_else(|| view.create_tag_type("Objective-C Extras", "OC"))
}

const IGNORABLE_MEMORY_MANAGEMENT_FUNCTIONS: &[&str] = &[
    "_objc_autorelease",
    "_objc_autoreleaseReturnValue",
    "_objc_release",
    "_objc_retain",
    "_objc_retainAutorelease",
    "_objc_retainAutoreleasedReturnValue",
    "j__objc_autorelease",
    "j__objc_autoreleaseReturnValue",
    "j__objc_release",
    "j__objc_retain",
    "j__objc_retainAutorelease",
    "j__objc_retainAutoreleasedReturnValue",
];

fn is_call_to_ignorable_memory_management_function<'func, A, M, F>(
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
        return false;
    };

    if IGNORABLE_MEMORY_MANAGEMENT_FUNCTIONS.contains(&symbol.full_name().as_str()) {
        log::warn!(
            "Ignoring call to {} ({target:#0x}) at {:#0x}",
            symbol.full_name(),
            instr.address()
        );
        return true;
    }
    return false;
}

fn remove_memory_management(analysis_context: &AnalysisContext) {
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
        if !is_call_to_ignorable_memory_management_function(&analysis_context.view(), &instr) {
            continue;
        }

        use llil::Expression::*;
        use llil::Instruction::*;
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

                if idx == 0 {
                    // If the `objc_retain` is the first instruction in the function, `lr` is already set.
                    // TODO: What should we rewrite this to? See `_MecabraCandidateRetain` in libmecabra.dylib.
                    log::error!(
                        "Found goto at first instruction in function: {:#0x}",
                        instr.address()
                    );
                    continue;
                }

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

        func.add_tag(
            &tag_type_for_view(&analysis_context.view()),
            "Removed memory management call",
            Some(instr.address()),
            false,
            None,
        );
        did_replace = true;
    }

    if did_replace {
        llil.generate_ssa_form();
        analysis_context.set_lifted_il_function(&llil);
    }
}

const ALLOC_INIT_FUNCTIONS: &[&str] = &[
    "_objc_alloc_init",
    "_objc_alloc_initWithZone",
    "_objc_alloc",
    "_objc_opt_new",
    "j__objc_alloc_init",
    "j__objc_alloc_initWithZone",
    "j__objc_alloc",
    "j__objc_opt_new",
];

fn ssa_variable_value_or_loaded_pointer(
    function: &MediumLevelILFunction,
    var: &binaryninja::variable::SSAVariable,
) -> Option<u64> {
    let value = function.ssa_variable_value(var);
    match value.state {
        RegisterValueType::ConstantPointerValue => Some(value.value as u64),
        RegisterValueType::UndeterminedValue => {
            let Some(def) = function.ssa_variable_definition(var) else {
                return None;
            };
            let MediumLevelILLiftedInstructionKind::SetVarSsa(set_var) = def.lift().kind else {
                return None;
            };

            let MediumLevelILLiftedInstructionKind::LoadSsa(LiftedLoadSsa { src, .. }) =
                set_var.src.kind
            else {
                return None;
            };
            let MediumLevelILLiftedInstructionKind::ConstPtr(Constant {
                constant: src_memory,
            }) = src.kind
            else {
                return None;
            };

            Some(src_memory)
        }
        _ => None,
    }
}

fn return_type_for_alloc_function(
    function: &MediumLevelILFunction,
    instr: &MediumLevelILLiftedInstruction,
    call: &LiftedCallSsa,
    target_function: &binaryninja::function::Function,
    view: &binaryninja::binary_view::BinaryView,
) -> Option<Ref<Type>> {
    if call.params.len() != 1 {
        log::debug!(
            "Call to {} at {:#0x} has {} parameters, expected 1",
            target_function.symbol().full_name(),
            instr.address,
            call.params.len()
        );
        return None;
    }
    let param = &call.params[0];

    let param = match param.kind {
        MediumLevelILLiftedInstructionKind::ConstPtr(Constant { constant: param }) => param,
        MediumLevelILLiftedInstructionKind::VarSsa(var) => {
            // Could be an indirection through __objc_classrefs
            if let Some(param) = ssa_variable_value_or_loaded_pointer(function, &var.src) {
                param
            } else {
                log::debug!(
                    "Could not determine pointer value for variable {var:?} passed as parameter of call to {} at {:#0x}",
                    target_function.symbol().full_name(),
                    instr.address
                );
                return None;
            }
        }
        _ => {
            log::warn!(
                "Unexpected parameter for call to {} at {:#0x}: {param:?}",
                target_function.symbol().full_name(),
                instr.address,
            );
            return None;
        }
    };

    let Some(param_symbol) = view.symbol_by_address(param) else {
        log::debug!(
            "No symbol for parameter {param:#0x} of call to {} at {:#0x}",
            target_function.symbol().full_name(),
            instr.address
        );
        return None;
    };

    let param_symbol_name = param_symbol.full_name().to_string();
    let class_name = if param_symbol_name.starts_with("cls_") {
        &param_symbol_name[4..]
    } else if param_symbol_name.starts_with("clsRef_") {
        &param_symbol_name[7..]
    } else if param_symbol_name.starts_with("_OBJC_CLASS_$_") {
        &param_symbol_name[14..]
    } else {
        log::debug!(
            "Unrecognized symbol name {param_symbol_name} for parameter of call to {} at {:#0x}",
            target_function.symbol().full_name(),
            instr.address
        );
        return None;
    };

    let Some(class_type) = view.type_by_name(class_name) else {
        log::warn!(
            "No type found for class {class_name} for parameter of call to {} at {:#0x}",
            target_function.symbol().full_name(),
            instr.address
        );
        return None;
    };

    Some(Type::pointer(&target_function.arch(), &class_type))
}

fn propagate_types(analysis_context: &AnalysisContext) {
    let Some(mlil) = analysis_context.mlil_function() else {
        return;
    };
    let mlil = mlil.ssa_form();

    let view = analysis_context.view();
    let func = analysis_context.function();

    for basic_block in &mlil.basic_blocks() {
        for instr in basic_block.iter() {
            let lifted = instr.lift();
            let call = match lifted.kind {
                MediumLevelILLiftedInstructionKind::CallSsa(ref call) => call,
                MediumLevelILLiftedInstructionKind::TailcallSsa(ref call) => call,
                _ => continue,
            };
            let MediumLevelILLiftedInstructionKind::ConstPtr(Constant {
                constant: call_target,
            }) = call.dest.kind
            else {
                continue;
            };

            // if let Some(_) = analysis_context
            //     .function()
            //     .call_type_adjustment(instr.address, None)
            // {
            //     log::debug!(
            //         "Call at {:#0x} already has a call type adjustment",
            //         instr.address
            //     );
            //     continue;
            // }

            let Some(target_function) =
                view.function_at(&analysis_context.function().platform(), call_target)
            else {
                log::debug!(
                    "No target function found for call to {call_target:#0x} at {:#0x}",
                    instr.address
                );
                continue;
            };

            let function_name = target_function.symbol().full_name().to_string();
            let return_type = if ALLOC_INIT_FUNCTIONS.contains(&function_name.as_str()) {
                return_type_for_alloc_function(&mlil, &lifted, call, &target_function, &view)
            } else {
                continue;
            };
            let Some(return_type) = return_type else {
                log::debug!(
                    "Could not determine new return type for call to {call_target:#0x} at {:#0x}",
                    instr.address
                );
                continue;
            };

            log::info!(
                "Overriding return type of call to {function_name} at {:#0x} to {:?}",
                instr.address,
                return_type.to_string()
            );

            let new_function_type = Type::function(
                &return_type,
                target_function.function_type().parameters().unwrap(),
                target_function
                    .function_type()
                    .has_variable_arguments()
                    .contents,
            );
            func.set_auto_call_type_adjustment(
                instr.address,
                Conf::new(&*new_function_type, 96),
                None,
            );
            func.add_tag(
                &tag_type_for_view(&view),
                "Adjusted return type of runtime call",
                Some(instr.address),
                false,
                None,
            );
        }
    }
}

fn register_activities(
    memory_management: &Activity,
    type_propagation: &Activity,
    workflow: Ref<Workflow>,
) {
    let workflow = workflow.clone_to(workflow.name());
    workflow.register_activity(memory_management).unwrap();
    workflow.register_activity(type_propagation).unwrap();

    workflow.insert(
        "core.function.generateMediumLevelIL",
        [memory_management.name()],
    );
    workflow.insert_after(
        "core.function.analyzeConstantReferences",
        [type_propagation.name()],
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

    let memory_management_config = activity::Config::action(
        OBJC_REMOVE_MEMORY_MANAGMENT_ACTIVITY_NAME,
        "Remove Objective-C memory management calls",
        "Remove calls to objc_retain / objc_release / objc_autorelease to simplify the resulting higher-level ILs",
    ).with_eligibility(activity::Eligibility::auto_with_default(false));

    let type_propagation_config = activity::Config::action(
        OBJC_TYPE_PROPAGATION_ACTIVITY_NAME,
        "Propagate Objective-C types",
        "Propagate Objective-C types to the IL",
    )
    .with_eligibility(
        // Currently disabled in DSCView due to https://github.com/Vector35/binaryninja-api/issues/6737
        activity::Eligibility::auto_with_default(false)
            .with_predicate(activity::ViewType::NotIn(&["DSCView"])),
    );

    let json = serde_json::to_string_pretty(&memory_management_config).unwrap();
    log::debug!("Registering activity: {}", json);

    let memory_management_activity =
        Activity::new_with_action(&memory_management_config, remove_memory_management);
    let type_propagation_activity =
        Activity::new_with_action(&type_propagation_config, propagate_types);

    register_activities(
        &memory_management_activity,
        &type_propagation_activity,
        Workflow::instance("core.function.metaAnalysis"),
    );
    register_activities(
        &memory_management_activity,
        &type_propagation_activity,
        Workflow::instance("core.function.objectiveC"),
    );
    register_activities(
        &memory_management_activity,
        &type_propagation_activity,
        Workflow::instance("core.function.sharedCache"),
    );

    true
}
