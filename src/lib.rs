use binaryninja::{
    binary_view::BinaryViewExt as _,
    logger::Logger,
    rc::Ref,
    workflow::{Activity, Workflow},
};
use log::LevelFilter;

use bn_bdash_extras::activity;

mod remove_memory_management;
mod type_propagation;
mod util;

fn tag_type_for_view(
    view: &binaryninja::binary_view::BinaryView,
) -> Ref<binaryninja::tags::TagType> {
    view.tag_type_by_name("Objective-C Extras")
        .unwrap_or_else(|| view.create_tag_type("Objective-C Extras", "OC"))
}

fn register_activities(
    memory_management: &Activity,
    types_alloc_init: &Activity,
    types_super_init: &Activity,
    types_msg_send_init: &Activity,
    workflow: &Workflow,
) {
    if !workflow.registered() {
        log::debug!(
            "Skipping activity registration for workflow {} as it is not registered",
            workflow.name()
        );
        return;
    }

    let workflow = workflow.clone_to(&workflow.name());
    workflow.register_activity(memory_management).unwrap();
    workflow.register_activity(types_alloc_init).unwrap();
    workflow.register_activity(types_super_init).unwrap();
    workflow.register_activity(types_msg_send_init).unwrap();

    workflow.insert(
        "core.function.generateMediumLevelIL",
        [memory_management.name()],
    );
    workflow.insert_after(
        "core.function.generateMediumLevelIL",
        [types_alloc_init.name()],
    );
    workflow.insert_after(
        "core.function.analyzeConstantReferences",
        [types_super_init.name()],
    );
    // TODO: Does this need to to have specific ordering relative to the built-in / shared cache Obj-C workflow activities?
    workflow.insert_after(
        &types_alloc_init.name(),
        [types_msg_send_init.name()],
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
        "bdash.objc-remove-memory-management",
        "Obj-C: Remove reference counting calls",
        "Remove calls to objc_retain / objc_release / objc_autorelease to simplify the resulting higher-level ILs",
    )
    .with_eligibility(activity::Eligibility::auto_with_default(false));

    let types_alloc_init_config = activity::Config::action(
        "bdash.objc-types.alloc-init",
        "Obj-C: Propagate return type from objc_alloc_init",
        "Adjust the return type of calls to objc_alloc / objc_alloc_init when a fixed type is passed as an argument.",
    )
    .with_eligibility(
        // Currently disabled in DSCView due to https://github.com/Vector35/binaryninja-api/issues/6737
        activity::Eligibility::auto_with_default(false)
            .with_predicate(activity::ViewType::NotIn(&["DSCView"])),
    );

    let types_super_init_config = activity::Config::action(
        "bdash.objc-types.super-init",
        "Obj-C: Propagate return type from [super init…]",
        "Adjust the return type of calls to objc_msgSendSuper2 where the selector is in the init family.",
    )
    .with_eligibility(activity::Eligibility::auto());

    let types_msg_send_init_config = activity::Config::action(
        "bdash.objc-types.msg-send-init",
        "Obj-C: Propagate return type from [self initWith…]",
        "Adjust the return type of calls to objc_msgSend where the selector is in the init family and the receiver type is known.",
    )
    .with_eligibility(
        // Currently disabled in DSCView due to https://github.com/Vector35/binaryninja-api/issues/6737
        activity::Eligibility::auto_with_default(false)
            .with_predicate(activity::ViewType::NotIn(&["DSCView"])),
    );

    let memory_management_activity =
        Activity::new_with_action(&memory_management_config.to_string(), remove_memory_management::action);
    let types_alloc_init_activity = Activity::new_with_action(
        &types_alloc_init_config.to_string(),
        type_propagation::alloc_init::action,
    );
    let types_super_init_activity = Activity::new_with_action(
        &types_super_init_config.to_string(),
        type_propagation::super_init::action,
    );
    let types_msg_send_init_activity = Activity::new_with_action(
        &types_msg_send_init_config.to_string(),
        type_propagation::msg_send_init::action,
    );

    register_activities(
        &memory_management_activity,
        &types_alloc_init_activity,
        &types_super_init_activity,
        &types_msg_send_init_activity,
        &Workflow::instance("core.function.metaAnalysis"),
    );
    register_activities(
        &memory_management_activity,
        &types_alloc_init_activity,
        &types_super_init_activity,
        &types_msg_send_init_activity,
        &Workflow::instance("core.function.objectiveC"),
    );
    register_activities(
        &memory_management_activity,
        &types_alloc_init_activity,
        &types_super_init_activity,
        &types_msg_send_init_activity,
        &Workflow::instance("core.function.sharedCache"),
    );

    true
}
