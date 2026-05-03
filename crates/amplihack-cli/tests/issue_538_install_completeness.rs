//! TDD tests for issue #538: `amplihack install` must not report success when
//! the staged framework is incomplete.
//!
//! These tests intentionally define the install-completeness contract before the
//! production verifier is hardened:
//! - npm packages must include `amplifier-bundle/`.
//! - install must fail loudly when a required bundle category is missing.
//! - a successful install must stage every source skill/agent/command child dir.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use assert_cmd::Command;

fn install_command_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut cur: &Path = &crate_dir;
    loop {
        if cur.join("amplifier-bundle").is_dir() && cur.join("Cargo.toml").is_file() {
            return cur.to_path_buf();
        }
        cur = cur
            .parent()
            .expect("walked above filesystem root looking for workspace root");
    }
}

fn write_stub_binary(dir: &Path, name: &str) -> PathBuf {
    fs::create_dir_all(dir).expect("create stub binary dir");
    let path = dir.join(name);
    let content = format!("#!/bin/sh\nexit 0\n{}\n", "x".repeat(1100));
    fs::write(&path, content).expect("write stub binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("chmod stub hooks binary");
    }
    path
}

fn cargo_amplihack_install(home: &Path, hooks_bin: &Path, source: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO"));
    let recipe_runner = hooks_bin
        .parent()
        .expect("stub binary has parent")
        .join("recipe-runner-rs");
    cmd.current_dir(workspace_root())
        .arg("run")
        .arg("--quiet")
        .arg("--package")
        .arg("amplihack")
        .arg("--")
        .arg("install")
        .arg("--local")
        .arg(source)
        .env("HOME", home)
        .env("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", hooks_bin)
        .env("RECIPE_RUNNER_RS_PATH", recipe_runner)
        .env_remove("AMPLIHACK_HOME")
        .env_remove("CLAUDECODE");
    cmd
}

fn immediate_child_dirs(path: &Path) -> Vec<String> {
    if !path.is_dir() {
        return Vec::new();
    }

    let mut dirs = fs::read_dir(path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
        .map(|entry| entry.expect("read directory entry"))
        .filter(|entry| entry.file_type().expect("read entry file type").is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    dirs.sort();
    dirs
}

fn create_bundle_missing_skills(root: &Path) {
    let bundle = root.join("amplifier-bundle");
    for dir in [
        "agents/core",
        "context",
        "tools/amplihack",
        "tools/xpia",
        "recipes",
        "behaviors",
        "modules",
    ] {
        fs::create_dir_all(bundle.join(dir)).expect("create required bundle dir");
        fs::write(bundle.join(dir).join("marker.txt"), "x\n").expect("write marker");
    }
    fs::write(
        bundle.join("tools/statusline.sh"),
        "#!/bin/sh\necho status\n",
    )
    .expect("write statusline");
    for recipe in [
        "smart-orchestrator.yaml",
        "default-workflow.yaml",
        "investigation-workflow.yaml",
    ] {
        fs::write(
            bundle.join("recipes").join(recipe),
            "name: test\nsteps: []\n",
        )
        .expect("write recipe");
    }
    fs::write(bundle.join("CLAUDE.md"), "# test bundle\n").expect("write CLAUDE.md");
}

#[test]
fn npm_package_manifest_includes_amplifier_bundle_assets() {
    let package_json =
        fs::read_to_string(workspace_root().join("package.json")).expect("read package.json");

    assert!(
        package_json.contains("\"amplifier-bundle/\"")
            || package_json.contains("\"amplifier-bundle\""),
        "package.json files list must include amplifier-bundle/ so npm-installed \
         amplihack binaries can stage the full framework assets"
    );
}

#[test]
fn install_fails_loudly_when_required_source_skills_are_missing() {
    let _guard = install_command_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("create temp home");
    let source = tempfile::tempdir().expect("create incomplete source");
    let stub_dir = home.path().join("stub-bin");
    let hooks_bin = write_stub_binary(&stub_dir, "amplihack-hooks");
    write_stub_binary(&stub_dir, "recipe-runner-rs");
    create_bundle_missing_skills(source.path());

    let output = cargo_amplihack_install(home.path(), &hooks_bin, source.path())
        .output()
        .expect("run amplihack install");
    let combined_output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !output.status.success(),
        "install must fail loudly when source amplifier-bundle/skills is missing; output:\n\
         {combined_output}"
    );
    assert!(
        combined_output.contains("skills") || combined_output.contains("Required framework assets"),
        "failure output must name the missing skills category or required framework assets; output:\n\
         {combined_output}"
    );
}

#[test]
fn install_stages_every_source_skill_agent_and_command_directory() {
    let _guard = install_command_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("create temp home");
    let stub_dir = home.path().join("stub-bin");
    let hooks_bin = write_stub_binary(&stub_dir, "amplihack-hooks");
    write_stub_binary(&stub_dir, "recipe-runner-rs");
    let workspace = workspace_root();

    cargo_amplihack_install(home.path(), &hooks_bin, &workspace)
        .assert()
        .success();

    let source_bundle = workspace.join("amplifier-bundle");
    let staged_claude = home.path().join(".amplihack/.claude");
    let staged_bundle = home.path().join(".amplihack/amplifier-bundle");

    for (source_category, staged_category) in [
        ("skills", "skills"),
        ("agents", "agents"),
        ("commands", "commands"),
    ] {
        for child in immediate_child_dirs(&source_bundle.join(source_category)) {
            let staged = staged_claude.join(staged_category).join(&child);
            assert!(
                staged.is_dir(),
                "source amplifier-bundle/{source_category}/{child} must be staged at {}",
                staged.display()
            );
        }
    }

    for child in immediate_child_dirs(&source_bundle.join("skills")) {
        let staged = staged_bundle.join("skills").join(&child);
        assert!(
            staged.is_dir(),
            "source skill {child} must also be present in the staged full amplifier-bundle at {}",
            staged.display()
        );
    }

    let source_skill_count = immediate_child_dirs(&source_bundle.join("skills")).len();
    let staged_skill_count = immediate_child_dirs(&staged_claude.join("skills")).len();
    assert!(
        staged_skill_count >= source_skill_count,
        "staged skill count must be at least source skill count; source={source_skill_count}, \
         staged={staged_skill_count}"
    );
}
