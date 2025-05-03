use binaryninja::{
    binary_view::BinaryViewExt as _,
    confidence::Conf,
    medium_level_il::{
        MediumLevelILFunction, MediumLevelILLiftedInstruction, MediumLevelILLiftedInstructionKind,
        operation::{Constant, LiftedCallSsa, LiftedSetVarSsa, LiftedSetVarSsaField, Var, VarSsa},
    },
    rc::Ref,
    types::Type,
    workflow::AnalysisContext,
};
use bstr::ByteSlice as _;
use itertools::Itertools as _;

use crate::util;

// j_ prefixes are for stub functions in the dyld shared cache.
// The prefix is added by Binary Ninja's shared cache workflow.
const OBJC_MSG_SEND_SUPER_FUNCTIONS: &[&[u8]] = &[
    b"_objc_msgSendSuper",
    b"_objc_msgSendSuper2",
    b"j__objc_msgSendSuper",
    b"j__objc_msgSendSuper2",
];

fn return_type_for_super_call(
    function: &MediumLevelILFunction,
    _instr: &MediumLevelILLiftedInstruction,
    call: &LiftedCallSsa,
    target_function: &binaryninja::function::Function,
    view: &binaryninja::binary_view::BinaryView,
) -> Option<Ref<Type>> {
    // Expecting to see at least `objc_super` and a selector.
    if call.params.len() > 2 {
        return None;
    }

    let selector_param = &call.params[1];
    let selector_param = match selector_param.kind {
        MediumLevelILLiftedInstructionKind::ConstPtr(Constant { constant: param }) => param,
        _ => return None,
    };

    let Some(selector_symbol) = view.symbol_by_address(selector_param) else {
        return None;
    };

    let selector_symbol_name = selector_symbol.full_name();
    let Some(selector_name) =
        util::selector_name_from_symbol_name(&selector_symbol_name.as_bytes().as_bstr())
    else {
        return None;
    };
    if !selector_name.starts_with(b"init") {
        return None;
    }

    let super_param = &call.params[0];
    let MediumLevelILLiftedInstructionKind::VarSsa(VarSsa { src }) = super_param.kind else {
        log::debug!(
            "Unhandled super paramater format at {:#0x} {:?}",
            super_param.address,
            super_param
        );
        return None;
    };

    // Parameter is an SSA varaible. Find its definitions to find when it was assigned.
    // From there we can determine the values it was assigned.
    let Some(def) = function.ssa_variable_definition(&src) else {
        log::debug!("  could not find definition of variable?");
        return None;
    };

    let def = def.lift();
    let MediumLevelILLiftedInstructionKind::SetVarSsa(LiftedSetVarSsa { src, .. }) = def.kind
    else {
        log::error!(
            "Unhandled variable definition at {:#0x} {:?}",
            def.address,
            def
        );
        return None;
    };

    let MediumLevelILLiftedInstructionKind::AddressOf(Var { src: src_var }) = src.kind else {
        log::error!("Unexpected source of MLIL_SET_VAR_SSA");
        return None;
    };

    // `src_var` is a `struct objc_super`. Find constant values assigned to the `super_class` field (offset 8).
    let super_class_constants = function
        .var_definitions(&src_var)
        .into_iter()
        .filter_map(|def| {
            let def = def.lift();
            let MediumLevelILLiftedInstructionKind::SetVarAliasedField(LiftedSetVarSsaField {
                src,
                offset: 8,
                ..
            }) = def.kind
            else {
                return None;
            };

            let MediumLevelILLiftedInstructionKind::ConstPtr(Constant { constant }) = src.kind
            else {
                return None;
            };
            Some(constant)
        })
        .collect_vec();

    // In the common case there are either zero or one assignments to the `super_class` field.
    // If there are zero, that likely means the assigned value was not a constant. Handling
    // that is above my pay grade.
    let &[super_class_ptr] = &super_class_constants[..] else {
        log::debug!(
            "Unexpected number of assignments to super class found for {:#0x}: {:#0x?}",
            src.address,
            super_class_constants
        );
        return None;
    };

    let Some(super_class_symbol) = view.symbol_by_address(super_class_ptr) else {
        log::debug!("No symbol found for super class at {:#0x}", super_class_ptr);
        return None;
    };

    let super_class_symbol_name = super_class_symbol.full_name();
    let Some(class_name) =
        util::class_name_from_symbol_name(&super_class_symbol_name.as_bytes_with_null().as_bstr())
    else {
        log::debug!(
            "Unable to extract class name from symbol name: {:?}",
            super_class_symbol_name
        );
        return None;
    };

    let Some(class_type) = view.type_by_name(class_name.to_str_lossy()) else {
        log::debug!("No type found for class named {:?}", class_name);
        return None;
    };

    Some(Type::pointer(&target_function.arch(), &class_type))
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

            let function_name = target_function.symbol().full_name();
            if !OBJC_MSG_SEND_SUPER_FUNCTIONS.contains(&function_name.as_bytes()) {
                continue;
            }

            let Some(return_type) =
                return_type_for_super_call(&mlil, &lifted, call, &target_function, &view)
            else {
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
                "Adjusted return type of super init call",
                Some(instr.address),
                false,
                None,
            );
        }
    }
}
