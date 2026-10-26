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
