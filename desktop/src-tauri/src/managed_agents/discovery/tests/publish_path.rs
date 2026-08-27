use super::super::*;

/// RAII guard: installs the pre-publish hook, clears it on drop (even on
/// panic) so a failing test cannot poison later ones.
struct PrePublishHookGuard;

impl PrePublishHookGuard {
    fn install(hook: Box<dyn Fn() + Send>) -> Self {
        super::super::pre_publish_test_hook::set(Some(hook));
        Self
    }
}

impl Drop for PrePublishHookGuard {
    fn drop(&mut self) {
        super::super::pre_publish_test_hook::set(None);
    }
}

fn harness_def(
    id: &str,
    label: &str,
    command: &str,
) -> crate::managed_agents::custom_harnesses::HarnessDefinition {
    crate::managed_agents::custom_harnesses::HarnessDefinition {
        id: id.to_string(),
        label: label.to_string(),
        command: command.to_string(),
        args: vec![],
        env: Default::default(),
        install_instructions_url: String::new(),
        install_hint: String::new(),
    }
}

#[test]
fn discovery_publish_path_survives_mid_flight_save() {
    use crate::managed_agents::custom_harnesses::{
        lookup_loaded_harness_by_id, registry_test_lock, save_and_warm,
    };

    let _path_guard = crate::managed_agents::lock_path_mutex();
    let _lock = registry_test_lock();
    let dir = tempfile::tempdir().unwrap();
    let hook_dir = dir.path().to_path_buf();
    let _guard = PrePublishHookGuard::install(Box::new(move || {
        let def = harness_def("mid-flight-save", "Mid Flight", "mid-flight-bin");
        save_and_warm(&hook_dir, &def, None).unwrap();
        assert!(lookup_loaded_harness_by_id("mid-flight-save").is_some());
    }));

    let _entries = discover_acp_runtimes_from(Some(dir.path()), true);

    assert!(
        lookup_loaded_harness_by_id("mid-flight-save").is_some(),
        "discovery's publish must re-read the directory — a stale-snapshot \
         publish clobbers a save that landed mid-discovery"
    );
}

#[test]
fn discovery_publish_path_drops_mid_flight_delete() {
    use crate::managed_agents::custom_harnesses::{
        delete_and_warm, lookup_loaded_harness_by_id, registry_test_lock, save_and_warm,
    };

    let _path_guard = crate::managed_agents::lock_path_mutex();
    let _lock = registry_test_lock();
    let dir = tempfile::tempdir().unwrap();
    let def = harness_def("mid-flight-delete", "Mid Flight Del", "mid-flight-del-bin");
    save_and_warm(dir.path(), &def, None).unwrap();

    let hook_dir = dir.path().to_path_buf();
    let _guard = PrePublishHookGuard::install(Box::new(move || {
        delete_and_warm(&hook_dir, "mid-flight-delete").unwrap();
        assert!(lookup_loaded_harness_by_id("mid-flight-delete").is_none());
    }));

    let _entries = discover_acp_runtimes_from(Some(dir.path()), true);

    assert!(
        lookup_loaded_harness_by_id("mid-flight-delete").is_none(),
        "discovery's publish must not resurrect a harness deleted mid-discovery"
    );
}
