//! Issue #1877 (TDD): profiles inherit the `env` block of the shared
//! `settings.json` or of `profile.settings_template`; the profile's own keys
//! win. Every fixture uses a temp source, a temp HOME and a temp profile
//! root, and asserts none of them is the real `~/.claude`.
//!
//! Contract (`settings_env`):
//! - `merge_env(&mut settings, &source_env)`: pure; per key inside `env`,
//!   profile keys win, missing keys are added, keys outside `env` and a
//!   non-object profile `env` are left alone.
//! - `warning(name, path, reason)` and `key_warning(name, key, path, reason)`:
//!   the two fixed warning forms; never an env value.
//! - `ensure_for(store, name, home)` (add, login, switch, adoption) and
//!   `ensure_all(store, home)` (repair) return [`Outcome`]: a [`Refresh`]
//!   plus the warnings. `Skipped` has exactly one warning, `Unchanged` none.
//!   `env` is not seeded by `store.add`, so a new profile gets it only here.

use super::name::ProfileName;
use super::settings_env::{Outcome, ensure_all, ensure_for, key_warning, merge_env, warning};
use super::settings_refresh::Refresh;
use super::store::ProfileStore;
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const SECRET: &str = "sk-ant-SENTINEL-1877";

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        other => panic!("not an object: {other}"),
    }
}

#[cfg(unix)]
fn chmod(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}
#[cfg(not(unix))]
fn chmod(_p: &Path, _mode: u32) {}

struct Fx {
    d: tempfile::TempDir,
    source: PathBuf,
    home: PathBuf,
    store: ProfileStore,
}

impl Fx {
    fn new() -> Self {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().canonicalize().unwrap();
        let source = base.join("main-claude");
        let home = base.join("home");
        for dir in [&source, &home] {
            fs::create_dir_all(dir).unwrap();
            chmod(dir, 0o700);
        }
        let root = base.join("profiles");
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        assert!(source.starts_with(&tmp), "source must be under temp_dir");
        assert!(root.starts_with(&tmp), "root must be under temp_dir");
        if let Some(real) = std::env::var_os("HOME") {
            let real = PathBuf::from(real);
            assert_ne!(source, real.join(".claude"), "never the live ~/.claude");
            assert_ne!(home, real, "never the real HOME");
        }
        let store = ProfileStore::new(root).with_shared_assets(source.clone());
        Self {
            d,
            source,
            home,
            store,
        }
    }

    fn home(&self) -> Option<&Path> {
        Some(&self.home)
    }

    fn source_file(&self) -> PathBuf {
        self.source.join("settings.json")
    }

    /// Writes the shared `settings.json` with mode 0600 (umask-independent).
    fn source_settings(&self, body: &str) {
        fs::write(self.source_file(), body).unwrap();
        chmod(&self.source_file(), 0o600);
    }

    /// Adds a profile (with no shared settings yet, so no seeded file), then
    /// writes its `settings.json` when `body` is given.
    fn profile(&self, name: &str, body: Option<&str>) -> PathBuf {
        let dir = self.store.add(&n(name)).unwrap();
        let path = dir.join("settings.json");
        let _ = fs::remove_file(&path);
        if let Some(body) = body {
            fs::write(&path, body).unwrap();
            chmod(&path, 0o600);
        }
        dir
    }

    fn read(&self, name: &str) -> Value {
        let path = self.store.root().join(name).join("settings.json");
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn raw(&self, name: &str) -> Vec<u8> {
        fs::read(self.store.root().join(name).join("settings.json")).unwrap()
    }

    /// Sets `profile.settings_template` in the fake HOME's user config.
    fn template(&self, path: &Path) {
        let dir = self.home.join(".amplihack");
        fs::create_dir_all(&dir).unwrap();
        chmod(&dir, 0o700);
        let file = dir.join("config.toml");
        fs::write(
            &file,
            format!("[profile]\nsettings_template = \"{}\"\n", path.display()),
        )
        .unwrap();
        chmod(&file, 0o600);
    }

    fn template_file(&self, file_name: &str, body: &str) -> PathBuf {
        let path = self.d.path().canonicalize().unwrap().join(file_name);
        fs::write(&path, body).unwrap();
        chmod(&path, 0o600);
        path
    }

    fn ensure(&self, name: &str) -> Outcome {
        ensure_for(&self.store, &n(name), self.home())
    }
}

fn skipped_reason(o: &Outcome) -> &str {
    match &o.result {
        Refresh::Skipped(reason) => reason,
        other => panic!("expected Skipped, got {other:?} ({:?})", o.warnings),
    }
}

fn assert_no_value_in(o: &Outcome) {
    for w in &o.warnings {
        assert!(!w.contains(SECRET), "warning shows an env value: {w}");
    }
    if let Refresh::Skipped(reason) = &o.result {
        assert!(!reason.contains(SECRET), "reason shows an env value");
    }
}

// ---- merge_env (pure) -----------------------------------------------------

#[test]
fn merge_keeps_profile_keys_and_adds_missing_ones() {
    let mut s = json!({"env": {"XPIA_ENABLED": "true", "FOO": "1"}, "model": "opus"});
    merge_env(&mut s, &obj(json!({"XPIA_ENABLED": "false", "BAR": "2"})));
    assert_eq!(
        s,
        json!({"env": {"XPIA_ENABLED": "true", "FOO": "1", "BAR": "2"}, "model": "opus"})
    );
}

#[test]
fn merge_creates_env_when_absent_and_leaves_other_keys() {
    let mut s = json!({"hooks": {"Stop": []}, "permissions": {"allow": ["x"]}});
    merge_env(&mut s, &obj(json!({"XPIA_ENABLED": "false"})));
    assert_eq!(
        s,
        json!({
            "hooks": {"Stop": []},
            "permissions": {"allow": ["x"]},
            "env": {"XPIA_ENABLED": "false"}
        })
    );
}

#[test]
fn merge_leaves_a_non_object_profile_env_alone() {
    let mut s = json!({"env": "oops"});
    merge_env(&mut s, &obj(json!({"XPIA_ENABLED": "false"})));
    assert_eq!(s, json!({"env": "oops"}));
}

#[test]
fn merge_with_an_empty_source_changes_nothing() {
    let mut s = json!({"language": "en"});
    merge_env(&mut s, &Map::new());
    assert_eq!(s, json!({"language": "en"}));
}

// ---- warning forms --------------------------------------------------------

#[test]
fn warning_has_the_fixed_form() {
    assert_eq!(
        warning(
            "work",
            Path::new("/h/.claude/settings.json"),
            "file is a symlink"
        ),
        "profile work: env not inherited from /h/.claude/settings.json: file is a symlink"
    );
}

#[test]
fn key_warning_has_the_fixed_form() {
    assert_eq!(
        key_warning(
            "work",
            "PORT",
            Path::new("/h/.claude/settings.json"),
            "value is not a string"
        ),
        "profile work: env key PORT not inherited from /h/.claude/settings.json: value is not a string"
    );
}

#[test]
fn key_warning_escapes_and_truncates_a_hostile_key() {
    let hostile = format!("BAD\n\x1b[31m{}", "K".repeat(200));
    let w = key_warning(
        "work",
        &hostile,
        Path::new("/s.json"),
        "invalid variable name",
    );
    assert!(!w.contains('\n') && !w.contains('\x1b'), "{w:?}");
    let key_part = w
        .strip_prefix("profile work: env key ")
        .and_then(|rest| rest.split(" not inherited from ").next())
        .unwrap();
    assert!(key_part.chars().count() <= 64, "{key_part:?}");
}

// ---- ensure_for: the XPIA gate --------------------------------------------

#[test]
fn backfill_adds_missing_keys_and_profile_keys_win() {
    let fx = Fx::new();
    fx.profile(
        "work",
        Some(r#"{"env":{"FOO":"1","BAR":"9"},"language":"en"}"#),
    );
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false","BAR":"2"},"model":"x"}"#);
    let o = fx.ensure("work");
    assert_eq!(o.result, Refresh::Updated);
    assert!(o.warnings.is_empty(), "{:?}", o.warnings);
    assert_eq!(
        fx.read("work"),
        json!({"env": {"FOO": "1", "BAR": "9", "XPIA_ENABLED": "false"}, "language": "en"})
    );
}

#[test]
fn backfill_keeps_hooks_permissions_and_does_not_add_api_key_helper() {
    let fx = Fx::new();
    let body = r#"{"hooks":{"Stop":[{"x":1}]},"permissions":{"allow":["Bash"]}}"#;
    fx.profile("work", Some(body));
    fx.source_settings(
        r#"{"env":{"XPIA_ENABLED":"false"},"apiKeyHelper":"/bin/k","hooks":{"Other":[]}}"#,
    );
    assert_eq!(fx.ensure("work").result, Refresh::Updated);
    let s = fx.read("work");
    assert_eq!(s["hooks"], json!({"Stop": [{"x": 1}]}));
    assert_eq!(s["permissions"], json!({"allow": ["Bash"]}));
    assert!(s.get("apiKeyHelper").is_none());
    assert_eq!(s["env"], json!({"XPIA_ENABLED": "false"}));
}

#[test]
fn profile_with_xpia_is_unchanged_byte_identical_and_source_not_read() {
    let fx = Fx::new();
    let body = "{ \"env\": {\"XPIA_ENABLED\": \"true\"},\n  \"language\":\"en\" }";
    fx.profile("work", Some(body));
    // A source that would be Skipped if it were opened proves it is not.
    fx.source_settings("{not json");
    chmod(&fx.source_file(), 0o666);
    let before = fx.raw("work");
    let o = fx.ensure("work");
    assert_eq!(o.result, Refresh::Unchanged);
    assert!(o.warnings.is_empty(), "{:?}", o.warnings);
    assert_eq!(fx.raw("work"), before);
}

#[test]
fn every_key_present_is_unchanged_and_not_rewritten() {
    let fx = Fx::new();
    // No XPIA in the profile, so the gate opens, but nothing is missing.
    fx.profile("work", Some(r#"{"env":{"FOO":"1"}}"#));
    fx.source_settings(r#"{"env":{"FOO":"2"}}"#);
    let before = fx.raw("work");
    let o = fx.ensure("work");
    assert_eq!(o.result, Refresh::Unchanged);
    assert_eq!(fx.raw("work"), before);
}

#[test]
fn source_without_env_is_unchanged() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(r#"{"language":"fr"}"#);
    let before = fx.raw("work");
    assert_eq!(fx.ensure("work").result, Refresh::Unchanged);
    assert_eq!(fx.raw("work"), before);
}

#[test]
fn missing_source_file_is_unchanged() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    let o = fx.ensure("work");
    assert_eq!(o.result, Refresh::Unchanged);
    assert!(o.warnings.is_empty());
}

// ---- missing profile settings.json ---------------------------------------

#[test]
fn missing_profile_settings_is_created_with_only_env() {
    let fx = Fx::new();
    let dir = fx.profile("work", None);
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"},"language":"en"}"#);
    assert_eq!(fx.ensure("work").result, Refresh::Updated);
    assert_eq!(fx.read("work"), json!({"env": {"XPIA_ENABLED": "false"}}));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.join("settings.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let _ = dir;
}

#[test]
fn missing_profile_settings_and_empty_source_env_creates_nothing() {
    let fx = Fx::new();
    let dir = fx.profile("work", None);
    fx.source_settings(r#"{"env":{}}"#);
    assert_eq!(fx.ensure("work").result, Refresh::Unchanged);
    assert!(!dir.join("settings.json").exists());
}

// ---- profile files the code does not understand --------------------------

#[test]
fn invalid_profile_json_is_skipped_and_never_overwritten() {
    let fx = Fx::new();
    fx.profile("work", Some("{broken"));
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    let o = fx.ensure("work");
    skipped_reason(&o);
    assert_eq!(o.warnings.len(), 1, "{:?}", o.warnings);
    assert_eq!(fx.raw("work"), b"{broken");
}

#[test]
fn non_object_profile_settings_is_skipped() {
    let fx = Fx::new();
    fx.profile("work", Some("[1,2]"));
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    let o = fx.ensure("work");
    assert_eq!(
        skipped_reason(&o),
        "profile settings.json is not a JSON object"
    );
    assert_eq!(fx.raw("work"), b"[1,2]");
}

#[test]
fn non_object_profile_env_is_skipped_and_left_alone() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"env":"oops"}"#));
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    let o = fx.ensure("work");
    assert_eq!(skipped_reason(&o), "profile env is not an object");
    assert_eq!(o.warnings.len(), 1);
    assert_eq!(fx.raw("work"), br#"{"env":"oops"}"#);
}

#[cfg(unix)]
#[test]
fn symlinked_profile_settings_is_skipped_and_target_untouched() {
    let fx = Fx::new();
    let dir = fx.profile("work", None);
    let target = fx.d.path().join("elsewhere.json");
    fs::write(&target, "{}").unwrap();
    std::os::unix::fs::symlink(&target, dir.join("settings.json")).unwrap();
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    skipped_reason(&fx.ensure("work"));
    assert_eq!(fs::read(&target).unwrap(), b"{}");
    assert!(
        fs::symlink_metadata(dir.join("settings.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

// ---- source checks --------------------------------------------------------

#[test]
fn non_object_source_env_is_skipped() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(r#"{"env":["XPIA_ENABLED"]}"#);
    let o = fx.ensure("work");
    assert_eq!(skipped_reason(&o), "source env is not an object");
    assert_eq!(o.warnings.len(), 1);
    assert_eq!(
        o.warnings[0],
        warning("work", &fx.source_file(), "source env is not an object")
    );
}

#[test]
fn invalid_source_json_is_skipped_with_a_location_not_the_text() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(&format!(r#"{{"env":{{"K":"{SECRET}"}} oops"#));
    let o = fx.ensure("work");
    skipped_reason(&o);
    assert_no_value_in(&o);
    assert!(fx.read("work").get("env").is_none());
}

#[cfg(unix)]
#[test]
fn group_writable_source_is_skipped() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    chmod(&fx.source_file(), 0o620);
    let o = fx.ensure("work");
    assert!(
        skipped_reason(&o)
            .starts_with("file is writable by group or others; fix it with `chmod 600 ")
    );
    assert!(fx.read("work").get("env").is_none());
}

#[cfg(unix)]
#[test]
fn other_writable_source_is_skipped() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    chmod(&fx.source_file(), 0o602);
    assert!(
        skipped_reason(&fx.ensure("work"))
            .starts_with("file is writable by group or others; fix it with `chmod 600 ")
    );
}

#[cfg(unix)]
#[test]
fn symlinked_source_is_skipped() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    let real = fx.d.path().join("dotfiles-settings.json");
    fs::write(&real, r#"{"env":{"XPIA_ENABLED":"false"}}"#).unwrap();
    chmod(&real, 0o600);
    std::os::unix::fs::symlink(&real, fx.source_file()).unwrap();
    let o = fx.ensure("work");
    assert_eq!(skipped_reason(&o), "file is a symlink");
    assert!(fx.read("work").get("env").is_none());
}

#[test]
fn oversized_source_is_skipped() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    let pad = "x".repeat(1024 * 1024 + 1);
    fx.source_settings(&format!(
        r#"{{"env":{{"XPIA_ENABLED":"false"}},"pad":"{pad}"}}"#
    ));
    assert_eq!(
        skipped_reason(&fx.ensure("work")),
        "file is larger than 1 MiB"
    );
}

#[test]
fn non_string_and_badly_named_entries_are_dropped_with_one_warning_each() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(&format!(
        r#"{{"env":{{"XPIA_ENABLED":"false","PORT":8080,"1BAD":"{SECRET}","OK_2":"{SECRET}"}}}}"#
    ));
    let o = fx.ensure("work");
    assert_eq!(o.result, Refresh::Updated);
    assert_eq!(
        fx.read("work")["env"],
        json!({"XPIA_ENABLED": "false", "OK_2": SECRET})
    );
    let src = fx.source_file();
    let mut warnings = o.warnings.clone();
    warnings.sort();
    let mut expected = vec![
        key_warning("work", "1BAD", &src, "invalid variable name"),
        key_warning("work", "PORT", &src, "value is not a string"),
    ];
    expected.sort();
    assert_eq!(warnings, expected);
    assert_no_value_in(&o);
}

#[test]
fn credential_variables_are_never_inherited() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(&format!(
        r#"{{"env":{{"XPIA_ENABLED":"false","ANTHROPIC_API_KEY":"{SECRET}","ANTHROPIC_AUTH_TOKEN":"{SECRET}","CLAUDE_CODE_OAUTH_TOKEN":"{SECRET}"}}}}"#
    ));
    let o = fx.ensure("work");
    assert_eq!(o.result, Refresh::Updated);
    assert_eq!(fx.read("work")["env"], json!({"XPIA_ENABLED": "false"}));
    assert!(!String::from_utf8(fx.raw("work")).unwrap().contains(SECRET));
    let src = fx.source_file();
    let reason = "credential variable; set it in the profile itself";
    let mut warnings = o.warnings.clone();
    warnings.sort();
    let mut expected = vec![
        key_warning("work", "ANTHROPIC_API_KEY", &src, reason),
        key_warning("work", "ANTHROPIC_AUTH_TOKEN", &src, reason),
        key_warning("work", "CLAUDE_CODE_OAUTH_TOKEN", &src, reason),
    ];
    expected.sort();
    assert_eq!(warnings, expected);
    assert_no_value_in(&o);
}

#[test]
fn overlong_key_is_dropped() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    let long = "K".repeat(257);
    fx.source_settings(&format!(
        r#"{{"env":{{"XPIA_ENABLED":"false","{long}":"v"}}}}"#
    ));
    let o = fx.ensure("work");
    assert_eq!(o.result, Refresh::Updated);
    assert_eq!(fx.read("work")["env"], json!({"XPIA_ENABLED": "false"}));
    assert_eq!(o.warnings.len(), 1, "{:?}", o.warnings);
    assert!(o.warnings[0].ends_with(": invalid variable name"));
}

#[test]
fn profile_entries_are_never_validated_or_dropped() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"env":{"1WEIRD":7}}"#));
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    assert_eq!(fx.ensure("work").result, Refresh::Updated);
    assert_eq!(
        fx.read("work")["env"],
        json!({"1WEIRD": 7, "XPIA_ENABLED": "false"})
    );
}

#[test]
fn no_home_is_unchanged_without_io() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    let before = fx.raw("work");
    let o = ensure_for(&fx.store, &n("work"), None);
    assert_eq!(o.result, Refresh::Unchanged);
    assert!(o.warnings.is_empty());
    assert_eq!(fx.raw("work"), before);
}

#[test]
fn no_shared_assets_and_no_template_is_unchanged() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    let bare = ProfileStore::new(fx.store.root().to_path_buf());
    let o = ensure_for(&bare, &n("work"), fx.home());
    assert_eq!(o.result, Refresh::Unchanged);
    assert!(fx.read("work").get("env").is_none());
}

// ---- template -------------------------------------------------------------

#[test]
fn template_is_the_source_instead_of_shared_settings() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(r#"{"env":{"FROM_HOME":"1"}}"#);
    let t = fx.template_file(
        "team.json",
        r#"{"env":{"XPIA_ENABLED":"false","TEAM":"x"}}"#,
    );
    fx.template(&t);
    assert_eq!(fx.ensure("work").result, Refresh::Updated);
    assert_eq!(
        fx.read("work")["env"],
        json!({"XPIA_ENABLED": "false", "TEAM": "x"})
    );
}

#[test]
fn missing_template_is_skipped_with_no_fallback() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    fx.template(&fx.d.path().canonicalize().unwrap().join("absent.json"));
    let o = fx.ensure("work");
    skipped_reason(&o);
    assert_eq!(o.warnings.len(), 1);
    assert!(
        fx.read("work").get("env").is_none(),
        "fell back to ~/.claude"
    );
}

#[test]
fn credential_named_template_is_refused_before_it_is_opened() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    for name in [".credentials.json", ".claude.json"] {
        let t = fx.template_file(name, r#"{"env":{"XPIA_ENABLED":"false"}}"#);
        fx.template(&t);
        let o = fx.ensure("work");
        assert_eq!(skipped_reason(&o), "refusing credential file", "{name}");
        assert!(fx.read("work").get("env").is_none());
    }
}

#[test]
fn invalid_template_config_value_is_skipped_with_no_fallback() {
    let fx = Fx::new();
    fx.profile("work", Some(r#"{"language":"en"}"#));
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    let dir = fx.home.join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("config.toml");
    fs::write(&file, "[profile]\nsettings_template = \"relative.json\"\n").unwrap();
    chmod(&file, 0o600);
    skipped_reason(&fx.ensure("work"));
    assert!(fx.read("work").get("env").is_none());
}

// ---- profile add: store.add then ensure_for --------------------------------

/// What `profile add` does: create the profile, then run the backfill.
fn add(fx: &Fx, name: &str) -> Outcome {
    fx.store.add(&n(name)).unwrap();
    fx.ensure(name)
}

#[test]
fn add_does_not_seed_env_through_the_allowlist() {
    let fx = Fx::new();
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"},"language":"en"}"#);
    fx.store.add(&n("work")).unwrap();
    let s = fx.read("work");
    assert!(s.get("env").is_none(), "store.add copied env: {s}");
    assert_eq!(s["language"], json!("en"));
}

#[test]
fn add_without_template_backfills_from_source() {
    let fx = Fx::new();
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"},"language":"en"}"#);
    let o = add(&fx, "work");
    assert_eq!(o.result, Refresh::Updated);
    assert!(o.warnings.is_empty());
    let s = fx.read("work");
    assert_eq!(s["env"], json!({"XPIA_ENABLED": "false"}));
    assert_eq!(s["language"], json!("en"));
}

#[cfg(unix)]
#[test]
fn add_refuses_group_writable_source() {
    let fx = Fx::new();
    fx.source_settings(&format!(
        r#"{{"env":{{"XPIA_ENABLED":"false","KEY":"{SECRET}"}},"language":"en"}}"#
    ));
    chmod(&fx.source_file(), 0o664);
    let o = add(&fx, "work");
    assert!(
        skipped_reason(&o)
            .starts_with("file is writable by group or others; fix it with `chmod 600 ")
    );
    assert_eq!(o.warnings.len(), 1);
    assert_no_value_in(&o);
    assert!(fx.read("work").get("env").is_none());
}

#[cfg(unix)]
#[test]
fn add_drops_invalid_keys_and_non_string_values() {
    let fx = Fx::new();
    fx.source_settings(&format!(
        r#"{{"env":{{"XPIA_ENABLED":"false","BAD-KEY":"{SECRET}","NUM":1}}}}"#
    ));
    let o = add(&fx, "work");
    assert_eq!(o.result, Refresh::Updated);
    assert_eq!(o.warnings.len(), 2, "{:?}", o.warnings);
    assert_no_value_in(&o);
    assert_eq!(fx.read("work")["env"], json!({"XPIA_ENABLED": "false"}));
}

#[cfg(unix)]
#[test]
fn add_with_template_uses_only_the_template() {
    let fx = Fx::new();
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"true","HOME_ONLY":"1"},"language":"en"}"#);
    let t = fx.template_file(
        "team.json",
        r#"{"env":{"XPIA_ENABLED":"false","TEAM":"x"}}"#,
    );
    fx.template(&t);
    let o = add(&fx, "work");
    assert_eq!(o.result, Refresh::Updated);
    let s = fx.read("work");
    assert_eq!(s["env"], json!({"XPIA_ENABLED": "false", "TEAM": "x"}));
    assert_eq!(s["language"], json!("en"), "other seeded keys kept");
}

#[cfg(unix)]
#[test]
fn add_with_broken_template_leaves_no_env() {
    let fx = Fx::new();
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"true"},"language":"en"}"#);
    let t = fx.template_file("team.json", "{broken");
    fx.template(&t);
    let o = add(&fx, "work");
    skipped_reason(&o);
    assert_eq!(o.warnings.len(), 1);
    let s = fx.read("work");
    assert!(
        s.get("env").is_none(),
        "env inherited despite template: {s}"
    );
    assert_eq!(s["language"], json!("en"));
}

// ---- ensure_all -----------------------------------------------------------

#[test]
fn ensure_all_visits_every_profile() {
    let fx = Fx::new();
    fx.profile("a", Some(r#"{"language":"en"}"#));
    fx.profile("b", Some(r#"{"env":{"XPIA_ENABLED":"true"}}"#));
    fx.profile("c", None);
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    let mut results: Vec<(String, Refresh)> = ensure_all(&fx.store, fx.home())
        .unwrap()
        .into_iter()
        .map(|(name, o)| (name.as_str().to_string(), o.result))
        .collect();
    results.sort_by(|x, y| x.0.cmp(&y.0));
    assert_eq!(
        results,
        vec![
            ("a".to_string(), Refresh::Updated),
            ("b".to_string(), Refresh::Unchanged),
            ("c".to_string(), Refresh::Updated),
        ]
    );
    assert_eq!(fx.read("b")["env"], json!({"XPIA_ENABLED": "true"}));
    assert_eq!(fx.read("c"), json!({"env": {"XPIA_ENABLED": "false"}}));
}

#[test]
fn ensure_all_reports_an_unreadable_root() {
    let fx = Fx::new();
    // A regular file where the root should be: read_dir fails.
    let not_a_dir = fx.d.path().join("root-file");
    fs::write(&not_a_dir, "").unwrap();
    let store = ProfileStore::new(not_a_dir).with_shared_assets(fx.source.clone());
    let o = ensure_all(&store, fx.home()).expect_err("root is not a directory");
    skipped_reason(&o);
    assert_eq!(o.warnings.len(), 1);
}

#[test]
fn credential_files_in_the_source_are_never_copied_by_the_backfill() {
    let fx = Fx::new();
    let dir = fx.profile("work", Some(r#"{"language":"en"}"#));
    fs::write(fx.source.join(".credentials.json"), SECRET).unwrap();
    fs::write(fx.source.join(".claude.json"), SECRET).unwrap();
    fx.source_settings(r#"{"env":{"XPIA_ENABLED":"false"}}"#);
    let o = fx.ensure("work");
    assert_eq!(o.result, Refresh::Updated);
    assert!(!dir.join(".credentials.json").exists());
    assert!(!dir.join(".claude.json").exists());
    assert!(!String::from_utf8(fx.raw("work")).unwrap().contains(SECRET));
}
