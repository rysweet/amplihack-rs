//! Issue #1606: layering, lower-only and trust rules of `user_config::load`,
//! through the `LoadCtx` seam (no process environment, a temp HOME).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::files::user_file;
use super::{Effective, LoadCtx, Origin, Value, load};

struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let project = root.join("project");
        fs::create_dir_all(home.join(".amplihack")).unwrap();
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::write(project.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        Self {
            _tmp: tmp,
            home,
            project,
        }
    }

    fn user(&self, body: &str) {
        let path = user_file(&self.home);
        fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn project_file(&self) -> PathBuf {
        self.project.join(".amplihack").join("config.toml")
    }

    fn project(&self, body: &str) {
        fs::create_dir_all(self.project.join(".amplihack")).unwrap();
        fs::write(self.project_file(), body).unwrap();
    }

    fn load(&self, env: &[(&str, &str)]) -> Effective {
        self.load_in(&self.project, env)
    }

    fn load_in(&self, cwd: &Path, env: &[(&str, &str)]) -> Effective {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        load(&LoadCtx {
            home: Some(self.home.clone()),
            cwd: cwd.to_path_buf(),
            read_project: true,
            env: &move |name| env.get(name).cloned(),
            migrate: true,
        })
    }
}

#[test]
fn defaults_apply_with_no_files() {
    let fx = Fx::new();
    let e = fx.load(&[]);
    let depth = e.entry("orchestrator.max_depth");
    assert_eq!(depth.value, Some(Value::Int(3)));
    assert_eq!(depth.origin, Origin::Default);
    assert_eq!(e.entry("agent.binary").value, None);
    assert!(e.diagnostics.is_empty());
}

#[test]
fn each_layer_outranks_the_one_below() {
    let fx = Fx::new();
    fx.user("[agent]\nbinary = \"claude\"\n");
    assert_eq!(
        fx.load(&[]).entry("agent.binary").origin,
        Origin::User(user_file(&fx.home))
    );
    // agent.binary is user-only: a project file's value is reported and
    // ignored.
    fx.project("[agent]\nbinary = \"codex\"\n[recipe]\nverbose = true\n");
    let e = fx.load(&[]);
    assert_eq!(
        e.entry("agent.binary").value,
        Some(Value::Text("claude".into()))
    );
    assert!(
        e.diagnostics
            .iter()
            .any(|d| d.key.as_deref() == Some("agent.binary")),
        "{:?}",
        e.diagnostics
    );
    assert_eq!(
        e.entry("recipe.verbose").origin,
        Origin::Project(fx.project_file())
    );
    let e = fx.load(&[("AMPLIHACK_AGENT_BINARY", "amplifier")]);
    assert_eq!(
        e.entry("agent.binary").origin,
        Origin::Env("AMPLIHACK_AGENT_BINARY")
    );
    // The getter ignores env: it is the files' answer.
    assert_eq!(
        e.file_value("agent.binary"),
        Some(&Value::Text("claude".into()))
    );
}

#[test]
fn a_tagged_agent_binary_env_value_is_not_the_env_layer() {
    let fx = Fx::new();
    fx.user("[agent]\nbinary = \"claude\"\n");
    let e = fx.load(&[
        ("AMPLIHACK_AGENT_BINARY", "copilot"),
        ("AMPLIHACK_AGENT_BINARY_SOURCE", "tmux_server:COPILOT_CLI"),
    ]);
    assert_eq!(
        e.entry("agent.binary").value,
        Some(Value::Text("claude".into()))
    );
}

#[test]
fn project_can_only_lower_lower_only_keys() {
    let fx = Fx::new();
    fx.user("[orchestrator]\nmax_depth = 4\n[recipe]\nstep_timeout = 600\n");
    fx.project("[orchestrator]\nmax_depth = 9\n[recipe]\nstep_timeout = 60\n");
    let e = fx.load(&[]);
    assert_eq!(e.entry("orchestrator.max_depth").value, Some(Value::Int(4)));
    assert_eq!(e.entry("recipe.step_timeout").value, Some(Value::Int(60)));
    assert!(
        e.diagnostics
            .iter()
            .any(|d| d.reason.contains("is above the user value 4")),
        "{:?}",
        e.diagnostics
    );
}

#[test]
fn project_cannot_raise_above_the_default_either() {
    let fx = Fx::new();
    fx.project("[orchestrator]\nmax_depth = 10\n");
    let e = fx.load(&[]);
    assert_eq!(e.entry("orchestrator.max_depth").value, Some(Value::Int(3)));
    assert_eq!(e.entry("orchestrator.max_depth").origin, Origin::Default);
}

/// Issue #1801: 21600 is a default a project cannot raise, not a ceiling.
#[test]
fn a_project_cannot_raise_the_usage_limit_wait_above_its_default() {
    let fx = Fx::new();
    fx.project("[recipe]\nusage_limit_max_wait_secs = 86400\n");
    let e = fx.load(&[]);
    assert_eq!(
        e.entry("recipe.usage_limit_max_wait_secs").value,
        Some(Value::Int(21_600))
    );
}

#[test]
fn a_project_can_lower_the_usage_limit_wait() {
    let fx = Fx::new();
    fx.project("[recipe]\nusage_limit_max_wait_secs = 600\n");
    let e = fx.load(&[]);
    assert_eq!(
        e.entry("recipe.usage_limit_max_wait_secs").value,
        Some(Value::Int(600))
    );
}

#[test]
fn user_only_keys_in_a_project_file_are_ignored() {
    let fx = Fx::new();
    fx.project("[paths]\nhome = \"/opt/x\"\n[node]\nheap_consent = true\n");
    let e = fx.load(&[]);
    assert_eq!(e.entry("paths.home").value, None);
    assert_eq!(e.entry("node.heap_consent").value, None);
    assert_eq!(e.diagnostics.len(), 2, "{:?}", e.diagnostics);
}

#[test]
fn invalid_env_values_are_reported_in_the_origin() {
    let fx = Fx::new();
    let e = fx.load(&[("AMPLIHACK_MAX_DEPTH", "lots")]);
    let depth = e.entry("orchestrator.max_depth");
    assert_eq!(depth.value, Some(Value::Int(3)));
    assert_eq!(
        depth.origin.to_string(),
        "env (invalid: AMPLIHACK_MAX_DEPTH is not an integer)"
    );
}

#[test]
fn a_syntax_error_drops_only_that_file() {
    let fx = Fx::new();
    fx.user("[agent\nbinary = \"claude\"\n");
    fx.project("[recipe]\nverbose = true\n");
    let e = fx.load(&[]);
    assert_eq!(e.entry("recipe.verbose").value, Some(Value::Bool(true)));
    assert_eq!(e.entry("agent.binary").value, None);
    assert!(
        e.diagnostics[0]
            .reason
            .starts_with("TOML syntax error at line 1")
    );
}

#[test]
fn the_search_does_not_leave_the_git_root() {
    let fx = Fx::new();
    fx.project("[recipe]\nverbose = true\n");
    let inner = fx.project.join("sub");
    fs::create_dir_all(&inner).unwrap();
    fs::write(inner.join(".git"), "gitdir: x\n").unwrap();
    assert_eq!(
        fx.load_in(&inner, &[]).entry("recipe.verbose").origin,
        Origin::Default
    );
}

#[cfg(unix)]
#[test]
fn an_insecure_user_file_loses_user_only_keys() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    fx.user("[recipe]\nverbose = true\n[node]\nheap_consent = true\n");
    fs::set_permissions(user_file(&fx.home), fs::Permissions::from_mode(0o620)).unwrap();
    let e = fx.load(&[]);
    assert_eq!(e.entry("recipe.verbose").value, Some(Value::Bool(true)));
    assert_eq!(e.entry("node.heap_consent").value, None);
}

#[test]
fn diagnostics_escape_hostile_keys() {
    let fx = Fx::new();
    fx.user("[agent]\n\"x\\u001b\\u202e\" = 1\n");
    let e = fx.load(&[]);
    let line = e.diagnostics[0].to_string();
    assert!(
        !line.contains('\u{1b}') && !line.contains('\u{202e}'),
        "{line}"
    );
}

#[test]
fn the_legacy_consent_is_a_layer_until_migrated() {
    let fx = Fx::new();
    fs::write(
        fx.home.join(".amplihack").join("config"),
        r#"{"node_options_consent": true}"#,
    )
    .unwrap();
    let e = load(&LoadCtx {
        home: Some(fx.home.clone()),
        cwd: fx.project.clone(),
        read_project: true,
        env: &|_| None,
        migrate: false,
    });
    assert_eq!(e.entry("node.heap_consent").value, Some(Value::Bool(true)));
    assert!(matches!(
        e.entry("node.heap_consent").origin,
        Origin::Legacy(_)
    ));
    let e = fx.load(&[]);
    assert_eq!(
        e.entry("node.heap_consent").origin,
        Origin::User(user_file(&fx.home))
    );
    assert!(e.legacy.is_none());
}

// --- Issue #2007: rotator keys and `user_keys` ---------------------------------

#[test]
fn rotator_keys_in_a_project_file_are_ignored_with_a_diagnostic() {
    let fx = Fx::new();
    fx.project("[rotator]\nsession_at = 10\ncodex_credit_floor = 0\n");
    let e = fx.load(&[]);
    assert_eq!(e.entry("rotator.session_at").value, Some(Value::Int(85)));
    assert_eq!(e.entry("rotator.session_at").origin, Origin::Default);
    assert_eq!(
        e.entry("rotator.codex_credit_floor").value,
        Some(Value::Int(3000))
    );
    assert_eq!(
        e.entry("rotator.codex_credit_floor").origin,
        Origin::Default
    );
    for key in ["rotator.session_at", "rotator.codex_credit_floor"] {
        let d = e
            .diagnostics
            .iter()
            .find(|d| d.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("no diagnostic for {key}: {:?}", e.diagnostics));
        assert!(
            d.to_string()
                .ends_with(&format!("{key}: can only be set in the user file; ignored")),
            "{d}"
        );
    }
}

#[test]
fn the_rotator_variable_outranks_the_user_file() {
    let fx = Fx::new();
    fx.user("[rotator]\nsession_at = 70\n");
    let e = fx.load(&[]);
    assert_eq!(e.entry("rotator.session_at").value, Some(Value::Int(70)));
    let e = fx.load(&[("AMPLIHACK_ROTATE_SESSION_AT", "60")]);
    assert_eq!(e.entry("rotator.session_at").value, Some(Value::Int(60)));
}

#[test]
fn user_keys_returns_one_entry_per_name_in_order() {
    let fx = Fx::new();
    fx.user("[rotator]\ninterval = 900\ncodex_credit_floor = 0\n");
    let names = [
        "rotator.session_at",
        "rotator.interval",
        "rotator.codex_credit_floor",
        "rotator.week_at",
    ];
    let got = super::user_keys(&fx.home, &names);
    assert_eq!(got.len(), names.len());
    assert_eq!(
        got,
        vec![
            Ok(None),
            Ok(Some(Value::Int(900))),
            Ok(Some(Value::Int(0))),
            Ok(None),
        ]
    );
    assert!(super::user_keys(&fx.home, &[]).is_empty());
}

#[test]
fn user_keys_ignores_the_process_environment() {
    // The rotator parses its own variables; `user_keys` is the file only.
    let fx = Fx::new();
    let got = super::user_keys(&fx.home, &["rotator.session_at"]);
    assert_eq!(got, vec![Ok(None)]);
}

#[test]
fn user_keys_reports_a_bad_value_for_that_key_only() {
    let fx = Fx::new();
    fx.user("[rotator]\nsession_at = 101\ninterval = 900\n");
    let got = super::user_keys(&fx.home, &["rotator.session_at", "rotator.interval"]);
    let e = got[0].as_ref().expect_err("101 must fail closed");
    assert!(e.contains("rotator.session_at"), "{e}");
    assert!(e.contains("from 1 to 100"), "{e}");
    assert_eq!(got[1], Ok(Some(Value::Int(900))));
}

#[test]
fn user_keys_fails_every_name_when_the_file_cannot_be_parsed() {
    let fx = Fx::new();
    fx.user("[rotator\nsession_at = 70\n");
    let got = super::user_keys(
        &fx.home,
        &["rotator.session_at", "rotator.codex_credit_floor"],
    );
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(Result::is_err), "{got:?}");
}

#[cfg(unix)]
#[test]
fn user_keys_fails_closed_on_a_file_others_can_write() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    fx.user("[rotator]\nsession_at = 70\n");
    fs::set_permissions(user_file(&fx.home), fs::Permissions::from_mode(0o664)).unwrap();
    let got = super::user_keys(&fx.home, &["rotator.session_at"]);
    assert!(
        got[0].is_err(),
        "an insecure file must not read as unset: {got:?}"
    );
}

#[test]
fn user_keys_refuses_a_home_outside_the_temp_dir_under_the_guard() {
    // Unit tests always run under the guard; `/` is never inside temp.
    let got = super::user_keys(Path::new("/"), &["rotator.session_at", "rotator.week_at"]);
    assert_eq!(got.len(), 2);
    for r in &got {
        let e = r.as_ref().expect_err("guarded HOME must fail");
        assert!(e.contains(super::TEST_HOME_GUARD_ENV), "{e}");
    }
}

#[test]
fn sessions_key_still_reads_through_user_keys() {
    let fx = Fx::new();
    fx.user("[sessions]\narchive_days = 7\n");
    assert_eq!(
        super::sessions_key(&fx.home, "sessions.archive_days"),
        Ok(Some(Value::Int(7)))
    );
    assert_eq!(
        super::sessions_key(&fx.home, "sessions.archive_dest"),
        Ok(None)
    );
}

// --- Issue #2007 D2: `KeyProblem.reason` never contains the path -------------

#[test]
fn a_whole_file_problem_gives_a_path_free_reason_and_keeps_the_path_in_full() {
    let fx = Fx::new();
    fx.user("[rotator\nweek_at = = 80\n");
    let path = super::display_path(&user_file(&fx.home));
    let read = super::user_keys_detail(&fx.home, &["rotator.week_at", "rotator.interval"]);
    assert_eq!(read.len(), 2);
    for r in read {
        let p = r.expect_err("an unparseable file is an error for every key");
        assert!(!p.reason.is_empty());
        assert!(!p.reason.contains(&path), "reason: {}", p.reason);
        assert!(
            !p.reason.contains(&fx.home.display().to_string()),
            "reason: {}",
            p.reason
        );
        assert!(p.full.contains(&path), "full keeps the path: {}", p.full);
        assert!(
            p.full.contains(&p.reason),
            "full: {} / {}",
            p.full,
            p.reason
        );
    }
}

#[cfg(unix)]
#[test]
fn a_group_writable_user_file_gives_the_safe_rs_reason_for_each_key() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    fx.user("[rotator]\nweek_at = 80\n");
    fs::set_permissions(user_file(&fx.home), fs::Permissions::from_mode(0o664)).unwrap();
    let read =
        super::user_keys_detail(&fx.home, &["rotator.week_at", "rotator.codex_credit_floor"]);
    let p = read[0]
        .clone()
        .expect_err("a key the file sets fails closed");
    assert_eq!(
        p.reason,
        "ignored because the file is writable by group or others"
    );
    assert!(p.whole_file, "the fault is the file's, not the key's");
    // The loader records the fault only against keys the file sets; an
    // unset key is `Ok(None)` here, and `UserConfig::at` fails it closed
    // through `whole_file` (tests_settings::config_2007).
    assert_eq!(read[1], Ok(None));
}

#[test]
fn a_bad_key_in_a_sound_file_fails_only_that_key() {
    let fx = Fx::new();
    fx.user("[rotator]\nweek_at = 0\ninterval = 900\n");
    let read = super::user_keys_detail(&fx.home, &["rotator.week_at", "rotator.interval"]);
    let p = read[0].clone().expect_err("0 is below the range");
    assert!(p.reason.contains("from 1 to 100"), "{}", p.reason);
    assert!(!p.reason.contains("config.toml"), "{}", p.reason);
    assert_eq!(read[1], Ok(Some(Value::Int(900))));
}

#[test]
fn a_guarded_home_keeps_the_path_in_full_and_out_of_reason() {
    // Issue #2007 S1: under the test guard the reason reaches RotateConfig
    // errors and `thresholds.*.error` in JSON, so it must not hold HOME.
    let home = Path::new("/");
    let read = super::user_keys_detail(home, &["rotator.session_at", "rotator.codex_credit_floor"]);
    assert_eq!(read.len(), 2);
    let shown = super::display_path(home);
    for r in read {
        let p = r.expect_err("a guarded HOME fails closed");
        assert!(p.whole_file, "the guard is a fault with the whole file");
        assert_eq!(
            p.reason,
            format!(
                "{} is set and HOME is outside the temp dir",
                super::TEST_HOME_GUARD_ENV
            ),
            "the reason is path-free"
        );
        assert!(p.full.contains(&shown), "full keeps the path: {}", p.full);
        assert!(p.full.contains(super::TEST_HOME_GUARD_ENV), "{}", p.full);
    }
}
