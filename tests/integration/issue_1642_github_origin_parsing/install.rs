//! Group (f): `amplihack install --local` ships the shared helper next to
//! `workflow_gh_retry.sh`, and an installed caller finds it through its own
//! SCRIPT_DIR and still rejects a hostile origin without calling gh.

use super::fixture::{Fixture, workspace_root};
use std::path::Path;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_amplihack")
}

fn path_with_bin() -> String {
    let dir = Path::new(bin())
        .parent()
        .expect("binary dir")
        .display()
        .to_string();
    match std::env::var("PATH") {
        Ok(path) => format!("{dir}:{path}"),
        Err(_) => dir,
    }
}

fn install_into(home: &Path) {
    let mut install = crate::isolated_amplihack::isolated(bin());
    install
        .env("HOME", home)
        .env("PATH", path_with_bin())
        .env("TMPDIR", home)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("CARGO_HOME", home.join(".cargo"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_RUNTIME_DIR", home.join(".runtime"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("CLAUDE_PLUGIN_DATA", home.join("claude-plugin-data"))
        .env("COPILOT_HOME", home.join(".copilot"))
        .env("COPILOT_CONFIG_DIR", home.join(".copilot"))
        .env("RUSTC_WRAPPER", "")
        .args(["install", "--local"])
        .arg(workspace_root());
    let output = install.output().expect("run amplihack install --local");
    assert!(
        output.status.success(),
        "private install failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn install_ships_the_helper_and_installed_callers_use_it() {
    let home = tempfile::tempdir().expect("private install home");
    install_into(home.path());

    let tools = home.path().join(".amplihack/amplifier-bundle/tools");
    assert!(
        tools.join("workflow_gh_retry.sh").is_file(),
        "install layout changed: no workflow_gh_retry.sh under {}",
        tools.display()
    );
    let helper = tools.join("workflow_github_repo.sh");
    assert!(helper.is_file(), "install must ship {}", helper.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&helper)
            .expect("stat")
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "installed helper must stay executable");
    }

    let fx = Fixture::with_origin("https://evil.example#@github.com/o/r");
    let mut cmd = fx.command("bash");
    cmd.arg(tools.join("workflow_pr_ready.sh"));
    let outcome = fx.run(cmd);
    assert!(
        !outcome.success() && fx.repo_gh_calls().is_empty(),
        "installed workflow_pr_ready.sh must reject a hostile origin without gh; gh calls {:?}\n{}",
        fx.repo_gh_calls(),
        outcome.describe()
    );
    assert!(
        outcome
            .stderr
            .contains("unable to determine current GitHub repo identity"),
        "the installed caller must reach the identity check (helper found via SCRIPT_DIR)\n{}",
        outcome.describe()
    );
}
