//! CI case controller. Independent cases run even when a preceding case fails.
use super::environment::{IsolatedCase, observe_git, private_dir};
use super::evidence::{self, Assertion, FailureKind, Report};
use super::hosting::{HostingStore, option};
use super::lifecycle::{Event, TrackedChild, read_event};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const BASE: &str = "3af9633bd9244169c16434a5fc59a88af3c9bb24";
pub fn config(case: &IsolatedCase) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(
        case.root.join("config.json"),
    )?)?)
}
pub fn save(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    private_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let next = target.join(entry.file_name());
        ensure!(
            !kind.is_symlink(),
            "staged asset is a symlink: {}",
            entry.path().display()
        );
        if kind.is_dir() {
            copy_tree(&entry.path(), &next)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), next)?;
        }
    }
    Ok(())
}
fn argument(args: &[String], key: &str) -> Result<PathBuf> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let environment_key = format!(
        "AMPLIHACK_2806_{}",
        key.trim_start_matches('-').replace('-', "_").to_uppercase()
    );
    let value = option(&refs, key)
        .map(String::from)
        .or_else(|| std::env::var(environment_key).ok())
        .ok_or_else(|| anyhow::anyhow!("missing {key}"))?;
    Ok(PathBuf::from(value))
}

pub fn run_all(args: &[String]) -> Result<()> {
    let evidence_root = argument(args, "--evidence")?;
    private_dir(&evidence_root)?;
    let evidence_root = evidence_root.canonicalize()?;
    let sha = argument(args, "--tested-sha")?
        .to_string_lossy()
        .into_owned();
    let mut report = Report::new(&evidence_root, &sha)?;
    let allowances: Vec<evidence::Allowance> = serde_json::from_slice(&fs::read(
        argument(args, "--assets")?.join("tests/gadugi/issue-2806/expected-failures.json"),
    )?)?;
    for allowance in allowances {
        let baseline_source = argument(args, "--assets")?
            .join("tests/gadugi/issue-2806")
            .join(&allowance.evidence_path);
        let baseline_target =
            super::environment::confined_path(&evidence_root, &allowance.evidence_path)?;
        private_dir(baseline_target.parent().context("baseline parent")?)?;
        fs::copy(baseline_source, baseline_target)?;
        report.add_allowance(allowance)?;
    }
    report.persist()?;
    let network =
        super::environment::network_interfaces(&fs::read_to_string("/proc/self/net/dev")?)?;
    save(
        &evidence_root.join("prerequisites.json"),
        &json!({"network_interfaces":network,"network_source":"/proc/self/net/dev","tested_sha":sha}),
    )?;
    ensure!(
        network == ["lo"],
        "scenario requires a CI network namespace with only loopback; observed {network:?}"
    );
    // Evidence and case roots are retained; TempDir would delete work needed for diagnosis.
    let run_root = tempfile::Builder::new().prefix("qa2806-").tempdir()?.keep();
    fs::set_permissions(&run_root, fs::Permissions::from_mode(0o700))?;
    save(
        &evidence_root.join("identity.json"),
        &json!({"tested_sha":sha,"production_base":BASE,"runner_revision":fs::read_to_string(argument(args,"--assets")?.join("claude-plugin/recipe-runner.rev"))?.trim(),"private_root":run_root,"network_interfaces":network}),
    )?;
    for name in evidence::CASES {
        let case = IsolatedCase::create(&run_root.join(name))?;
        let result = (|| -> Result<()> {
            stage(&case, args, name)?;
            match *name {
                "build" => build(&case, &mut report),
                "drive-control" | "drive-missing" => drive(&case, &mut report, name),
                "finalization-valid" | "finalization-missing" | "finalization-unreadable" => {
                    finalization(&case, &mut report, name)
                }
                _ => cleanup(&case, &mut report, name),
            }
        })();
        if let Err(error) = result {
            fs::write(
                case.evidence.join("harness-error.txt"),
                format!("{error:#}\n"),
            )?;
            report.record(Assertion::failed(
                name,
                &format!("{name}.harness"),
                FailureKind::Harness,
                &format!("{error:#}"),
            ))?;
        }
        // Copy evidence out of the sweep's entire work root after each independent case.
        copy_tree(&case.evidence, &evidence_root.join(name))?;
        println!("CASE_RECORDED: {name}");
        report.persist()?;
    }
    ensure!(
        report.success(),
        "case evidence did not satisfy acceptance; see {}",
        evidence_root.display()
    );
    println!("ALL_CASES_PASSED");
    Ok(())
}
fn stage(case: &IsolatedCase, args: &[String], name: &str) -> Result<()> {
    let assets = argument(args, "--assets")?.canonicalize()?;
    // Stage only assets these real recipes consume. Unrelated skill bundles
    // contain compatibility symlinks and are not execution prerequisites.
    for area in [
        "recipes",
        "tools",
        "agents",
        "skills/qa-team",
        "skills/merge-ready",
        "skills/auto-drive-to-merge",
        "skills/crusty-old-engineer",
    ] {
        copy_tree(
            &assets.join("amplifier-bundle").join(area),
            &case.home.join(".amplihack/amplifier-bundle").join(area),
        )?;
    }
    copy_tree(
        &assets.join("tests/gadugi/issue-2806/responses"),
        &case.root.join("responses"),
    )?;
    let fixture = std::env::current_exe()?.canonicalize()?;
    for (key, source) in [
        ("amplihack", argument(args, "--amplihack")?),
        ("recipe-runner-rs", argument(args, "--runner")?),
        ("cargo", argument(args, "--cargo")?),
        ("rustc", argument(args, "--rustc")?),
        ("pre-commit", argument(args, "--precommit")?),
        ("gh", fixture.clone()),
        ("claude", fixture.clone()),
        ("codex", fixture.clone()),
        ("git-remote-issue2806", fixture.clone()),
        ("model-launcher", fixture),
    ] {
        ensure!(
            source.is_absolute() && source.is_file(),
            "executable missing: {}",
            source.display()
        );
        let target = case.root.join("bin").join(key);
        if [
            "gh",
            "claude",
            "codex",
            "git-remote-issue2806",
            "model-launcher",
        ]
        .contains(&key)
        {
            // Launchers canonicalize executable paths. Preserve the fixture's
            // invocation name even after that canonicalization.
            fs::copy(source, target)?;
        } else {
            symlink(source, target)?;
        }
    }
    let toolchain = argument(args, "--cargo")?;
    let toolchain = toolchain
        .parent()
        .context("cargo has no toolchain directory")?;
    for tool in ["rustfmt", "cargo-fmt"] {
        let source = toolchain.join(tool);
        ensure!(
            source.is_file(),
            "required toolchain component missing: {}",
            source.display()
        );
        symlink(source, case.root.join("bin").join(tool))?;
    }
    let origin = case.root.join("origin.git");
    case.git(
        &case.root,
        &[
            "init",
            "--bare",
            "-q",
            "-b",
            "main",
            origin.to_str().context("origin path")?,
        ],
    )?;
    let checkout = case.root.join("project-drive-pr1");
    case.git(
        &case.root,
        &[
            "clone",
            "-q",
            origin.to_str().context("origin path")?,
            checkout.to_str().context("checkout path")?,
        ],
    )?;
    fs::write(
        case.root.join("gitconfig"),
        r#"[remote "origin"]
vcs = issue2806
[commit]
gpgsign = false
"#,
    )?;
    save(
        &case.root.join("config.json"),
        &json!({"case":name,"checkout":checkout,"origin":origin,"environment":{
            "AMPLIHACK_SOURCE_ROOT":case.home.join(".amplihack"),"RECIPE_RUNNER_RS_PATH":case.root.join("bin/recipe-runner-rs"),
            "AMPLIHACK_AGENT_BINARY":"codex","AMPLIHACK_NO_UPDATE_CHECK":"1","AMPLIHACK_SKIP_REFLECTION":"1",
        "AMPLIHACK_SKIP_AUTO_INSTALL":"1","AMPLIHACK_NO_FRESHNESS_CHECK":"1","AMPLIHACK_NONINTERACTIVE":"1",
        "AMPLIHACK_SKIP_POWER_STEERING":"1","AMPLIHACK_ROTATE_AUTOSTART":"0","AMPLIHACK_DISABLE_BLARIFY":"1",
        "AMPLIHACK_GOVERNOR":"off","AMPLIHACK_USAGE_LIMIT_WAIT":"0",
        "AMPLIHACK_CODEX_BINARY_PATH":case.root.join("bin/codex"),"AMPLIHACK_CLAUDE_BINARY_PATH":case.root.join("bin/claude"),
        "AMPLIHACK_LAUNCHER_BINARY":case.root.join("bin/model-launcher"),
            "GIT_ALLOW_PROTOCOL":"file:issue2806","CARGO_HOME":case.home.join(".cargo"),"PRE_COMMIT_HOME":case.home.join(".cache/pre-commit")
        }}),
    )?;
    case.git(
        &checkout,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/fixture/project.git",
        ],
    )?;
    private_dir(&checkout.join("src"))?;
    fs::write(
        checkout.join("Cargo.toml"),
        "[package]\nname = \"answer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
    )?;
    fs::write(
        checkout.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"answer\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        checkout.join("src/lib.rs"),
        "pub fn answer() -> u32 { 0 }\n",
    )?;
    fs::write(checkout.join("expected.txt"), "42\n")?;
    fs::write(checkout.join(".gitignore"), "target/\nworktrees/\n")?;
    fs::write(
        checkout.join(".pre-commit-config.yaml"),
        "repos:\n  - repo: local\n    hooks:\n      - id: answer\n        name: answer content\n        language: system\n        entry: /usr/bin/cmp answer.txt expected.txt\n        pass_filenames: false\n        always_run: true\n",
    )?;
    case.git(&checkout, &["add", "."])?;
    case.git(&checkout, &["commit", "-qm", "Baseline answer project"])?;
    case.git(&checkout, &["push", "-qu", "origin", "main"])?;
    HostingStore::create(
        &case.evidence.join("hosting.json"),
        "fixture/project",
        &origin,
    )?;
    save(
        &case.evidence.join("isolation.json"),
        &json!({"root":case.root,"mode":"0700","home":case.home,"work_root":case.work,"origin":origin,"clients":[case.root.join("bin/gh"),case.root.join("bin/claude"),case.root.join("bin/codex")]}),
    )?;
    if name != "build" {
        seed_published(case)?;
    }
    Ok(())
}
pub fn checkout(case: &IsolatedCase) -> Result<PathBuf> {
    Ok(PathBuf::from(
        config(case)?["checkout"]
            .as_str()
            .context("checkout missing")?,
    ))
}
pub fn origin(case: &IsolatedCase) -> Result<PathBuf> {
    Ok(PathBuf::from(
        config(case)?["origin"].as_str().context("origin missing")?,
    ))
}
fn seed_published(case: &IsolatedCase) -> Result<()> {
    let repo = checkout(case)?;
    case.git(&repo, &["checkout", "-qb", "feat/answer"])?;
    super::responses::implement(case, &repo)?;
    fs::write(
        repo.join("README.md"),
        "# Answer\n\nanswer() returns 42. answer.txt contains 42 followed by a newline.\nRun cargo test --workspace --locked and pre-commit run --all-files.\n",
    )?;
    case.git(&repo, &["add", "answer.txt", "src/lib.rs", "README.md"])?;
    case.git(&repo, &["commit", "-qm", "Implement answer"])?;
    case.git(&repo, &["push", "-qu", "origin", "feat/answer"])?;
    let mut host = HostingStore::open(&case.evidence.join("hosting.json"))?;
    ensure!(
        host.invoke(
            &repo,
            &[
                "pr",
                "create",
                "--head",
                "feat/answer",
                "--base",
                "main",
                "--title",
                "Implement answer",
                "--body",
                "Closes #7"
            ]
        )?
        .exit_code
            == 0,
        "seed PR publication failed"
    );
    // Factory callers launch from main with the PR checked out in a linked worktree.
    case.git(&repo, &["checkout", "-q", "main"])?;
    case.git(
        &repo,
        &["worktree", "add", "-q", "worktrees/pr-1", "feat/answer"],
    )?;
    Ok(())
}
pub fn run_recipe(
    case: &IsolatedCase,
    name: &str,
    repo: &Path,
    context: &[(&str, String)],
) -> Result<std::process::Output> {
    let mut command = case.command(&case.root.join("bin/amplihack"))?;
    command.args(["recipe", "run", name, "-w"]).arg(repo).args([
        "--format",
        "json",
        "--no-detach",
        "--verbose",
    ]);
    for (key, value) in context {
        command.arg("-c").arg(format!("{key}={value}"));
    }
    command.current_dir(repo).stdin(Stdio::null());
    let output = command.output()?;
    let label = Path::new(name)
        .file_stem()
        .context("recipe name")?
        .to_string_lossy();
    fs::write(
        case.evidence.join(format!("{label}.stdout")),
        &output.stdout,
    )?;
    fs::write(
        case.evidence.join(format!("{label}.stderr")),
        &output.stderr,
    )?;
    save(
        &case.evidence.join(format!("{label}.exit.json")),
        &json!({"exit_code":output.status.code()}),
    )?;
    snapshot_runs(case)?;
    Ok(output)
}
fn snapshot_runs(case: &IsolatedCase) -> Result<()> {
    let runs = case.work.join("runs");
    if runs.is_dir() {
        copy_tree(&runs, &case.evidence.join("runs"))?;
    }
    Ok(())
}
fn assert_value(
    report: &mut Report,
    case: &str,
    suffix: &str,
    passed: bool,
    reason: &str,
) -> Result<()> {
    let id = format!("{case}.{suffix}");
    report.record(if passed {
        Assertion::passed(case, &id)
    } else {
        Assertion::failed(case, &id, FailureKind::Regression, reason)
    })
}
fn build(case: &IsolatedCase, report: &mut Report) -> Result<()> {
    let repo = checkout(case)?;
    let task: Value = serde_json::from_slice(&fs::read(case.root.join("responses/build.json"))?)?;
    let output = run_recipe(
        case,
        "smart-orchestrator",
        &repo,
        &[
            (
                "task_description",
                task["task"].as_str().context("task missing")?.into(),
            ),
            ("repo_path", repo.display().to_string()),
            ("workflow", "default-workflow".into()),
            ("force_single_workstream", "true".into()),
            ("no_merge", "true".into()),
            ("should_merge", "false".into()),
            ("issue_number", "7".into()),
            ("base_branch", "main".into()),
        ],
    )?;
    let terminal: Value = serde_json::from_slice(&output.stdout)
        .context("smart-orchestrator terminal record is not JSON")?;
    save(&case.evidence.join("terminal.json"), &terminal)?;
    let terminal_success = output.status.success()
        && terminal["success"] == true
        && terminal["recipe_name"] == "smart-orchestrator";
    let hosting = HostingStore::open(&case.evidence.join("hosting.json"))?;
    let prs = hosting.open_prs()?;
    assert_value(
        report,
        "build",
        "terminal",
        terminal_success,
        "smart-orchestrator did not report terminal success",
    )?;
    assert_value(
        report,
        "build",
        "open-pr",
        prs.len() == 1,
        "build did not publish exactly one open PR",
    )?;
    if let Some(pr) = prs.first() {
        let recorded = case.evidence.join("implementation-worktree.json");
        let worktree = if recorded.is_file() {
            Some(PathBuf::from(serde_json::from_slice::<String>(&fs::read(
                recorded,
            )?)?))
        } else {
            None
        };
        if let Some(worktree) = worktree {
            let observed = evidence::observe_build(
                &worktree,
                &origin(case)?,
                &hosting,
                pr.number,
                "answer.txt",
            )?;
            save(&case.evidence.join("build-observation.json"), &observed)?;
            let exact_content = observe_git(&worktree, &["show", "HEAD:answer.txt"])? == "42";
            assert_value(
                report,
                "build",
                "content",
                exact_content,
                "committed answer does not satisfy issue 7",
            )?;
            assert_value(
                report,
                "build",
                "commit",
                observed.local_committed,
                "implementation not committed",
            )?;
            assert_value(
                report,
                "build",
                "origin",
                observed.remote_matches,
                "origin does not match implementation",
            )?;
            assert_value(
                report,
                "build",
                "pr-head",
                observed.open_pr_matches,
                "PR is not current and open",
            )?;
            return assert_value(
                report,
                "build",
                "complete",
                terminal_success
                    && exact_content
                    && observed.local_committed
                    && observed.remote_matches
                    && observed.open_pr_matches,
                "build acceptance incomplete",
            );
        }
    }
    assert_value(
        report,
        "build",
        "complete",
        false,
        "no retained published implementation worktree",
    )
}

fn finalization(case: &IsolatedCase, report: &mut Report, name: &str) -> Result<()> {
    let repo = checkout(case)?.join("worktrees/pr-1");
    let host = HostingStore::open(&case.evidence.join("hosting.json"))?;
    host.pr(1)?;
    let context = case.tmp.join("authoritative.json");
    let mut value = json!({"repo_path":repo,"worktree_setup":{"worktree_path":repo,"branch_name":"feat/answer","base_ref":"origin/main"},"branch_name":"feat/answer","base_ref":"origin/main","remote_host_type":"github","pr_number":"1","pr_url":"https://github.com/fixture/project/pull/1","publish_state_reached":"true","implementation_terminal_evidence":{"implementation_completed":"true"},"verification_terminal_evidence":{"verification_completed":"false"},"finalizer_step_status":{"status":"ok","reporting_failure":"false"}});
    save(&context, &value)?;
    let helper = case
        .home
        .join(".amplihack/amplifier-bundle/tools/workflow_agentic_finalization.sh");
    let collect = case
        .command(Path::new("/usr/bin/env"))?
        .arg(format!("AMPLIHACK_CONTEXT_FILE={}", context.display()))
        .arg("/bin/bash")
        .arg(&helper)
        .args(["collect", "--base", "main"])
        .current_dir(&repo)
        .output()?;
    // env -i assignments for this boundary are explicit in its child, not inherited.
    fs::write(case.evidence.join("collect.stderr"), &collect.stderr)?;
    ensure!(
        collect.status.success(),
        "collect failed: {}",
        String::from_utf8_lossy(&collect.stderr)
    );
    value["finalization_evidence"] = serde_json::from_slice(&collect.stdout)?;
    save(&context, &value)?;
    let mut command = case.command(&std::env::current_exe()?)?;
    command
        .arg("finalize-boundary")
        .arg(&context)
        .arg(&repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    fs::write(
        case.evidence.join("boundary.pid"),
        format!("{}\n", child.id()),
    )?;
    let mut reader = BufReader::new(child.stdout.take().context("boundary stdout")?);
    let mut event = String::new();
    ensure!(
        reader.read_line(&mut event)? > 0,
        "boundary EOF before publication event"
    );
    let event: Value = serde_json::from_str(&event)?;
    ensure!(
        event["event"] == "before_context_consumption",
        "wrong boundary event"
    );
    let fault = name.strip_prefix("finalization-").context("case name")?;
    match fault {
        "missing" => fs::remove_file(&context)?,
        "unreadable" => fs::set_permissions(&context, fs::Permissions::from_mode(0o0))?,
        _ => {}
    }
    let read_error = match fs::read(&context) {
        Ok(_) => None,
        Err(e) => Some(format!("{:?}", e.kind())),
    };
    if fault == "unreadable" {
        ensure!(
            read_error.as_deref() == Some("PermissionDenied"),
            "executing identity can still read protected context"
        );
    }
    writeln!(child.stdin.take().context("boundary stdin")?, "consume")?;
    let mut output = String::new();
    std::io::Read::read_to_string(&mut reader, &mut output)?;
    let completed = child.wait_with_output()?;
    fs::write(case.evidence.join("validate.stdout"), &output)?;
    fs::write(case.evidence.join("validate.stderr"), &completed.stderr)?;
    let terminal: Value = serde_json::from_str(&output)?;
    let fresh = HostingStore::open(&case.evidence.join("hosting.json"))?.pr(1)?;
    let observed = evidence::PendingFinalization {
        context: fault.into(),
        context_read_error: read_error,
        fault_before_consumption: fault != "valid",
        origin_head: observe_git(&origin(case)?, &["rev-parse", "refs/heads/feat/answer"])?,
        pr_head: HostingStore::open(&case.evidence.join("hosting.json"))?
            .pr(1)?
            .head_sha,
        pr_state: HostingStore::open(&case.evidence.join("hosting.json"))?
            .pr(1)?
            .state,
        checks: match fresh.checks {
            super::hosting::Checks::Pending => "pending",
            super::hosting::Checks::Green => "green",
            super::hosting::Checks::Failed => "failed",
        }
        .into(),
        terminal_state: terminal["terminal_state"]
            .as_str()
            .context("typed terminal_state absent")?
            .into(),
        terminal_success: terminal["terminal_success"] == "true"
            || terminal["terminal_success"] == true,
        exit_code: completed.status.code().context("boundary signal exit")?,
    };
    save(&case.evidence.join("pending-observation.json"), &observed)?;
    evidence::validate_pending_prerequisites(&observed).context(
        "pending-check fixture prerequisites failed; regression allowances are forbidden",
    )?;
    assert_value(
        report,
        name,
        "classification",
        evidence::validate_pending_finalization(&observed).is_ok(),
        &format!("{} with {fault} context", observed.terminal_state),
    )
}

pub fn finalize_boundary(args: &[String]) -> Result<()> {
    ensure!(
        args.len() == 2,
        "finalization boundary needs context and checkout"
    );
    let case = IsolatedCase::open(Path::new(&std::env::var("AMPLIHACK_2806_ROOT")?))?;
    println!(
        "{}",
        json!({"event":"before_context_consumption","pid":std::process::id()})
    );
    std::io::stdout().flush()?;
    let mut release = String::new();
    std::io::stdin().read_line(&mut release)?;
    ensure!(release.trim() == "consume", "invalid boundary release");
    use std::os::unix::process::CommandExt;
    // Replace this child with the production helper under a fresh explicit env -i.
    let mut command = std::process::Command::new("/usr/bin/env");
    command
        .env_clear()
        .arg("-i")
        .arg(format!("HOME={}", case.home.display()))
        .arg(format!(
            "PATH={}:/usr/bin:/bin",
            case.root.join("bin").display()
        ))
        .arg(format!(
            "AMPLIHACK_HOME={}",
            case.home.join(".amplihack").display()
        ))
        .arg(format!("AMPLIHACK_2806_ROOT={}", case.root.display()))
        .arg("GIT_CONFIG_GLOBAL=/dev/null")
        .arg("GIT_CONFIG_SYSTEM=/dev/null")
        .arg("GIT_ALLOW_PROTOCOL=file")
        .arg(format!("AMPLIHACK_CONTEXT_FILE={}", args[0]))
        .arg("/bin/bash")
        .arg(
            case.home
                .join(".amplihack/amplifier-bundle/tools/workflow_agentic_finalization.sh"),
        )
        .arg("validate")
        .current_dir(&args[1]);
    Err(command.exec().into())
}

fn drive(case: &IsolatedCase, report: &mut Report, name: &str) -> Result<()> {
    let repo = checkout(case)?;
    fs::write(
        repo.join("worktrees/pr-1/caller-dirty.txt"),
        "operator work must survive\n",
    )?;
    let output = run_recipe(
        case,
        "auto-drive-to-merge",
        &repo,
        &[
            ("repo_path", repo.display().to_string()),
            ("pr_number", "1".into()),
            (
                "task_description",
                "Drive PR #1 through review and merge; merge is authorized.".into(),
            ),
            ("no_merge", "false".into()),
            ("autodrive_dry_run", "false".into()),
        ],
    )?;
    let mut host = HostingStore::open(&case.evidence.join("hosting.json"))?;
    let location = case.evidence.join("drive-location.json");
    let located = if location.is_file() {
        let observed: evidence::DriveObservation = serde_json::from_slice(&fs::read(location)?)?;
        evidence::validate_drive_location(&observed).is_ok()
    } else {
        false
    };
    let dirty = repo.join("worktrees/pr-1/caller-dirty.txt");
    assert_value(
        report,
        name,
        "caller-dirty",
        fs::read(&dirty)? == b"operator work must survive\n",
        "drive changed operator dirty work",
    )?;
    if !located {
        let proof: Value = serde_json::from_slice(
            &fs::read(case.evidence.join("worktree-missing.json")).context(
                "drive did not reach worktree resolution; no regression attribution permitted",
            )?,
        )?;
        let caller_common = observe_git(
            &repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        ensure!(
            proof["private_common_dir"] != caller_common
                && proof["pr_state"] == "OPEN"
                && proof["pr_head"] == host.pr(1)?.head_sha,
            "missing-worktree prerequisites are not established"
        );
    }
    assert_value(report, name, "worktree", located, "no-worktree")?;
    if !located && name == "drive-missing" {
        for suffix in ["gates", "retained", "merge", "complete"] {
            report.record(Assertion::blocked(
                name,
                &format!("{name}.{suffix}"),
                &format!("{name}.worktree"),
                "private worktree resolution did not reach review and merge",
            ))?;
        }
        return Ok(());
    }
    let readback = host.invoke(&repo, &["pr", "view", "1", "--json", "state,headRefOid"])?;
    let readback: Value = serde_json::from_slice(&readback.stdout)?;
    save(&case.evidence.join("merged-readback.json"), &readback)?;
    let gates = fs::read_to_string(case.evidence.join("preflight-gates.jsonl"))?;
    let gates: Vec<Value> = gates
        .lines()
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    assert_value(
        report,
        name,
        "gates",
        ["build-preflight", "crusty-preflight", "merge-preflight"]
            .iter()
            .all(|contract| gates.iter().any(|gate| gate["contract"] == *contract)),
        "not all three entry gates executed",
    )?;
    let observed: evidence::DriveObservation =
        serde_json::from_slice(&fs::read(case.evidence.join("drive-location.json"))?)?;
    let retained = observe_git(
        &observed.resolved_worktree,
        &["show", &format!("{}:answer.txt", observed.observed_head)],
    )? == "42";
    assert_value(
        report,
        name,
        "retained",
        retained,
        "drive did not retain committed implementation",
    )?;
    case.git(
        &observed.resolved_worktree,
        &[
            "bundle",
            "create",
            case.evidence
                .join("drive.bundle")
                .to_str()
                .context("bundle path")?,
            "--all",
        ],
    )?;
    let pr = host.pr(1)?;
    let merge = pr.state == "MERGED"
        && host.invocations()?.iter().any(|call| {
            call.args
                .windows(2)
                .any(|pair| pair[0] == "--match-head-commit" && pair[1] == pr.head_sha)
        });
    assert_value(
        report,
        name,
        "merge",
        merge,
        "production finalizer did not merge observed head",
    )?;
    assert_value(
        report,
        name,
        "complete",
        output.status.success() && merge && located,
        "complete private-copy drive acceptance not met",
    )
}

pub fn hold_work(args: &[String]) -> Result<()> {
    ensure!(
        args.is_empty(),
        "hold-work takes its recorded runtime from the runner"
    );
    let case = IsolatedCase::open(Path::new(&std::env::var("AMPLIHACK_2806_ROOT")?))?;
    let runtime = PathBuf::from(std::env::var("AMPLIHACK_WORKFLOW_RUNTIME_DIR")?);
    ensure!(runtime.starts_with(&case.work), "runtime escapes work root");
    let repo = runtime.join("retained-repository");
    case.git(
        &runtime,
        &[
            "clone",
            "-q",
            origin(&case)?.to_str().context("origin path")?,
            repo.to_str().context("runtime repo path")?,
        ],
    )?;
    case.git(&repo, &["checkout", "-q", "feat/answer"])?;
    fs::write(
        repo.join("unpublished.txt"),
        "committed work not on origin\n",
    )?;
    case.git(&repo, &["add", "unpublished.txt"])?;
    case.git(&repo, &["commit", "-qm", "Unpublished interrupted work"])?;
    fs::write(repo.join("continue.txt"), "unfinished work\n")?;
    let commit = observe_git(&repo, &["rev-parse", "HEAD"])?;
    save(
        &case.evidence.join("interrupted-location.json"),
        &json!({"runtime":runtime,"repo":repo,"run_id":std::env::var("AMPLIHACK_RUN_ID")?,"commit":commit}),
    )?;
    let mut pipe = std::os::unix::net::UnixStream::connect(case.tmp.join("events.sock"))?;
    writeln!(
        pipe,
        "{}",
        serde_json::to_string(&Event::WorkReady {
            run_id: std::env::var("AMPLIHACK_RUN_ID")?,
            pid: std::process::id(),
            commit,
            dirty_path: "continue.txt".into()
        })?
    )?;
    let mut release = String::new();
    ensure!(
        BufReader::new(pipe).read_line(&mut release)? > 0,
        "interrupted work event channel closed"
    );
    ensure!(release.trim() == "finish", "invalid lifecycle release");
    Ok(())
}

fn cleanup_output(case: &IsolatedCase, label: &str) -> Result<(Value, i32)> {
    let output = case
        .command(&case.root.join("bin/amplihack"))?
        .args([
            "cleanup",
            "--apply",
            "--keep-hours",
            "0",
            "--only",
            "run-tmp",
            "--format",
            "json",
        ])
        .output()?;
    fs::write(
        case.evidence.join(format!("{label}.stdout")),
        &output.stdout,
    )?;
    fs::write(
        case.evidence.join(format!("{label}.stderr")),
        &output.stderr,
    )?;
    let exit = output
        .status
        .code()
        .context("cleanup terminated by signal")?;
    save(
        &case.evidence.join(format!("{label}.exit.json")),
        &json!({"exit_code":exit}),
    )?;
    Ok((
        serde_json::from_slice(&output.stdout).context("cleanup did not return its JSON report")?,
        exit,
    ))
}
fn sweep(case: &IsolatedCase, label: &str) -> Result<Value> {
    let (value, exit) = cleanup_output(case, label)?;
    ensure!(
        exit == 0,
        "cleanup command failed with exit {exit}; see {label}.stdout/stderr"
    );
    Ok(value)
}

fn cleanup(case: &IsolatedCase, report: &mut Report, name: &str) -> Result<()> {
    let caller = checkout(case)?.join("worktrees/pr-1");
    let recipe = case.root.join("cleanup-run.yaml");
    let exe = std::env::current_exe()?;
    fs::write(
        &recipe,
        format!(
            "name: issue-2806-cleanup\nsteps:\n  - id: interrupted-work\n    type: bash\n    command: |\n      exec '{}' hold-work\n",
            exe.display()
        ),
    )?;
    let listener = std::os::unix::net::UnixListener::bind(case.tmp.join("events.sock"))?;
    let mut command = case.command(&case.root.join("bin/amplihack"))?;
    command
        .args(["recipe", "run", "--no-detach"])
        .arg(&recipe)
        .arg("-w")
        .arg(&caller)
        .stdout(fs::File::create(case.evidence.join("recipe.stdout"))?)
        .stderr(fs::File::create(case.evidence.join("recipe.stderr"))?);
    let mut child = TrackedChild::spawn(&mut command, &case.evidence.join("cleanup-run.pid"))?;
    let mut pipe = super::lifecycle::accept_event(&listener, &child)?;
    let event = read_event(&mut BufReader::new(pipe.try_clone()?))?;
    save(&case.evidence.join("ready.json"), &event)?;
    let Event::WorkReady { commit, .. } = event else {
        anyhow::bail!("cleanup probe did not create work")
    };
    let location: Value =
        serde_json::from_slice(&fs::read(case.evidence.join("interrupted-location.json"))?)?;
    let repo = PathBuf::from(
        location["repo"]
            .as_str()
            .context("interrupted repo absent")?,
    );
    let runtime = PathBuf::from(location["runtime"].as_str().context("runtime absent")?);
    let before = evidence::observe_retention(
        &repo,
        &origin(case)?,
        &commit,
        "continue.txt",
        b"unfinished work\n",
    )?;
    ensure!(
        before.commit_reachable
            && before.dirty_content_intact
            && before.unpushed
            && before.branch_usable,
        "cleanup preconditions were not actually created"
    );
    save(&case.evidence.join("before-retention.json"), &before)?;
    case.git(
        &repo,
        &[
            "bundle",
            "create",
            case.evidence
                .join("before-cleanup.bundle")
                .to_str()
                .context("bundle path")?,
            "--all",
        ],
    )?;
    if name == "cleanup-clean" {
        fs::remove_file(repo.join("continue.txt"))?;
        case.git(&repo, &["push", "-q", "origin", "feat/answer"])?;
        writeln!(pipe, "finish")?;
        ensure!(child.wait()?.success(), "clean control run failed");
        let sweep = sweep(case, "cleanup")?;
        save(&case.evidence.join("cleanup.json"), &sweep)?;
        snapshot_runs(case)?;
        return assert_value(
            report,
            name,
            "removed",
            !runtime.exists(),
            "clean fully pushed runtime was not removable",
        );
    }
    if name == "cleanup-removal-failure" {
        // A protected directory deterministically prevents removal under CI's
        // unprivileged identity. No fault relies on elapsed age or a race.
        fs::remove_file(repo.join("continue.txt"))?;
        case.git(&repo, &["push", "-q", "origin", "feat/answer"])?;
        // RuntimeGuard repairs modes inside its candidate. Protect its
        // parent instead, so the directory's final unlink actually fails.
        let protected = runtime.parent().context("runtime has no parent")?;
        ensure!(
            protected == case.work.join("r"),
            "removal fault requires the observed short-runtime layout"
        );
        fs::set_permissions(protected, fs::Permissions::from_mode(0o500))?;
        writeln!(pipe, "finish")?;
        child.wait()?;
        let (first, first_exit) = cleanup_output(case, "cleanup-first")?;
        let (second, second_exit) = cleanup_output(case, "cleanup-second")?;
        for (report, exit) in [(&first, first_exit), (&second, second_exit)] {
            ensure!([0, 1].contains(&exit), "unexpected cleanup exit {exit}");
            let entries = report["entries"]
                .as_array()
                .context("cleanup entries missing")?;
            ensure!(
                entries
                    .iter()
                    .filter(|entry| entry["action"] == "failed")
                    .all(|entry| entry["path"] == runtime.to_string_lossy().as_ref()
                        && entry["reason"]
                            .as_str()
                            .is_some_and(|reason| reason.contains("Permission denied"))),
                "unrelated cleanup failure cannot receive a regression allowance"
            );
            ensure!(
                exit == 0 || entries.iter().any(|entry| entry["action"] == "failed"),
                "cleanup failed without a removal-error record"
            );
        }
        save(&case.evidence.join("cleanup-first.json"), &first)?;
        save(&case.evidence.join("cleanup-second.json"), &second)?;
        let first_text =
            fs::read_to_string(case.evidence.join("recipe.stderr"))? + &first.to_string();
        let second_text = second.to_string();
        let observed =
            first_text.contains("Permission denied") || first_text.contains("permission denied");
        ensure!(
            observed,
            "removal-failure fixture did not cause an actual access error"
        );
        fs::set_permissions(protected, fs::Permissions::from_mode(0o700))?;
        return assert_value(
            report,
            name,
            "reported-once",
            !second_text.contains("Permission denied")
                && !second_text.contains("permission denied"),
            "same removal failure reported by repeated cleanup",
        );
    }
    let mut holder = if name == "cleanup-other-stopped" {
        let mut command = case.command(&exe)?;
        command
            .arg("hold-directory")
            .current_dir(&repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut holder =
            TrackedChild::spawn(&mut command, &case.evidence.join("directory-holder.pid"))?;
        let ready = read_event(&mut BufReader::new(holder.take_stdout()?))?;
        ensure!(
            ready == Event::ProcessReady { pid: holder.id() },
            "directory holder readiness belongs to another process"
        );
        Some(holder)
    } else {
        None
    };
    child.interrupt_and_wait()?;
    drop(pipe);
    // The second run's ordinary end-of-run sweep must preserve the earlier
    // stopped runtime with exactly the same rules as its owner's cleanup.
    if let Some(holder) = holder.as_mut() {
        let held = evidence::observe_retention(
            &repo,
            &origin(case)?,
            &commit,
            "continue.txt",
            b"unfinished work\n",
        )?;
        ensure!(
            held.commit_reachable && held.dirty_content_intact && held.unpushed,
            "active directory holder failed to retain sweep preconditions"
        );
        save(&case.evidence.join("held-after-interruption.json"), &held)?;
        writeln!(holder.take_stdin()?, "finish")?;
        ensure!(holder.wait()?.success(), "directory holder failed to exit");
    }
    if name == "cleanup-other-stopped" {
        let other = case.root.join("other-run.yaml");
        fs::write(
            &other,
            "name: issue-2806-other\nsteps:\n  - id: complete\n    type: bash\n    command: 'git rev-parse HEAD'\n",
        )?;
        let output = run_recipe(
            case,
            other.to_str().context("other recipe path")?,
            &caller,
            &[],
        )?;
        ensure!(output.status.success(), "other recorded run failed");
    }
    let cleanup = sweep(case, "cleanup")?;
    save(&case.evidence.join("cleanup.json"), &cleanup)?;
    snapshot_runs(case)?;
    let observed = if repo.is_dir() {
        Some(evidence::observe_retention(
            &repo,
            &origin(case)?,
            &commit,
            "continue.txt",
            b"unfinished work\n",
        )?)
    } else {
        None
    };
    save(&case.evidence.join("retention.json"), &observed)?;
    assert_value(
        report,
        name,
        "commit",
        observed
            .as_ref()
            .is_some_and(|v| v.commit_reachable && v.unpushed),
        "cleanup lost unpushed work",
    )?;
    assert_value(
        report,
        name,
        "dirty",
        observed.as_ref().is_some_and(|v| v.dirty_content_intact),
        "cleanup lost uncommitted work",
    )?;
    let explanation = cleanup["entries"].as_array().is_some_and(|entries| {
        entries.iter().any(|entry| {
            entry["reason"].as_str().is_some_and(|reason| {
                reason.contains("dirty")
                    || reason.contains("unpushed")
                    || reason.contains("uncommitted")
            })
        })
    });
    assert_value(
        report,
        name,
        "explanation",
        explanation,
        if observed.is_none() {
            "cleanup did not explain preservation"
        } else {
            "preserved work lacked an explanation"
        },
    )?;
    if observed.is_some() {
        case.git(&repo, &["add", "continue.txt"])?;
        case.git(&repo, &["commit", "-qm", "Continue retained work"])?;
        assert_value(
            report,
            name,
            "continuation",
            observe_git(&repo, &["show", "HEAD:continue.txt"])? == "unfinished work",
            "retained branch cannot continue",
        )?;
    } else {
        report.record(Assertion::blocked(
            name,
            &format!("{name}.continuation"),
            &format!("{name}.commit"),
            "runtime was removed",
        ))?;
    }
    Ok(())
}
