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
