//! Boundary capture must precede probes; replacements are private fixture paths.
use super::*;
use std::path::Path;

fn setup(paths: &AliasPaths) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let source = paths.dir.path().join("source");
    create_source_repo(&source);
    let bin = paths.dir.path().join("bin");
    let binary = create_exe_stub(&bin, "codex");
    let hooks = create_exe_stub(&bin, "amplihack-hooks");
    (source, binary, hooks)
}

fn env<'a>(paths: &'a AliasPaths, binary: &'a Path) -> EnvGuard {
    EnvGuard::set([
        ("HOME", paths.home.to_str().unwrap()),
        ("CODEX_HOME", paths.codex_home.to_str().unwrap()),
        ("AMPLIHACK_CODEX_BINARY_PATH", binary.to_str().unwrap()),
        ("AMPLIHACK_AGENT_BINARY", "codex"),
    ])
}

#[test]
fn explicit_codex_missing_descendants_below_stable_alias_anchor_are_created() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut paths = AliasPaths::new(true);
    paths.codex_home = paths.alias.join("user/missing/nested/codex");
    let (source, binary, hooks) = setup(&paths);
    let _env = env(&paths, &binary);
    let result = codex_plugin::install(&source, &hooks);
    assert!(
        matches!(result, Ok(true)),
        "create bounded missing descendants: {result:?}"
    );
    assert!(paths.codex_home.join("config.toml").is_file());
    assert!(!paths.root.join("pending.json").exists());
    paths.assert_alias_unchanged();
}

#[test]
fn selected_home_and_explicit_home_parent_links_are_rejected_before_probe() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for location in ["home", "codex-home", "codex-parent"] {
        selected_home_and_explicit_home_parent_links_are_rejected_before_probe_case(location);
    }
}

fn selected_home_and_explicit_home_parent_links_are_rejected_before_probe_case(location: &str) {
    let paths = AliasPaths::new(true);
    let (source, binary, hooks) = setup(&paths);
    let path = match location {
        "home" => paths.home.clone(),
        "codex-home" => paths.codex_home.clone(),
        _ => paths.codex_home.parent().unwrap().to_path_buf(),
    };
    let displaced = paths.dir.path().join("displaced");
    fs::rename(&path, &displaced).unwrap();
    std::os::unix::fs::symlink(&displaced, &path).unwrap();
    let script = fs::read_to_string(&binary).unwrap();
    fs::write(
        &binary,
        script.replace(
            "'plugin list --json')",
            "'plugin list --json')\n    printf 'probe\\n' >> \"$0.probes\"\n",
        ),
    )
    .unwrap();
    let _env = env(&paths, &binary);
    assert!(codex_plugin::install(&source, &hooks).is_err());
    assert!(
        !binary.with_extension("probes").exists(),
        "{location} must be rejected before native probe"
    );
    assert!(
        !paths.root.exists(),
        "unsafe selection must not create staging resources"
    );
    assert_eq!(fs::read_link(&path).unwrap(), displaced);
}

#[test]
fn alias_retarget_and_regular_home_replacement_during_probe_prevent_mutations() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for replacement in ["alias", "home"] {
        alias_retarget_and_regular_home_replacement_during_probe_prevent_mutations_case(
            replacement,
        );
    }
}

fn alias_retarget_and_regular_home_replacement_during_probe_prevent_mutations_case(
    replacement: &str,
) {
    let paths = AliasPaths::new(true);
    let (source, binary, hooks) = setup(&paths);
    let foreign = paths.dir.path().join("foreign");
    fs::create_dir_all(foreign.join("user/codex")).unwrap();
    fs::write(foreign.join("sentinel"), b"foreign untouched").unwrap();
    let script = fs::read_to_string(&binary).unwrap();
    let mutation = "'plugin list --json')\nif [ ! -f \"$0.changed\" ]; then\n if [ \"$ALIAS_TEST_ACTION\" = alias ]; then\n  rm -- \"$ALIAS_TEST_PATH\"; ln -s -- \"$ALIAS_TEST_FOREIGN\" \"$ALIAS_TEST_PATH\"\n else\n  mv -- \"$HOME\" \"$ALIAS_TEST_FOREIGN/original-home\"; mkdir -- \"$HOME\"\n fi\n touch \"$0.changed\"\nfi\n";
    fs::write(&binary, script.replace("'plugin list --json')", mutation)).unwrap();
    let _env = env(&paths, &binary);
    let _mutation_env = EnvGuard::set([
        ("ALIAS_TEST_ACTION", replacement),
        ("ALIAS_TEST_PATH", paths.alias.to_str().unwrap()),
        ("ALIAS_TEST_FOREIGN", foreign.to_str().unwrap()),
    ]);
    let result = codex_plugin::install(&source, &hooks);
    assert!(
        binary.with_extension("changed").exists(),
        "actual probe replacement must execute"
    );
    assert!(
        result.is_err(),
        "changed captured mapping/identity must fail"
    );
    assert!(
        !paths.root.exists(),
        "must reject before creating staging under changed anchor"
    );
    assert!(
        !foreign.join("user/codex/config.toml").exists(),
        "foreign selected home must not be configured"
    );
    assert_eq!(
        fs::read(foreign.join("sentinel")).unwrap(),
        b"foreign untouched"
    );
}

#[test]
fn explicit_home_component_escape_is_rejected_before_probe_or_creation() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut paths = AliasPaths::new(true);
    fs::create_dir(paths.alias.join("user/child")).unwrap();
    paths.codex_home = paths.alias.join("user/child/../codex");
    let (source, binary, hooks) = setup(&paths);
    let script = fs::read_to_string(&binary).unwrap();
    fs::write(
        &binary,
        script.replace(
            "'plugin list --json')",
            "'plugin list --json')\n    touch \"$0.probed\"\n",
        ),
    )
    .unwrap();
    let _env = env(&paths, &binary);
    assert!(codex_plugin::install(&source, &hooks).is_err());
    assert!(
        !binary.with_extension("probed").exists(),
        "escaping component must fail before native probe"
    );
    assert!(!paths.root.exists());
}

#[test]
fn independent_home_link() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    selected_home_and_explicit_home_parent_links_are_rejected_before_probe_case("home");
}

#[test]
fn independent_codex_home_link() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    selected_home_and_explicit_home_parent_links_are_rejected_before_probe_case("codex-home");
}

#[test]
fn independent_codex_parent_link() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    selected_home_and_explicit_home_parent_links_are_rejected_before_probe_case("codex-parent");
}

#[test]
fn independent_alias_probe_replacement() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_retarget_and_regular_home_replacement_during_probe_prevent_mutations_case("alias");
}

#[test]
fn independent_home_probe_replacement() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    alias_retarget_and_regular_home_replacement_during_probe_prevent_mutations_case("home");
}

#[test]
fn version_probe_retarget_is_rejected_before_native_inventory_or_staging() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let paths = AliasPaths::new(true);
    let (source, binary, hooks) = setup(&paths);
    let foreign = paths.dir.path().join("foreign-version");
    fs::create_dir_all(foreign.join("user/codex")).unwrap();
    let script = fs::read_to_string(&binary).unwrap();
    fs::write(&binary, script.replace("'--version  ')", "'--version  ')\nif [ ! -f \"$0.version-changed\" ]; then\n rm -- \"$ALIAS_TEST_PATH\"; ln -s -- \"$ALIAS_TEST_FOREIGN\" \"$ALIAS_TEST_PATH\"\n touch \"$0.version-changed\"\nfi\n")).unwrap();
    let _env = env(&paths, &binary);
    let _mutation = EnvGuard::set([
        ("ALIAS_TEST_PATH", paths.alias.to_str().unwrap()),
        ("ALIAS_TEST_FOREIGN", foreign.to_str().unwrap()),
    ]);
    let result = codex_plugin::install(&source, &hooks);
    assert!(
        binary.with_extension("version-changed").exists(),
        "version probe must actually execute"
    );
    assert!(
        result.is_err(),
        "version-probe replacement must not be recaptured"
    );
    assert!(!paths.root.exists());
    assert!(!foreign.join("user/codex/config.toml").exists());
}
