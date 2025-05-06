use binaryninja::{
    binary_view::{BinaryView, BinaryViewExt as _},
    medium_level_il::{
        MediumLevelILLiftedInstruction, MediumLevelILLiftedInstructionKind,
        operation::{Constant, LiftedSetVarSsa, LiftedSetVarSsaField, LiftedVarPhi, Var, VarSsa},
    },
    rc::Ref,
    types::Type,
    workflow::AnalysisContext,
};
use bstr::ByteSlice as _;

use crate::util;

// j_ prefixes are for stub functions in the dyld shared cache.
// The prefix is added by Binary Ninja's shared cache workflow.
const OBJC_MSG_SEND_SUPER_FUNCTIONS: &[&[u8]] = &[
    b"_objc_msgSendSuper",
    b"_objc_msgSendSuper2",
    b"j__objc_msgSendSuper",
    b"j__objc_msgSendSuper2",
];

fn return_type_for_super_call(call: &util::Call, view: &BinaryView) -> Option<Ref<Type>> {
    // Expecting to see at least `objc_super` and a selector.
    if call.call.params.len() < 2 {
        return None;
    }

    let selector_addr =
        util::match_constant_pointer_or_load_of_constant_pointer(&call.call.params[1])?;
    let selector_symbol_name = view.symbol_by_address(selector_addr)?.full_name();
    let selector_name =
        util::selector_name_from_symbol_name(selector_symbol_name.to_bytes().as_bstr())?;

    if !selector_name.starts_with(b"init") {
        return None;
    }

    let super_param = &call.call.params[0];
    let MediumLevelILLiftedInstructionKind::VarSsa(VarSsa {
        src: super_param_var,
    }) = super_param.kind
    else {
        log::debug!(
            "Unhandled super paramater format at {:#0x} {:?}",
            super_param.address,
            super_param
        );
        return None;
    };

    // Parameter is an SSA variable. Find its definitions to find when it was assigned.
    // From there we can determine the values it was assigned.
    let Some(super_param_def) = call
        .instr
        .function
        .ssa_variable_definition(&super_param_var)
    else {
        log::debug!("  could not find definition of variable?");
        return None;
    };

    let src = match super_param_def.lift().kind {
        MediumLevelILLiftedInstructionKind::SetVarSsa(LiftedSetVarSsa { src, .. }) => src,
        MediumLevelILLiftedInstructionKind::VarPhi(LiftedVarPhi { .. }) => {
            // The Swift compiler generates code that conditionally assigns to the receiver field of `objc_super`.
            // TODO: Recognize that pattern and handle it.
            log::debug!(
                "  found phi node for definition of `objc_super` variable at {:#0x} {:?}",
                super_param_def.address,
                super_param_def
            );
            return None;
        }
        _ => {
            log::error!(
                "Unexpected variable definition kind at {:#0x} {:#x?}",
                super_param_def.address,
                super_param_def
            );
            return None;
        }
    };

    let src_var = match src.kind {
        MediumLevelILLiftedInstructionKind::AddressOf(Var { src: src_var }) => src_var,
        MediumLevelILLiftedInstructionKind::VarSsa(_)
        | MediumLevelILLiftedInstructionKind::Sub(_) => {
            // The Swift compiler generates code that initializes the `objc_super` variable in more varied ways.
            log::debug!(
                "  found non-address-of variable definition of `objc_super` variable at {:#0x} {:?}",
                super_param_def.address,
                super_param_def
            );
            return None;
        }
        _ => {
            log::error!(
                "Unexpected source of variable definition at {:#0x} {:x?}",
                super_param_def.address,
                super_param_def
            );
            return None;
        }
    };

    // `src_var` is a `struct objc_super`. Find constant values assigned to the `super_class` field (offset 8).
    let super_class_constants: Vec<_> = call
        .instr
        .function
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
        .collect();

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
        log::debug!("No symbol found for super class at {super_class_ptr:#0x}");
        return None;
    };

    let super_class_symbol_name = super_class_symbol.full_name();
    let Some(class_name) =
        util::class_name_from_symbol_name(super_class_symbol_name.to_bytes().as_bstr())
    else {
        log::debug!("Unable to extract class name from symbol name: {super_class_symbol_name:?}");
        return None;
    };

    let Some(class_type) = view.type_by_name(class_name.to_str_lossy()) else {
        log::debug!("No type found for class named {class_name:?}");
        return None;
    };

    Some(Type::pointer(&call.arch, &class_type))
}

fn process_instruction(instr: &MediumLevelILLiftedInstruction, view: &BinaryView) -> Option<()> {
    let call = util::match_call_to_function_named(instr, view, OBJC_MSG_SEND_SUPER_FUNCTIONS)?;

    util::adjust_return_type_of_call(
        &call,
        return_type_for_super_call(&call, view)?.as_ref(),
        view,
        "Adjusted return type of super init call",
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
            process_instruction(&instr.lift(), &view);
        }
    }
}
