//! Identity inputs are resolved inside the native Cargo context. No toolchain
//! probes, application code, real HOME, or network are involved in these tests.
use lbug_cache::{compatibility, config, identity};
use std::collections::BTreeMap;

#[test]
fn key_uses_the_resolved_lock_entry_instead_of_the_manifest_range() {
    let lock = r#"version = 4
[[package]]
name = "not-lbug"
version = "9.0.0"
[[package]]
name = "lbug"
version = "0.15.4"
"#;
    assert_eq!(identity::locked_lbug_version(lock).unwrap(), "0.15.4");
    for invalid in ["", "[[package]]\nname = \"lbug\"", &format!("{lock}{lock}")] {
        assert!(identity::locked_lbug_version(invalid).is_err());
    }
}

#[test]
fn each_key_component_changes_artifact_selection() {
    let base = identity::artifact_key("0.15.4", "aarch64-apple-darwin", "appleclang", 17).unwrap();
    assert_eq!(base, "lbug-0.15.4-aarch64-apple-darwin-appleclang17");
    for (version, target, family, major) in [
        ("0.15.5", "aarch64-apple-darwin", "appleclang", 17),
        ("0.15.4", "x86_64-apple-darwin", "appleclang", 17),
        ("0.15.4", "aarch64-apple-darwin", "clang", 17),
        ("0.15.4", "aarch64-apple-darwin", "appleclang", 18),
    ] {
        assert_ne!(
            base,
            identity::artifact_key(version, target, family, major).unwrap()
        );
    }
}

#[test]
fn compiler_banner_distinguishes_apple_upstream_and_gcc_without_guessing() {
    for (banner, family, major) in [
        (
            "Apple clang version 17.0.0 (clang-1700.0.13.5)",
            "appleclang",
            17,
        ),
        ("clang version 21.1.2", "clang", 21),
        ("g++ (GCC) 15.2.1 20260101", "gcc", 15),
        ("g++ (Ubuntu 15.2.0-1ubuntu1) 15.2.0", "gcc", 15),
    ] {
        let compiler = identity::parse_compiler_banner(banner).unwrap();
        assert_eq!((compiler.family.as_str(), compiler.major), (family, major));
    }
    for banner in [
        "",
        "Xcode 26.0",
        "a custom compiler 17",
        "Apple clang version ?",
    ] {
        assert!(identity::parse_compiler_banner(banner).is_err(), "{banner}");
    }
}

#[test]
fn cross_compiler_override_uses_pinned_cc_precedence_in_the_target_context() {
    let target = "aarch64-unknown-linux-gnu";
    let host = "x86_64-unknown-linux-gnu";
    let mut env = BTreeMap::from([
        ("CXX".into(), "host-c++".into()),
        ("HOST_CXX".into(), "explicit-host-c++".into()),
        ("TARGET_CXX".into(), "target-c++".into()),
        (
            "CXX_aarch64_unknown_linux_gnu".into(),
            "underscore-c++".into(),
        ),
        ("CXX_aarch64-unknown-linux-gnu".into(), "hyphen-c++".into()),
    ]);
    assert_eq!(
        identity::compiler_command(target, host, &env).unwrap(),
        "hyphen-c++"
    );
    env.remove("CXX_aarch64-unknown-linux-gnu");
    assert_eq!(
        identity::compiler_command(target, host, &env).unwrap(),
        "underscore-c++"
    );
    env.remove("CXX_aarch64_unknown_linux_gnu");
    assert_eq!(
        identity::compiler_command(target, host, &env).unwrap(),
        "target-c++"
    );
    assert_eq!(
        identity::compiler_command(host, host, &env).unwrap(),
        "explicit-host-c++"
    );
}

#[test]
fn artifact_key_rejects_path_and_control_injection() {
    for target in ["../target", "/tmp/target", "aarch64\nforged", "a/b", ""] {
        assert!(identity::artifact_key("0.15.4", target, "clang", 17).is_err());
    }
    assert!(identity::artifact_key("0.15.4", "aarch64-apple-darwin", "unknown", 17).is_err());
    assert!(identity::artifact_key("0.15.4", "aarch64-apple-darwin", "clang", 0).is_err());
}

#[test]
fn cache_root_precedence_never_redirects_an_invalid_explicit_root() {
    let mut env = BTreeMap::from([
        ("HOME".into(), "/owned/home".into()),
        ("XDG_CACHE_HOME".into(), "/owned/xdg".into()),
        ("AMPLIHACK_LBUG_CACHE_DIR".into(), "/owned/explicit".into()),
    ]);
    assert_eq!(
        config::cache_root(&env).unwrap().to_str(),
        Some("/owned/explicit")
    );
    env.insert("AMPLIHACK_LBUG_CACHE_DIR".into(), "relative/cache".into());
    assert!(config::cache_root(&env).is_err());
    env.remove("AMPLIHACK_LBUG_CACHE_DIR");
    assert_eq!(
        config::cache_root(&env).unwrap().to_str(),
        Some("/owned/xdg/amplihack/lbug")
    );
    env.remove("XDG_CACHE_HOME");
    assert_eq!(
        config::cache_root(&env).unwrap().to_str(),
        Some("/owned/home/.cache/amplihack/lbug")
    );
}

#[test]
fn compatibility_requires_affirmative_coverage_of_every_native_setting() {
    // The test policy has no deployment authority. Production policy must be
    // approved from native evidence before an asset can be selected/produced.
    let approved = BTreeMap::from([
        ("cpu".into(), "generic".into()),
        ("deployment".into(), "test-baseline".into()),
        ("runtime".into(), "libc++".into()),
        ("abi".into(), "test-abi".into()),
        ("sdk".into(), "test-sdk".into()),
        ("features".into(), "default".into()),
        ("macros".into(), "NDEBUG".into()),
        ("flags".into(), "-O3".into()),
    ]);
    assert!(compatibility::settings_match(&approved, &approved));
    for name in approved.keys() {
        let mut requested = approved.clone();
        requested.insert(name.clone(), "unapproved".into());
        assert!(
            !compatibility::settings_match(&approved, &requested),
            "{name}"
        );
        requested.remove(name);
        assert!(
            !compatibility::settings_match(&approved, &requested),
            "missing {name}"
        );
    }
    assert!(!compatibility::settings_match(
        &BTreeMap::new(),
        &BTreeMap::new()
    ));
    let mut custom = approved.clone();
    custom.insert(
        "extra_cxxflags".into(),
        "-march=native -fsanitize=address".into(),
    );
    assert!(!compatibility::settings_match(&approved, &custom));
}
