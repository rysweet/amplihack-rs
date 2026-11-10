//! Subprocess fixture contracts: inspect the actual child command without
//! executing an unprotected CLI or staging any host binary in the red phase.
use std::ffi::OsStr;
use std::path::Path;

#[test]
fn subprocess_fixture_owns_all_application_roots() {
    let command = crate::isolated_amplihack::isolated("amplihack");
    let vars: std::collections::HashMap<_, _> = command.get_envs().collect();
    let home = vars
        .get(OsStr::new("HOME"))
        .and_then(|value| *value)
        .expect("fixture must explicitly set HOME");
    for key in [
        "AMPLIHACK_HOME",
        "CLAUDE_CONFIG_DIR",
        "AMPLIHACK_CLAUDE_PROFILE_ROOT",
    ] {
        let root = vars
            .get(OsStr::new(key))
            .and_then(|value| *value)
            .unwrap_or_else(|| panic!("fixture must explicitly own {key}"));
        assert!(
            Path::new(root).starts_with(Path::new(home)),
            "{key} must be inside the private HOME"
        );
    }
}

#[test]
fn subprocess_fixture_does_not_inherit_host_resolver_discovery() {
    let command = crate::isolated_amplihack::isolated("amplihack");
    let vars: std::collections::HashMap<_, _> = command.get_envs().collect();
    let path = vars
        .get(OsStr::new("PATH"))
        .and_then(|value| *value)
        .expect("fixture must explicitly control PATH");
    let host_path = std::env::var_os("PATH").expect("test runner PATH");
    assert_ne!(
        path, host_path,
        "inheriting the runner PATH permits host resolver discovery"
    );
}
