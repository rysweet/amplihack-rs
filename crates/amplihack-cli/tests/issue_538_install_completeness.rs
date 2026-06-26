//! TDD tests for issue #538: `amplihack install` must not report success when
//! the staged framework is incomplete.
//!
//! These tests intentionally define the install-completeness contract before the
//! production verifier is hardened:
//! - npm packages must include `amplifier-bundle/`.
//! - install must fail loudly when a required bundle category is missing.
//! - a successful install must stage every source skill/agent/command child dir.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use assert_cmd::Command;

fn install_command_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn workspace_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();

    ROOT.get_or_init(|| {
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
    })
    .as_path()
}

fn valid_existing_binary(path: &Path) -> bool {
    path.is_file() && is_executable(path)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn workspace_debug_amplihack() -> PathBuf {
    workspace_root()
        .join("target")
        .join("debug")
        .join(format!("amplihack{}", std::env::consts::EXE_SUFFIX))
}

fn resolve_amplihack_binary() -> PathBuf {
    resolve_amplihack_binary_from(
        std::env::var_os("AMPLIHACK_PROBE_BIN"),
        std::env::var_os("CARGO_BIN_EXE_amplihack"),
        &workspace_debug_amplihack(),
    )
    .unwrap_or_else(|message| panic!("{message}"))
}

fn resolve_amplihack_binary_from(
    probe_bin: Option<OsString>,
    cargo_bin: Option<OsString>,
    fallback: &Path,
) -> Result<PathBuf, String> {
    if let Some(path) = probe_bin {
        let path = PathBuf::from(path);
        if valid_existing_binary(&path) {
            return Ok(path);
        }
        return Err(format!(
            "AMPLIHACK_PROBE_BIN is set to {}, but that path is not an existing executable file",
            path.display()
        ));
    }

    if let Some(path) = cargo_bin {
        let path = PathBuf::from(path);
        if valid_existing_binary(&path) {
            return Ok(path);
        }
        return Err(format!(
            "CARGO_BIN_EXE_amplihack is set to {}, but that path is not an existing executable file",
            path.display()
        ));
    }

    if valid_existing_binary(&fallback) {
        return Ok(fallback.to_path_buf());
    }

    Err(format!(
        "no prebuilt amplihack binary found; set AMPLIHACK_PROBE_BIN or build the workspace binary before running this test (checked {})",
        fallback.display()
    ))
}

fn write_success_executable(dir: &Path, name: &str) -> PathBuf {
    fs::create_dir_all(dir).expect("create test executable dir");
    let path = dir.join(name);
    let content = format!("#!/bin/sh\nexit 0\n{}\n", "x".repeat(1100));
    fs::write(&path, content).expect("write test executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("chmod test executable");
    }
    path
}

fn write_install_tooling(home: &Path) -> PathBuf {
    let bin_dir = home.join("test-bin");
    let hooks_bin = write_success_executable(&bin_dir, "amplihack-hooks");
    write_success_executable(&bin_dir, "recipe-runner-rs");
    hooks_bin
}

fn amplihack_install_command(home: &Path, hooks_bin: &Path, source: &Path) -> Command {
    amplihack_install_command_with_binary(resolve_amplihack_binary(), home, hooks_bin, source)
}

fn amplihack_install_command_with_binary(
    binary: impl AsRef<Path>,
    home: &Path,
    hooks_bin: &Path,
    source: &Path,
) -> Command {
    let mut cmd = Command::new(binary.as_ref());
    let recipe_runner = hooks_bin
        .parent()
        .expect("hooks binary has parent")
        .join("recipe-runner-rs");
    cmd.current_dir(workspace_root())
        .arg("install")
        .arg("--local")
        .arg(source)
        .env("HOME", home)
        .env("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", hooks_bin)
        .env("RECIPE_RUNNER_RS_PATH", recipe_runner)
        .env("AMPLIHACK_SKIP_MMDC", "1")
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
    write_compatible_recipe_bundle(&bundle.join("recipes"));
    fs::write(bundle.join("CLAUDE.md"), "# test bundle\n").expect("write CLAUDE.md");
}

fn write_compatible_recipe_bundle(recipes: &Path) {
    fs::write(
        recipes.join("smart-orchestrator.yaml"),
        r#"name: "smart-orchestrator"
steps:
  - id: "smart-classify-route"
    type: "recipe"
    recipe: "smart-classify-route"
  - id: "smart-execute-routing"
    type: "recipe"
    recipe: "smart-execute-routing"
  - id: "smart-reflect-loop"
    type: "recipe"
    recipe: "smart-reflect-loop"
  - id: "smart-validate-summarize"
    type: "recipe"
    recipe: "smart-validate-summarize"
"#,
    )
    .expect("write smart-orchestrator recipe");
    for recipe in [
        "default-workflow",
        "investigation-workflow",
        "smart-classify-route",
        "smart-execute-routing",
        "smart-reflect-loop",
        "smart-validate-summarize",
    ] {
        fs::write(
            recipes.join(format!("{recipe}.yaml")),
            format!(
                "name: \"{recipe}\"\nsteps:\n  - id: smoke\n    type: bash\n    command: 'true'\n"
            ),
        )
        .expect("write companion recipe");
    }
    fs::write(
        recipes.join("_recipe_manifest.json"),
        r#"{
  "smart-classify-route": "250c8da0ee348745",
  "smart-execute-routing": "11612506ae846a47",
  "smart-orchestrator": "8d55ee4817dbc815",
  "smart-reflect-loop": "7b8101dfce096480",
  "smart-validate-summarize": "007548c49e9654fb"
}
"#,
    )
    .expect("write recipe manifest");
}

#[test]
fn install_command_launcher_is_not_cargo() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let binary = write_success_executable(temp.path(), "amplihack");
    let cmd = amplihack_install_command_with_binary(
        &binary,
        Path::new("/tmp/amplihack-test-home"),
        Path::new("/tmp/amplihack-hooks"),
        Path::new("/tmp/amplihack-source"),
    );
    let program = Path::new(cmd.get_program());
    assert_ne!(
        program.file_name(),
        Some(OsStr::new("cargo")),
        "install completeness tests must launch a prebuilt amplihack binary, not Cargo"
    );

    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(
        !args.iter().any(|arg| arg == "run" || arg == "build"),
        "install completeness tests must not request nested builds; args: {args:?}"
    );
}

#[test]
fn binary_resolver_prefers_probe_bin_over_cargo_bin_and_fallback() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let probe_bin = write_success_executable(temp.path(), "probe-amplihack");
    let cargo_bin = write_success_executable(temp.path(), "cargo-amplihack");
    let fallback = write_success_executable(temp.path(), "fallback-amplihack");

    let resolved = resolve_amplihack_binary_from(
        Some(OsString::from(&probe_bin)),
        Some(OsString::from(&cargo_bin)),
        &fallback,
    )
    .expect("resolve probe binary");

    assert_eq!(resolved, probe_bin);
}

#[test]
fn binary_resolver_uses_cargo_bin_when_probe_bin_is_unset() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let cargo_bin = write_success_executable(temp.path(), "cargo-amplihack");
    let fallback = write_success_executable(temp.path(), "fallback-amplihack");

    let resolved = resolve_amplihack_binary_from(None, Some(OsString::from(&cargo_bin)), &fallback)
        .expect("resolve Cargo-provided binary");

    assert_eq!(resolved, cargo_bin);
}

#[test]
fn binary_resolver_uses_existing_fallback_without_env_paths() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let fallback = write_success_executable(temp.path(), "fallback-amplihack");

    let resolved = resolve_amplihack_binary_from(None, None, &fallback).expect("resolve fallback");

    assert_eq!(resolved, fallback);
}

#[test]
fn binary_resolver_rejects_invalid_probe_bin_instead_of_falling_back() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let missing_probe = temp.path().join("missing-probe-amplihack");
    let cargo_bin = write_success_executable(temp.path(), "cargo-amplihack");
    let fallback = write_success_executable(temp.path(), "fallback-amplihack");

    let error = resolve_amplihack_binary_from(
        Some(OsString::from(&missing_probe)),
        Some(OsString::from(&cargo_bin)),
        &fallback,
    )
    .expect_err("invalid AMPLIHACK_PROBE_BIN must fail");

    assert!(
        error.contains("AMPLIHACK_PROBE_BIN") && error.contains("existing executable file"),
        "unexpected resolver error: {error}"
    );
}

#[test]
fn binary_resolver_fails_clearly_when_no_prebuilt_binary_is_available() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let fallback = temp.path().join("missing-fallback-amplihack");

    let error = resolve_amplihack_binary_from(None, None, &fallback)
        .expect_err("missing prebuilt binary must fail");

    assert!(
        error.contains("no prebuilt amplihack binary found")
            && error.contains("AMPLIHACK_PROBE_BIN")
            && error.contains(&fallback.display().to_string()),
        "unexpected resolver error: {error}"
    );
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
    let hooks_bin = write_install_tooling(home.path());
    create_bundle_missing_skills(source.path());

    let output = amplihack_install_command(home.path(), &hooks_bin, source.path())
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
    let hooks_bin = write_install_tooling(home.path());
    let workspace = workspace_root();

    amplihack_install_command(home.path(), &hooks_bin, workspace)
        .assert()
        .success();

    let source_bundle = workspace.join("amplifier-bundle");
    let staged_claude = home.path().join(".amplihack/.claude");
    let staged_bundle = home.path().join(".amplihack/amplifier-bundle");

    let source_skills = immediate_child_dirs(&source_bundle.join("skills"));
    let source_agents = immediate_child_dirs(&source_bundle.join("agents"));
    let source_commands = immediate_child_dirs(&source_bundle.join("commands"));

    for (source_category, staged_category, children) in [
        ("skills", "skills", &source_skills),
        ("agents", "agents", &source_agents),
        ("commands", "commands", &source_commands),
    ] {
        for child in children {
            let staged = staged_claude.join(staged_category).join(child);
            assert!(
                staged.is_dir(),
                "source amplifier-bundle/{source_category}/{child} must be staged at {}",
                staged.display()
            );
        }
    }

    for child in &source_skills {
        let staged = staged_bundle.join("skills").join(child);
        assert!(
            staged.is_dir(),
            "source skill {child} must also be present in the staged full amplifier-bundle at {}",
            staged.display()
        );
    }

    let source_skill_count = source_skills.len();
    let staged_skill_count = immediate_child_dirs(&staged_claude.join("skills")).len();
    assert!(
        staged_skill_count >= source_skill_count,
        "staged skill count must be at least source skill count; source={source_skill_count}, \
         staged={staged_skill_count}"
    );
}
