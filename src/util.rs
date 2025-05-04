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
    variable::{RegisterValueType, SSAVariable},
};
use bstr::BStr;

pub(crate) fn class_name_from_symbol_name(symbol_name: &BStr) -> Option<&BStr> {
    // The symbol name for the `objc_class_t` can have different names depending
    // on factors such as being local or external, and whether the reference
    // is from the shared cache or a standalone Mach-O file.
    Some(if symbol_name.starts_with(b"cls_") {
        &symbol_name[4..]
    } else if symbol_name.starts_with(b"clsRef_") {
        &symbol_name[7..]
    } else if symbol_name.starts_with(b"_OBJC_CLASS_$_") {
        &symbol_name[14..]
    } else {
        return None;
    })
}

pub(crate) fn selector_name_from_symbol_name(symbol_name: &BStr) -> Option<&BStr> {
    Some(if symbol_name.starts_with(b"sel_") {
        &symbol_name[4..]
    } else {
        return None;
    })
}

pub(crate) struct Call<'a> {
    pub instr: &'a MediumLevelILLiftedInstruction,
    pub call: &'a LiftedCallSsa,
    pub target: Ref<Function>,
}

/// Returns a `Call` if `instr` is a call or tail call to a function whose name appears in `function_names`
pub(crate) fn match_call_to_function_named<'a>(
    instr: &'a MediumLevelILLiftedInstruction,
    view: &'a BinaryView,
    function_names: &'a [&[u8]],
) -> Option<Call<'a>> {
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
    if !function_names.contains(&function_name.as_bytes()) {
        return None;
    }

    Some(Call {
        instr,
        call,
        target: target_function,
    })
}

/// Adjust the return type of the call represented by `call`
///
/// A tag is added at the call instruction with the given description.
pub(crate) fn adjust_return_type_of_call(
    call: &Call<'_>,
    return_type: Ref<Type>,
    view: &BinaryView,
    tag_description: &str,
) {
    let function = call.instr.function.function();
    let target_function_type = if let Some(existing_call_type_adjustment) =
        function.call_type_adjustment(call.instr.address, None)
    {
        existing_call_type_adjustment.contents
    } else {
        call.target.function_type()
    };

    let adjusted_call_type = Type::function(
        &return_type,
        target_function_type.parameters().unwrap(),
        target_function_type.has_variable_arguments().contents,
    );

    function.set_auto_call_type_adjustment(
        call.instr.address,
        Conf::new(&*adjusted_call_type, 192),
        None,
    );
    function.add_tag(
        &crate::tag_type_for_view(view),
        tag_description,
        Some(call.instr.address),
        false,
        None,
    );
}

fn ssa_variable_value_or_load_of_constant_pointer(
    function: &MediumLevelILFunction,
    var: &SSAVariable,
) -> Option<u64> {
    let value = function.ssa_variable_value(var);
    match value.state {
        RegisterValueType::ConstantPointerValue => return Some(value.value as u64),
        RegisterValueType::UndeterminedValue => {}
        _ => return None,
    }

    let def = function.ssa_variable_definition(var)?;
    let MediumLevelILLiftedInstructionKind::SetVarSsa(set_var) = def.lift().kind else {
        return None;
    };

    let MediumLevelILLiftedInstructionKind::LoadSsa(LiftedLoadSsa { src, .. }) = set_var.src.kind
    else {
        return None;
    };

    match src.kind {
        MediumLevelILLiftedInstructionKind::ConstPtr(Constant { constant }) => Some(constant),
        _ => None,
    }
}

/// If `instr` is a constant pointer or is a variable whose value is loaded from a constant pointer,
/// return that pointer address.
pub(crate) fn match_constant_pointer_or_load_of_constant_pointer(
    instr: &MediumLevelILLiftedInstruction,
) -> Option<u64> {
    match instr.kind {
        MediumLevelILLiftedInstructionKind::ConstPtr(Constant { constant }) => Some(constant),
        MediumLevelILLiftedInstructionKind::VarSsa(var) => {
            ssa_variable_value_or_load_of_constant_pointer(&instr.function, &var.src)
        }
        _ => None,
    }
}
