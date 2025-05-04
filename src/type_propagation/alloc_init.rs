use binaryninja::{
    binary_view::{BinaryView, BinaryViewExt as _},
    confidence::Conf,
    function::Function,
    medium_level_il::{
        MediumLevelILFunction, MediumLevelILLiftedInstruction, MediumLevelILLiftedInstructionKind,
        operation::{Constant, LiftedCallSsa, LiftedLoadSsa},
    },
    rc::Ref,
    types::Type,
    variable::RegisterValueType,
    workflow::AnalysisContext,
};
use bstr::ByteSlice;

use crate::util;

// j_ prefixes are for stub functions in the dyld shared cache.
// The prefix is added by Binary Ninja's shared cache workflow.
const ALLOC_INIT_FUNCTIONS: &[&[u8]] = &[
    b"_objc_alloc_init",
    b"_objc_alloc_initWithZone",
    b"_objc_alloc",
    b"_objc_allocWithZone",
    b"_objc_opt_new",
    b"j__objc_alloc_init",
    b"j__objc_alloc_initWithZone",
    b"j__objc_alloc",
    b"j__objc_allocWithZone",
    b"j__objc_opt_new",
];

fn ssa_variable_value_or_loaded_pointer(
    function: &MediumLevelILFunction,
    var: &binaryninja::variable::SSAVariable,
) -> Option<u64> {
    let value = function.ssa_variable_value(var);
    match value.state {
        RegisterValueType::ConstantPointerValue => Some(value.value as u64),
        RegisterValueType::UndeterminedValue => {
            let def = function.ssa_variable_definition(var)?;
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
    instr: &MediumLevelILLiftedInstruction,
    call: &LiftedCallSsa,
    target_function: &Function,
    view: &BinaryView,
) -> Option<Ref<Type>> {
    if call.params.len() != 1 {
        return None;
    }

    let param = match call.params[0].kind {
        MediumLevelILLiftedInstructionKind::ConstPtr(Constant { constant: param }) => param,
        MediumLevelILLiftedInstructionKind::VarSsa(var) => {
            // Could be an indirection through __objc_classrefs
            match ssa_variable_value_or_loaded_pointer(&instr.function, &var.src) {
                Some(param) => param,
                None => return None,
            }
        }
        _ => return None,
    };

    let param_symbol_name = view.symbol_by_address(param)?.full_name();
    let class_name =
        util::class_name_from_symbol_name(&param_symbol_name.as_bytes_with_null().as_bstr())?;

    let class_type = view.type_by_name(class_name.to_str_lossy())?;

    Some(Type::pointer(&target_function.arch(), &class_type))
}

fn process_instruction(instr: MediumLevelILLiftedInstruction, view: &BinaryView) -> Option<()> {
    let call = match instr.kind {
        MediumLevelILLiftedInstructionKind::CallSsa(ref call) => call,
        MediumLevelILLiftedInstructionKind::TailcallSsa(ref call) => call,
        _ => return None,
    };

    let MediumLevelILLiftedInstructionKind::ConstPtr(Constant {
        constant: call_target,
    }) = call.dest.kind
    else {
        return None;
    };

    let target_function = view.function_at(&instr.function.function().platform(), call_target)?;

    let function_name = target_function.symbol().full_name();
    if !ALLOC_INIT_FUNCTIONS.contains(&function_name.as_bytes_with_null()) {
        return None;
    }

    let return_type = return_type_for_alloc_function(&instr, call, &target_function, view)?;

    let target_function_type = target_function.function_type();
    let function_call_type = Type::function(
        &return_type,
        target_function_type.parameters().unwrap(),
        target_function_type.has_variable_arguments().contents,
    );

    let function = instr.function.function();
    function.set_auto_call_type_adjustment(
        instr.address,
        Conf::new(&*function_call_type, 96),
        None,
    );
    function.add_tag(
        &crate::tag_type_for_view(view),
        "Adjusted return type of alloc / init call",
        Some(instr.address),
        false,
        None,
    );

    Some(())
}

pub(crate) fn action(analysis_context: &AnalysisContext) {
    let Some(mlil) = analysis_context.mlil_function() else {
        return;
    };

    let mlil_ssa = mlil.ssa_form();
    let view = analysis_context.view();

    for basic_block in &mlil_ssa.basic_blocks() {
        for instr in basic_block.iter() {
            process_instruction(instr.lift(), &view);
        }
    }
}
