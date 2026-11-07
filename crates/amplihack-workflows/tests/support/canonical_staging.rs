//! Shared, isolated Git fixtures. No staging decision is implemented here.
#![allow(dead_code)] // This module is shared by several independent test targets.

use serde::Deserialize;
use serde_json::{Value as Json, json};
use serde_yaml::Value;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

#[derive(Clone, Debug, Deserialize)]
pub struct Case {
    pub asset: String,
    pub step_id: String,
    pub mutation: bool,
    pub working_dir: String,
    pub checkpoint_owner: String,
    pub authorised_edit: bool,
}

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

pub fn inventory() -> Vec<Case> {
    serde_json::from_str(include_str!("../fixtures/canonical_staging/inventory.json")).unwrap()
}

pub fn asset_path(asset: &str) -> PathBuf {
    let directory = std::env::var_os("CANONICAL_STAGING_RECIPE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("amplifier-bundle/recipes"));
    directory.join(format!("{asset}.yaml"))
}

pub fn recipe(asset: &str) -> Value {
    serde_yaml::from_slice(&fs::read(asset_path(asset)).unwrap()).unwrap()
}

pub fn step(case: &Case) -> Value {
    recipe(&case.asset)["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(&case.step_id))
        .unwrap()
        .clone()
}

pub fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

/// Child-only environment; never inherit credentials, Git selectors or session markers.
pub fn isolated(command: &mut Command, home: &Path) {
    command
        .env_clear()
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env(
            "PATH",
            std::env::var("CANONICAL_STAGING_TOOL_PATH")
                .unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin".into()),
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    if let Some(tmp) = std::env::var_os("TMPDIR") {
        command.env("TMPDIR", tmp);
    }
}

pub fn checked(command: &mut Command) -> Output {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "command {command:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

pub fn sha256(path: &Path) -> String {
    let out = checked(Command::new("shasum").args(["-a", "256"]).arg(path));
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .into()
}

/// Installed executables may be invoked normally, but the assignment permits
/// supporting file reads only within the workspace. Record their version when
/// a binary digest would require reading outside that boundary.
pub fn executable_digest(path: &Path) -> Option<String> {
    let resolved = path.canonicalize().unwrap();
    resolved
        .starts_with("/Users/ryan/src/bb")
        .then(|| sha256(&resolved))
}

pub struct Fixture {
    pub temp: TempDir,
    pub home: PathBuf,
    pub caller: PathBuf,
    pub review: PathBuf,
    pub task: PathBuf,
    pub evidence: PathBuf,
    pub launcher: PathBuf,
}

impl Fixture {
    pub fn new(label: &str, edges: bool) -> Self {
        Self::build(label, edges, true, true)
    }

    pub fn checkpoint(label: &str) -> Self {
        Self::build(label, false, false, true)
    }

    pub fn checkpoint_clean(label: &str) -> Self {
        Self::build(label, false, false, false)
    }

    fn build(label: &str, edges: bool, tracked_runtime: bool, partial_task: bool) -> Self {
        let evidence_root = PathBuf::from(
            std::env::var_os("CANONICAL_STAGING_EVIDENCE_DIR")
                .expect("set CANONICAL_STAGING_EVIDENCE_DIR to the assigned report directory"),
        );
        fs::create_dir_all(&evidence_root).unwrap();
        let evidence = tempfile::Builder::new()
            .prefix(&format!("{label}-"))
            .tempdir_in(evidence_root)
            .unwrap()
            .keep();
        let temp = tempfile::Builder::new()
            .prefix("canonical-staging-")
            .tempdir()
            .unwrap();
        let base = temp.path().canonicalize().unwrap();
        let home = base.join("home");
        fs::create_dir(&home).unwrap();
        let caller = base.join("caller");
        let review = base.join("review");
        let task = base.join("task");
        let launcher = base.join("private-adapter");
        executable(
            &launcher,
            include_str!("../fixtures/canonical_staging/private_adapter.py"),
        );
        let f = Self {
            temp,
            home,
            caller,
            review,
            task,
            evidence,
            launcher,
        };
        for repo in [&f.caller, &f.review] {
            fs::create_dir(repo).unwrap();
            f.git(repo, &["init", "--template=", "-b", "main"]);
            f.git(repo, &["config", "user.name", "Fixture Author"]);
            f.git(repo, &["config", "user.email", "fixture@example.invalid"]);
            f.git(repo, &["config", "core.fileMode", "true"]);
            f.git(repo, &["config", "commit.gpgSign", "false"]);
            for (path, body) in [
                ("user.txt", "base\n"),
                ("source.txt", "base\n"),
                ("deleted.txt", "base\n"),
                ("old name.txt", "base\n"),
                (".amplihack/tracked.json", "base\n"),
            ] {
                if path.starts_with(".amplihack/") && !tracked_runtime {
                    continue;
                }
                let p = repo.join(path);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(p, body).unwrap();
            }
            f.git(repo, &["add", "--all"]);
            f.git(repo, &["commit", "-m", "fixture baseline"]);
        }
        f.git(
            &f.review,
            &["worktree", "add", "-b", "task", f.task.to_str().unwrap()],
        );
        for repo in [&f.caller, &f.review, &f.task] {
            if repo != &f.task || partial_task {
                f.partial_stage(repo, edges);
            }
        }
        f
    }

    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>, cwd: &Path) -> Command {
        let mut c = Command::new(program);
        isolated(&mut c, &self.home);
        c.current_dir(cwd);
        c
    }

    pub fn git(&self, repo: &Path, args: &[&str]) -> Vec<u8> {
        checked(self.command("git", repo).args(args)).stdout
    }

    fn partial_stage(&self, repo: &Path, edges: bool) {
        fs::write(repo.join("user.txt"), "user staged content\n").unwrap();
        fs::set_permissions(repo.join("user.txt"), fs::Permissions::from_mode(0o755)).unwrap();
        self.git(repo, &["add", "--", "user.txt"]);
        fs::write(repo.join("user.txt"), "user unstaged content\n").unwrap();
        fs::set_permissions(repo.join("user.txt"), fs::Permissions::from_mode(0o644)).unwrap();
        if edges {
            self.git(repo, &["rm", "--", "deleted.txt"]);
            fs::write(repo.join("deleted.txt"), "unstaged recreation\n").unwrap();
            self.git(repo, &["mv", "--", "old name.txt", "renamed\nfile.txt"]);
            fs::write(repo.join("renamed\nfile.txt"), "unstaged rename edit\n").unwrap();
            for name in ["-leading", ":(glob)*", "tab\tname", "space name"] {
                fs::write(repo.join(name), "staged\n").unwrap();
                self.git(repo, &["--literal-pathspecs", "add", "--", name]);
                fs::write(repo.join(name), "unstaged\n").unwrap();
            }
            symlink("user.txt", repo.join("source-link")).unwrap();
            self.git(repo, &["add", "--", "source-link"]);
            fs::remove_file(repo.join("source-link")).unwrap();
            symlink("source.txt", repo.join("source-link")).unwrap();
        }
    }

    pub fn index_path(&self, repo: &Path) -> PathBuf {
        let path = self.git(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        );
        PathBuf::from(String::from_utf8(path).unwrap().trim_end())
    }

    pub fn indexes(&self) -> Vec<(PathBuf, PathBuf)> {
        [&self.caller, &self.review, &self.task]
            .into_iter()
            .map(|p| (p.clone(), self.index_path(p)))
            .collect()
    }

    pub fn entries(&self, repo: &Path) -> Vec<u8> {
        self.git(repo, &["ls-files", "--stage", "-z"])
    }

    /// Read every raw index before issuing ANY post-run Git command.
    pub fn raw(
        &self,
        paths: &[(PathBuf, PathBuf)],
        label: &str,
    ) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        paths
            .iter()
            .enumerate()
            .map(|(i, (repo, path))| {
                let bytes = match fs::read(path) {
                    Ok(b) => Some(b),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => panic!("read index {}: {e}", path.display()),
                };
                if let Some(b) = &bytes {
                    fs::write(self.evidence.join(format!("{label}-{i}.index")), b).unwrap();
                }
                (repo.clone(), bytes)
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Variant {
    Actual,
    RemovedOptOut,
    DisabledNoOp,
    DisabledMutation,
    RepeatedSequence,
    HostileSelectors,
}

pub struct Run {
    pub f: Fixture,
    pub receipt: Json,
    pub violations: Vec<String>,
}

/// The REAL runner owns automatic staging. This harness only projects one source
/// step and substitutes deterministic agent output and absolute fixture paths.
pub fn run(case: &Case, variant: Variant, edges: bool) -> Run {
    let f = Fixture::new(&case.step_id, edges);
    let source = recipe(&case.asset);
    let mut projected = source.clone();
    let mut selected = step(case);
    let requirements_path = PathBuf::from(
        std::env::var_os("CANONICAL_STAGING_REQUIREMENTS_FILE")
            .expect("supply the parent's complete unchanged requirements document"),
    );
    static REQUIREMENTS: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let requirements_bytes = REQUIREMENTS.get_or_init(|| fs::read(&requirements_path).unwrap());
    let requirements = std::str::from_utf8(requirements_bytes).unwrap();
    assert!(
        !requirements.trim().is_empty(),
        "complete requirements document is empty"
    );
    fs::write(f.evidence.join("requirements.txt"), requirements.as_bytes()).unwrap();
    let target = if case.working_dir == "{{review_dir}}" {
        &f.review
    } else if case.working_dir == "<runner working directory>" {
        &f.caller
    } else {
        &f.task
    };
    let map = selected.as_mapping_mut().unwrap();
    match variant {
        Variant::RemovedOptOut => {
            map.remove(Value::String("auto_stage".into()));
        }
        Variant::DisabledNoOp | Variant::DisabledMutation | Variant::HostileSelectors => {
            map.insert("auto_stage".into(), false.into());
        }
        Variant::Actual | Variant::RepeatedSequence => {}
    }
    let preserve_directory_templates =
        std::env::var("CANONICAL_STAGING_PRESERVE_DIRECTORY_TEMPLATES").as_deref() == Ok("1");
    if !preserve_directory_templates {
        selected["working_dir"] = target.to_str().unwrap().into();
    }
    selected["prompt"] =
        format!("Fixture data only. Do not execute this prompt.\n\n{requirements}").into();
    // Preserve conditions, parse_json, retry settings, output, model and all
    // other step fields. Context supplies the actual branch preconditions.
    projected["steps"] = if matches!(variant, Variant::RepeatedSequence) {
        let mut first = selected.clone();
        first["output"] = "fixture_sequence_result".into();
        first["parse_json"] = true.into();
        let mut repeat = selected.clone();
        repeat["id"] = format!("{}-repeat", case.step_id).into();
        repeat["condition"] = "fixture_sequence_result.status == 'OK'".into();
        Value::Sequence(vec![first, repeat])
    } else {
        Value::Sequence(vec![selected.clone()])
    };
    projected
        .as_mapping_mut()
        .unwrap()
        .remove(Value::String("output".into()));
    projected["name"] = "canonical-staging-projection".into();
    let projected_path = f.evidence.join("projection.yaml");
    fs::write(&projected_path, serde_yaml::to_string(&projected).unwrap()).unwrap();
    let runner = PathBuf::from(
        std::env::var_os("CANONICAL_STAGING_RUNNER")
            .expect("set CANONICAL_STAGING_RUNNER to the actual recipe-runner-rs executable"),
    );
    assert!(
        runner.is_absolute() && runner.is_file(),
        "runner must be an absolute executable path"
    );
    let runner_hash = executable_digest(&runner);
    let runner_version = checked(
        f.command(&runner, &f.caller)
            .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
            .arg("--version"),
    )
    .stdout;
    // A private agent definition resolves the real role without consulting
    // installed agents, repository instructions or personal directories.
    // v0.5.0 has no --agent-dir/--no-default-search switches. Its normal
    // resolver searches this private HOME first. All other search roots are
    // within the disposable caller checkout, never the user's home.
    let agents = f.home.join(".amplihack/.claude/agents");
    fs::create_dir_all(&agents).unwrap();
    let agent_name = selected["agent"].as_str().unwrap();
    let agent_path = agents.join(format!("{}.md", agent_name.split(':').next_back().unwrap()));
    fs::write(
        &agent_path,
        "---\nname: fixture\n---\nDeterministic fixture role.\n",
    )
    .unwrap();
    let trace = f.evidence.join("adapter.json");
    let mut cmd = f.command(&runner, &f.caller);
    cmd.arg(&projected_path)
        .args([
            "-C",
            f.caller.to_str().unwrap(),
            "--agent-binary",
            "codex",
            "--output-format",
            "json",
        ])
        .env("AMPLIHACK_LAUNCHER_BINARY", &f.launcher)
        .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
        .env("AMPLIHACK_STAGING_TRACE", &trace)
        .env(
            "AMPLIHACK_STAGING_MUTATE",
            if case.mutation { "1" } else { "0" },
        )
        .env(
            "AMPLIHACK_STAGING_NOOP",
            if matches!(variant, Variant::DisabledNoOp) {
                "1"
            } else {
                "0"
            },
        );
    let context = json!({"resume_checkpoint":"", "goal_already_met":"false",
        "clarified_requirements":{"requires_integration_work":"true", "requires_api_design":"true", "requires_database_design":"true"},
        "terminal_state":{"terminal_success":"false", "should_finalize":"true", "should_run_ci_wait":"true", "should_publish":"true"},
        "terminal_adjudication_evidence":{"mechanical_verdict":"FAILED"},
        "crusty_verdict":{"crusty_verdict":"CONCERNS", "concern_count":1},
        "worktree_setup":{"worktree_path":f.task}, "repo_path":f.task, "review_dir":f.review});
    for (key, value) in context.as_object().unwrap() {
        let value = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        cmd.args(["--set", &format!("{key}={value}")]);
    }
    let paths = f.indexes();
    let hostile_index = f.temp.path().join("alternate.index");
    let hostile_before = if matches!(variant, Variant::HostileSelectors) {
        let bytes = fs::read(f.index_path(&f.caller)).unwrap();
        fs::write(&hostile_index, &bytes).unwrap();
        fs::write(f.evidence.join("alternate.before.index"), &bytes).unwrap();
        cmd.env("GIT_DIR", f.caller.join(".git"))
            .env("GIT_WORK_TREE", &f.caller)
            .env("GIT_INDEX_FILE", &hostile_index);
        Some(bytes)
    } else {
        None
    };
    let entries: BTreeMap<_, _> = paths
        .iter()
        .map(|(repo, _)| (repo.clone(), f.entries(repo)))
        .collect();
    let before = f.raw(&paths, "before");
    for (i, (repo, _)) in paths.iter().enumerate() {
        fs::write(
            f.evidence.join(format!("user-{i}.before.blob")),
            f.git(repo, &["show", ":user.txt"]),
        )
        .unwrap();
        assert_eq!(
            f.git(repo, &["show", ":user.txt"]),
            b"user staged content\n"
        );
        assert!(
            f.git(repo, &["ls-files", "--stage", "--", "user.txt"])
                .starts_with(b"100755 ")
        );
        assert_eq!(
            fs::read(repo.join("user.txt")).unwrap(),
            b"user unstaged content\n"
        );
    }
    let command = format!("{cmd:?}");
    fs::write(
        f.evidence.join("execution.json"),
        serde_json::to_vec_pretty(&json!({"command":command,"state":"running"})).unwrap(),
    )
    .unwrap();
    let output = cmd.output().unwrap();
    let after = f.raw(&paths, "after"); // FIRST operation after runner return.
    let hostile_after = if hostile_before.is_some() {
        let bytes = fs::read(&hostile_index).unwrap();
        fs::write(f.evidence.join("alternate.after.index"), &bytes).unwrap();
        Some(bytes)
    } else {
        None
    };
    fs::write(f.evidence.join("runner.stdout"), &output.stdout).unwrap();
    fs::write(f.evidence.join("runner.stderr"), &output.stderr).unwrap();
    fs::write(
        f.evidence.join("execution.json"),
        serde_json::to_vec_pretty(
            &json!({"command":command,"state":"returned","exit_status":output.status.code()}),
        )
        .unwrap(),
    )
    .unwrap();
    let adapter = fs::read(&trace)
        .ok()
        .and_then(|b| serde_json::from_slice::<Json>(&b).ok());
    let mut violations = Vec::new();
    if hostile_before != hostile_after {
        violations.push("index-integrity: inherited alternate index changed".into());
    }
    let mut semantics = true;
    for (repo, _) in &paths {
        if before[repo] != after[repo] {
            violations.push(format!(
                "index-integrity: raw index changed at {}",
                repo.display()
            ));
        }
        let now = f.entries(repo);
        fs::write(
            f.evidence.join(format!(
                "{}-user.after.blob",
                repo.file_name().unwrap().to_string_lossy()
            )),
            f.git(repo, &["show", ":user.txt"]),
        )
        .unwrap();
        fs::write(
            f.evidence.join(format!(
                "{}-entries.before",
                repo.file_name().unwrap().to_string_lossy()
            )),
            &entries[repo],
        )
        .unwrap();
        fs::write(
            f.evidence.join(format!(
                "{}-entries.after",
                repo.file_name().unwrap().to_string_lossy()
            )),
            &now,
        )
        .unwrap();
        if entries[repo] != now {
            semantics = false;
            violations.push(format!(
                "index-integrity: staged blobs or modes changed at {}",
                repo.display()
            ));
        }
    }
    let mut observable_writes = Vec::new();
    let mut fixture_observations_valid = true;
    if !matches!(variant, Variant::DisabledNoOp) && adapter.is_some() {
        for (metadata, expected) in [
            (".amplihack/indexing.pid", b"4242\n".as_slice()),
            (
                ".claude/runtime/sessions.jsonl",
                b"{\"fixture\":true}\n".as_slice(),
            ),
            (
                ".amplihack/tracked.json",
                b"{\"modified\":true}\n".as_slice(),
            ),
        ] {
            let contents = fs::read(target.join(metadata)).ok();
            if contents.as_deref() != Some(expected) {
                fixture_observations_valid = false;
                violations.push(format!("fixture metadata absent or incorrect: {metadata}"));
            }
            observable_writes.push(json!({"path":metadata,"working_tree_bytes":contents}));
            if metadata == ".amplihack/tracked.json" {
                continue;
            }
            let tracked = f.git(
                target,
                &["--literal-pathspecs", "ls-files", "-z", "--", metadata],
            );
            if !tracked.is_empty() {
                violations.push(format!(
                    "index-integrity: runtime metadata staged: {metadata}"
                ));
            }
        }
        if f.git(target, &["show", ":.amplihack/tracked.json"]) != b"base\n" {
            violations.push("index-integrity: tracked runtime modification staged".into());
        }
        if case.mutation && f.git(target, &["show", ":source.txt"]) != b"base\n" {
            violations.push("index-integrity: deferred source modification staged".into());
        }
        if case.mutation
            && !f
                .git(target, &["ls-files", "-z", "--", "new source.txt"])
                .is_empty()
        {
            violations.push("index-integrity: new deferred source path staged".into());
        }
        if case.mutation {
            for (path, expected) in [
                ("source.txt", b"deferred source modification\n".as_slice()),
                ("new source.txt", b"deferred new source\n".as_slice()),
            ] {
                let contents = fs::read(target.join(path)).ok();
                if contents.as_deref() != Some(expected) {
                    fixture_observations_valid = false;
                    violations.push(format!(
                        "fixture source mutation absent or incorrect: {path}"
                    ));
                }
                observable_writes.push(json!({"path":path,"working_tree_bytes":contents}));
            }
        }
    }
    let receipt = json!({"schema_version":1,"case_id":case.step_id,"asset":case.asset,
        "variant":format!("{variant:?}"),"edge_entries":edges,"candidate_head":String::from_utf8(f.git(&root(), &["rev-parse","HEAD"])).unwrap().trim(),
        "source_asset_sha256":sha256(&asset_path(&case.asset)),"source_asset_path":asset_path(&case.asset),
        "projection_sha256":sha256(&projected_path),"requirements_sha256":sha256(&f.evidence.join("requirements.txt")),"projection_substitutions":{
            "steps":"one source agent step; preceding/following steps excluded", "prompt":"deterministic data",
            "working_dir":{"source":case.working_dir,"resolved":target,"source_template_preserved":preserve_directory_templates},"context_overrides":context,
            "agent_definition":"private inert role", "output":"recipe summary removed; source step output preserved except the documented repeated-sequence case",
            "sequence_override":matches!(variant, Variant::RepeatedSequence)},
        "runner_path":runner,"runner_sha256":runner_hash,"runner_version":String::from_utf8_lossy(&runner_version),
        "runner_digest_limit":"Installed binary bytes outside the authorised workspace are not read; version probe identifies the executable.",
        "candidate_uncommitted_paths_nul":f.git(&root(), &["diff","--name-only","-z","HEAD"]),
        "candidate_untracked_paths_nul":f.git(&root(), &["ls-files","--others","--exclude-standard","-z"]),
        "command":command,"command_argv":std::iter::once(runner.to_string_lossy().into_owned()).chain(cmd.get_args().map(|a| a.to_string_lossy().into_owned())).collect::<Vec<_>>(),"exit_status":output.status.code(),"adapter_execution":adapter,
        "fixture_observations_valid":fixture_observations_valid,"observable_writes":observable_writes,
        "index_paths":paths,"inherited_alternate_index":hostile_before.as_ref().map(|_| &hostile_index),
        "raw_index_equal":before==after && hostile_before==hostile_after,"semantic_index_equal":semantics,
        "violations":violations,"terminal_outcome":if output.status.success(){"runner_success"}else{"runner_failure"}});
    fs::write(
        f.evidence.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    Run {
        f,
        receipt,
        violations,
    }
}

pub fn executed(run: &Run) {
    assert_eq!(
        run.receipt["exit_status"],
        0,
        "runner failed before integrity assertion: {}",
        run.f.evidence.display()
    );
    assert_eq!(
        run.receipt["adapter_execution"]["completed"],
        true,
        "adapter did not complete; this is NOT a valid RED/control: {}",
        run.f.evidence.display()
    );
    assert_eq!(
        run.receipt["fixture_observations_valid"],
        true,
        "required metadata/source writes are missing; this is NOT a valid RED/control: {}",
        run.f.evidence.display()
    );
    let cwd = run.receipt["adapter_execution"]["cwd"].as_str().unwrap();
    assert_eq!(
        cwd,
        run.receipt["projection_substitutions"]["working_dir"]["resolved"]
            .as_str()
            .unwrap()
    );
}
