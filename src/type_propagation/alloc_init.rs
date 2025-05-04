use binaryninja::{
    binary_view::{BinaryView, BinaryViewExt as _},
    medium_level_il::MediumLevelILLiftedInstruction,
    rc::Ref,
    types::Type,
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

fn return_type_for_alloc_call(call: &util::Call<'_>, view: &BinaryView) -> Option<Ref<Type>> {
    if call.call.params.len() != 1 {
        return None;
    }

    let class_addr =
        util::match_constant_pointer_or_load_of_constant_pointer(&call.call.params[0])?;
    let class_symbol_name = view.symbol_by_address(class_addr)?.full_name();
    let class_name =
        util::class_name_from_symbol_name(&class_symbol_name.as_bytes_with_null().as_bstr())?;

    let class_type = view.type_by_name(class_name.to_str().ok()?)?;
    Some(Type::pointer(&call.target.arch(), &class_type))
}

fn process_instruction(instr: MediumLevelILLiftedInstruction, view: &BinaryView) -> Option<()> {
    let call = util::match_call_to_function_named(&instr, view, ALLOC_INIT_FUNCTIONS)?;

    util::adjust_return_type_of_call(
        &call,
        return_type_for_alloc_call(&call, view)?,
        view,
        "Adjusted return type of alloc / init call",
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
