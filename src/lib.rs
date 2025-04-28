use binaryninja::{
    binary_view::BinaryViewExt as _,
    logger::Logger,
    rc::Ref,
    workflow::{Activity, Workflow},
};
use log::LevelFilter;

// TODO: Extract these into a helper crate
mod activity;
mod llil;

mod remove_memory_management;
mod type_propagation;

const OBJC_REMOVE_MEMORY_MANAGMENT_ACTIVITY_NAME: &str = "bdash.objc-remove-memory-management";
const OBJC_TYPE_PROPAGATION_ACTIVITY_NAME: &str = "bdash.objc-type-propagation";

fn tag_type_for_view(
    view: &binaryninja::binary_view::BinaryView,
) -> Ref<binaryninja::tags::TagType> {
    view.tag_type_by_name("Objective-C Extras")
        .unwrap_or_else(|| view.create_tag_type("Objective-C Extras", "OC"))
}

fn register_activities(
    memory_management: &Activity,
    type_propagation: &Activity,
    workflow: Ref<Workflow>,
) {
    let workflow = workflow.clone_to(workflow.name());
    workflow.register_activity(memory_management).unwrap();
    workflow.register_activity(type_propagation).unwrap();

    workflow.insert(
        "core.function.generateMediumLevelIL",
        [memory_management.name()],
    );
    workflow.insert_after(
        "core.function.analyzeConstantReferences",
        [type_propagation.name()],
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
        OBJC_REMOVE_MEMORY_MANAGMENT_ACTIVITY_NAME,
        "Remove Objective-C memory management calls",
        "Remove calls to objc_retain / objc_release / objc_autorelease to simplify the resulting higher-level ILs",
    )
    .with_eligibility(activity::Eligibility::auto_with_default(false));

    let type_propagation_config = activity::Config::action(
        OBJC_TYPE_PROPAGATION_ACTIVITY_NAME,
        "Propagate Objective-C types",
        "Propagate Objective-C types to the IL",
    )
    .with_eligibility(
        // Currently disabled in DSCView due to https://github.com/Vector35/binaryninja-api/issues/6737
        activity::Eligibility::auto_with_default(false)
            .with_predicate(activity::ViewType::NotIn(&["DSCView"])),
    );

    let json = serde_json::to_string_pretty(&memory_management_config).unwrap();
    log::debug!("Registering activity: {}", json);

    let memory_management_activity =
        Activity::new_with_action(&memory_management_config, remove_memory_management::action);
    let type_propagation_activity = Activity::new_with_action(
        &type_propagation_config,
        type_propagation::alloc_init::action,
    );

    register_activities(
        &memory_management_activity,
        &type_propagation_activity,
        Workflow::instance("core.function.metaAnalysis"),
    );
    register_activities(
        &memory_management_activity,
        &type_propagation_activity,
        Workflow::instance("core.function.objectiveC"),
    );
    register_activities(
        &memory_management_activity,
        &type_propagation_activity,
        Workflow::instance("core.function.sharedCache"),
    );

    true
}
