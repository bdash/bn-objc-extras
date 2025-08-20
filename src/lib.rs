use binaryninja::{
    binary_view::BinaryViewExt as _,
    logger::Logger,
    rc::Ref,
    workflow::{Activity, Workflow, activity},
};
use log::LevelFilter;

mod remove_memory_management;
mod type_propagation;
mod util;

const OBJC_REMOVE_MEMORY_MANAGMENT_ACTIVITY_NAME: &str = "bdash.objc-remove-memory-management";
const OBJC_TYPES_ALLOC_INIT_ACTIVITY_NAME: &str = "bdash.objc-types.alloc-init";
const OBJC_TYPES_SUPER_INIT_ACTIVITY_NAME: &str = "bdash.objc-types.super-init";

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
    workflow: &str,
) -> Result<(), ()> {
    let Some(workflow) = Workflow::cloned(workflow) else {
        log::debug!(
            "Skipping activity registration for workflow {workflow} as it is not registered"
        );
        return Ok(());
    };

    workflow
        .register_activity(memory_management)?
        .register_activity(types_alloc_init)?
        .register_activity(types_super_init)?
        .insert(
            "core.function.generateMediumLevelIL",
            [memory_management.name()],
        )?
        .insert_after(
            "core.function.analyzeConstantReferences",
            [types_alloc_init.name()],
        )?
        .insert_after(
            "core.function.analyzeConstantReferences",
            [types_super_init.name()],
        )?
        .register()?;
    Ok(())
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
        "Obj-C: Remove reference counting calls",
        "Remove calls to objc_retain / objc_release / objc_autorelease to simplify the resulting higher-level ILs",
    )
    .eligibility(activity::Eligibility::auto_with_default(false));

    let types_alloc_init_config = activity::Config::action(
        OBJC_TYPES_ALLOC_INIT_ACTIVITY_NAME,
        "Obj-C: Propagate return type from objc_alloc_init",
        "Adjust the return type of calls to objc_alloc / objc_alloc_init when a fixed type is passed as an argument.",
    )
    .eligibility(
        // Currently disabled in DSCView due to https://github.com/Vector35/binaryninja-api/issues/6737
        activity::Eligibility::auto_with_default(false)
            .predicate(activity::ViewType::not_in(["DSCView"])),
    );

    let types_super_init_config = activity::Config::action(
        OBJC_TYPES_SUPER_INIT_ACTIVITY_NAME,
        "Obj-C: Propagate return type from [super init…]",
        "Adjust the return type of calls to objc_msgSendSuper2 where the selector is in the init family.",
    )
    .eligibility(activity::Eligibility::auto());

    let memory_management_activity =
        Activity::new_with_action(&memory_management_config, remove_memory_management::action);
    let types_alloc_init_activity = Activity::new_with_action(
        &types_alloc_init_config,
        type_propagation::alloc_init::action,
    );
    let types_super_init_activity = Activity::new_with_action(
        &types_super_init_config,
        type_propagation::super_init::action,
    );

    if register_activities(
        &memory_management_activity,
        &types_alloc_init_activity,
        &types_super_init_activity,
        "core.function.metaAnalysis",
    )
    .is_err()
    {
        log::debug!("Failed to register activities for workflow core.function.metaAnalysis");
    }

    if register_activities(
        &memory_management_activity,
        &types_alloc_init_activity,
        &types_super_init_activity,
        "core.function.objectiveC",
    )
    .is_err()
    {
        log::debug!("Failed to register activities for workflow core.function.objectiveC");
    }

    true
}
