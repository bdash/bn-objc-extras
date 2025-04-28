use binaryninja::{
    binary_view::BinaryViewExt as _,
    confidence::Conf,
    medium_level_il::{
        MediumLevelILFunction, MediumLevelILLiftedInstruction, MediumLevelILLiftedInstructionKind,
        operation::{Constant, LiftedCallSsa, LiftedLoadSsa},
    },
    rc::Ref,
    types::Type,
    variable::RegisterValueType,
    workflow::AnalysisContext,
};

// j_ prefixes are for stub functions in the dyld shared cache.
// The prefix is added by Binary Ninja's shared cache workflow.
const ALLOC_INIT_FUNCTIONS: &[&str] = &[
    "_objc_alloc_init",
    "_objc_alloc_initWithZone",
    "_objc_alloc",
    "_objc_allocWithZone",
    "_objc_opt_new",
    "j__objc_alloc_init",
    "j__objc_alloc_initWithZone",
    "j__objc_alloc",
    "j__objc_allocWithZone",
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
    _instr: &MediumLevelILLiftedInstruction,
    call: &LiftedCallSsa,
    target_function: &binaryninja::function::Function,
    view: &binaryninja::binary_view::BinaryView,
) -> Option<Ref<Type>> {
    if call.params.len() != 1 {
        return None;
    }
    let param = &call.params[0];

    let param = match param.kind {
        MediumLevelILLiftedInstructionKind::ConstPtr(Constant { constant: param }) => param,
        MediumLevelILLiftedInstructionKind::VarSsa(var) => {
            // Could be an indirection through __objc_classrefs
            match ssa_variable_value_or_loaded_pointer(function, &var.src) {
                Some(param) => param,
                None => return None,
            }
        }
        _ => return None,
    };

    let Some(param_symbol) = view.symbol_by_address(param) else {
        return None;
    };

    let param_symbol_name = param_symbol.full_name().to_string();
    let Some(class_name) = class_name_from_symbol_name(&param_symbol_name) else { return None };

    let Some(class_type) = view.type_by_name(class_name) else {
        return None;
    };

    Some(Type::pointer(&target_function.arch(), &class_type))
}

fn class_name_from_symbol_name(symbol_name: &str) -> Option<&str> {
    // The symbol name for the `objc_class_t` can have different names depending
    // on factors such as being local or external, and whether the reference
    // is from the shared cache or a standalone Mach-O file.
    Some(if symbol_name.starts_with("cls_") {
        &symbol_name[4..]
    } else if symbol_name.starts_with("clsRef_") {
        &symbol_name[7..]
    } else if symbol_name.starts_with("_OBJC_CLASS_$_") {
        &symbol_name[14..]
    } else {
        return None;
    })
}

pub(crate) fn action(analysis_context: &AnalysisContext) {
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

            let Some(target_function) =
                view.function_at(&analysis_context.function().platform(), call_target)
            else {
                continue;
            };

            let function_name = target_function.symbol().full_name().to_string();
            let return_type = if ALLOC_INIT_FUNCTIONS.contains(&function_name.as_str()) {
                return_type_for_alloc_function(&mlil, &lifted, call, &target_function, &view)
            } else {
                continue;
            };
            let Some(return_type) = return_type else {
                continue;
            };

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
                &crate::tag_type_for_view(&view),
                "Adjusted return type of runtime call",
                Some(instr.address),
                false,
                None,
            );
        }
    }
}
