use binaryninja::{
    binary_view::{BinaryView, BinaryViewExt as _},
    medium_level_il::{
        MediumLevelILLiftedInstruction, MediumLevelILLiftedInstructionKind, operation::VarSsa,
    },
    types::TypeClass,
    workflow::AnalysisContext,
};
use bstr::ByteSlice as _;

use crate::util;

// j_ prefixes are for stub functions in the dyld shared cache.
// The prefix is added by Binary Ninja's shared cache workflow.
const OBJC_MSG_SEND_FUNCTIONS: &[&[u8]] = &[b"_objc_msgSend", b"j__objc_msgSend"];

fn process_instruction(instr: &MediumLevelILLiftedInstruction, view: &BinaryView) -> Option<()> {
    let call = util::match_call_to_function_named(instr, view, OBJC_MSG_SEND_FUNCTIONS)?;

    // Expecting to see at least the receiver and a selector.
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

    let receiver = &call.call.params[0];
    let MediumLevelILLiftedInstructionKind::VarSsa(VarSsa { src }) = receiver.kind else {
        log::info!(
            "Unhandled receiver parameter format at {:#0x} {:?}",
            receiver.address,
            receiver
        );
        return None;
    };
    let receiver_type = instr
        .function
        .function()
        .variable_type(&src.variable)?
        .contents;
    if receiver_type.type_class() != TypeClass::PointerTypeClass {
        // This is often `id`.
        return None;
    }

    let underlying_receiver_type = receiver_type.target()?.contents;
    let underlying_receiver_type = match underlying_receiver_type.type_class() {
        TypeClass::NamedTypeReferenceClass => underlying_receiver_type
            .get_named_type_reference()?
            .target(view)?,
        TypeClass::StructureTypeClass => underlying_receiver_type,
        _ => return None,
    };
    let registered_name = underlying_receiver_type.registered_name()?;
    if registered_name.name().to_string() == "objc_object" {
        return None;
    }

    log::warn!(
        "Overriding return type of call to {} at {:#0x} (within {:?} {:#0x}) to {}",
        call.target_name,
        call.instr.address,
		instr.function.function().symbol().full_name(),
		instr.function.function().start(),
        receiver_type,
    );
    util::adjust_return_type_of_call(
        &call,
        &receiver_type,
        view,
        "Adjusted return type of -init call",
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
