//! Action adapter for the explicitly supported recipe tasks. No call-count dispatch.
use super::{
    agent, driver,
    environment::{IsolatedCase, checked, observe_git, private_dir},
    hosting::{Checks, HostingStore},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn context(key: &str) -> Result<Value> {
    if let Ok(path) = std::env::var("AMPLIHACK_CONTEXT_FILE") {
        let data: Value = serde_json::from_slice(&fs::read(path)?)?;
        let value = data.get(key).cloned().unwrap_or(Value::Null);
        return Ok(match value {
            Value::String(text) => serde_json::from_str(&text).unwrap_or(Value::String(text)),
            other => other,
        });
    }
    match std::env::var(format!("RECIPE_VAR_{key}")) {
        Ok(value) => Ok(serde_json::from_str(&value).unwrap_or(Value::String(value))),
        Err(std::env::VarError::NotPresent) => Ok(Value::Null),
        Err(error) => Err(error.into()),
    }
}
fn text_context(key: &str) -> Result<String> {
    Ok(context(key)?.as_str().unwrap_or("").into())
}
pub fn find_worktree(repo: &Path, branch: &str) -> Result<Option<PathBuf>> {
    let list = observe_git(repo, &["worktree", "list", "--porcelain"])?;
    for record in list.split("\n\n") {
        if record
            .lines()
            .any(|line| line == format!("branch refs/heads/{branch}"))
        {
            return Ok(record
                .lines()
                .find_map(|line| line.strip_prefix("worktree ").map(PathBuf::from)));
        }
    }
    Ok(None)
}
pub fn implement(case: &IsolatedCase, repo: &Path) -> Result<()> {
    let value: Value = serde_json::from_slice(&fs::read(case.root.join("responses/build.json"))?)?;
    let plan: agent::Plan = serde_json::from_value(value["implementation"].clone())?;
    let receipt = agent::execute_plan(case, repo, &plan)?;
    driver::save(
        &case.evidence.join("implementation-worktree.json"),
        &repo.display().to_string(),
    )?;
    driver::save(
        &case
            .evidence
            .join(format!("implementation-{}.json", uuid::Uuid::new_v4())),
        &receipt,
    )?;
    Ok(())
}
fn worktree(case: &IsolatedCase) -> Result<PathBuf> {
    let setup = context("worktree_setup")?;
    if let Some(path) = setup["worktree_path"].as_str() {
        return Ok(PathBuf::from(path));
    }
    let path = text_context("repo_path")?;
    if !path.is_empty() {
        return Ok(PathBuf::from(path));
    }
    driver::checkout(case)
}
pub fn check(case: &IsolatedCase, repo: &Path, tool: &str, args: &[&str]) -> Result<Value> {
    let executable = case.root.join("bin").join(tool);
    let output = case
        .command(&executable)?
        .args(args)
        .current_dir(repo)
        .output()?;
    let prefix = format!("check-{}", uuid::Uuid::new_v4());
    let log = case.evidence.join(format!("{prefix}.log"));
    let receipt = case.evidence.join(format!("{prefix}.exit"));
    let mut bytes = output.stdout.clone();
    bytes.extend_from_slice(&output.stderr);
    fs::write(&log, bytes)?;
    fs::write(
        &receipt,
        format!("{}\n", output.status.code().context("check signaled")?),
    )?;
    ensure!(
        output.status.success(),
        "{tool} {args:?} failed; see {}",
        log.display()
    );
    Ok(json!({"log":log,"exit_receipt":receipt}))
}
fn verify(case: &IsolatedCase, repo: &Path) -> Result<Value> {
    ensure!(
        fs::read(repo.join("answer.txt"))? == b"42\n",
        "answer content does not satisfy task"
    );
    check(case, repo, "cargo", &["test", "--workspace", "--locked"])
}
fn host(case: &IsolatedCase) -> Result<HostingStore> {
    HostingStore::open(&case.evidence.join("hosting.json"))
}
fn post(case: &IsolatedCase, body: &str) -> Result<()> {
    let repo = worktree(case)?;
    ensure!(
        host(case)?
            .invoke(&repo, &["pr", "comment", "1", "--body", body])?
            .exit_code
            == 0,
        "posting review failed"
    );
    Ok(())
}
fn preflight(case: &IsolatedCase, task: &str) -> Result<String> {
    let contract = if task.contains("Read build_preflight") {
        "resolve-pr"
    } else if task.contains("\"pr_found\":\"true|false|unknown\"") {
        "build-preflight"
    } else if task.contains("\"skip_reason\":\"\"") {
        "crusty-preflight"
    } else {
        "merge-preflight"
    };
    let manifest: Value =
        serde_json::from_slice(&fs::read(case.root.join("responses/contracts.json"))?)?;
    let fields = manifest["preflight_contracts"][contract]
        .as_array()
        .context("preflight contract absent from fixture manifest")?;
    let fields: Vec<&str> = fields
        .iter()
        .map(|field| field.as_str().context("contract field is not text"))
        .collect::<Result<_>>()?;
    let declaration = agent::Contract::json_object(contract, &fields);
    agent::select_contract(task, &[declaration])?;
    let cwd = std::env::current_dir()?;
    let pr = host(case)?.pr(1)?;
    let mut gates = OpenOptions::new()
        .create(true)
        .append(true)
        .open(case.evidence.join("preflight-gates.jsonl"))?;
    writeln!(
        gates,
        "{}",
        json!({"contract":contract,"cwd":cwd,"head":pr.head_sha,"common_dir":observe_git(&cwd,&["rev-parse","--path-format=absolute","--git-common-dir"] )?})
    )?;
    let mut found = find_worktree(&cwd, &pr.branch)?;
    if found.is_none() && driver::config(case)?["case"] == "drive-control" {
        let target = cwd
            .parent()
            .context("private checkout has no parent")?
            .join("worktrees/pr-1");
        private_dir(target.parent().context("worktree parent missing")?)?;
        // Explicit control preparation is recorded independently and is never
        // allowed in drive-missing. It does not stand in for production resolution.
        let branches = observe_git(
            &cwd,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
        )?;
        let mut args = vec!["worktree", "add", "-q"];
        if !branches.lines().any(|b| b == pr.branch) {
            args.extend(["-b", &pr.branch]);
        }
        args.push(target.to_str().context("worktree path")?);
        args.push(if branches.lines().any(|b| b == pr.branch) {
            &pr.branch
        } else {
            &pr.head_sha
        });
        case.git(&cwd, &args)?;
        driver::save(
            &case
                .evidence
                .join(format!("control-preparation-{}.json", uuid::Uuid::new_v4())),
            &json!({"cwd":cwd,"common_dir":observe_git(&cwd,&["rev-parse","--path-format=absolute","--git-common-dir"] )?,"head":pr.head_sha,"prepared":target}),
        )?;
        found = find_worktree(&cwd, &pr.branch)?;
    }
    let Some(repo) = found else {
        driver::save(
            &case.evidence.join("worktree-missing.json"),
            &json!({"executed":true,"prerequisites_satisfied":true,"private_common_dir":observe_git(&cwd,&["rev-parse","--path-format=absolute","--git-common-dir"] )?,"pr_head":pr.head_sha,"pr_state":pr.state,"contract":contract,"registered_worktrees":observe_git(&cwd,&["worktree","list","--porcelain"] )?}),
        )?;
        return Ok(json!({"pr_found":"true","pr":"1","pr_url":"https://github.com/fixture/project/pull/1","pr_state":"OPEN","branch":pr.branch,"head_sha":pr.head_sha,"should_run":"false","skip_reason":"no-worktree","base_branch":"main","base_sha":observe_git(&driver::origin(case)?,&["rev-parse","main"] )?,"state_dir":"","repo":"","review_dir":cwd,"reason":"no-worktree","blockers":["no-worktree"],"attempts":[]}).to_string());
    };
    let current = observe_git(&repo, &["rev-parse", "HEAD"])?;
    ensure!(
        current == pr.head_sha,
        "preflight local and hosting head differ"
    );
    let state = case.root.join("autodrive-state");
    let out = case
        .command(&case.root.join("bin/amplihack"))?
        .args(["orch", "helper", "autodrive", "state", "--directory"])
        .arg(&state)
        .current_dir(&repo)
        .output()?;
    checked(out, "private autodrive state")?;
    let out = case
        .command(&case.root.join("bin/amplihack"))?
        .args(["orch", "helper", "autodrive", "worktree", "--repo"])
        .arg(&repo)
        .args(["--head", &pr.head_sha, "--branch", &pr.branch])
        .output()?;
    checked(out, "verified linked worktree")?;
    let common = PathBuf::from(observe_git(
        &repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?);
    let caller = driver::checkout(case)?;
    let observation = super::evidence::DriveObservation {
        caller_common_dir: PathBuf::from(observe_git(
            &caller,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?),
        private_common_dir: common.clone(),
        execution_common_dir: PathBuf::from(observe_git(
            &cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?),
        launch_checkout: cwd.clone(),
        resolved_worktree: repo.clone(),
        observed_head: pr.head_sha.clone(),
        worktree_head: current,
        worktree_branch: Some(pr.branch.clone()),
        registered_linked_worktree: true,
    };
    if !case.evidence.join("drive-location.json").exists() {
        driver::save(&case.evidence.join("drive-location.json"), &observation)?;
    }
    let mut result = json!({"pr_found":"true","should_run":"true","skip_reason":"","pr":"1","pr_url":"https://github.com/fixture/project/pull/1","pr_state":"OPEN","branch":pr.branch,"head_sha":pr.head_sha,"base_branch":"main","base_sha":observe_git(&driver::origin(case)?,&["rev-parse","main"] )?,"state_dir":state,"repo":repo,"review_dir":cwd,"reason":"observed linked worktree and current local origin","blockers":[],"attempts":[],"merged_before_run":false,"handed_off":false,"terminal_state":"IMPLEMENTED_VERIFIED"});
    if task.contains("complete schema-1 Binding") {
        result["entry_evidence"] = json!({"pr_state":"OPEN","head_sha":pr.head_sha});
    }
    Ok(result.to_string())
}

pub fn respond(case: &IsolatedCase, task: &str) -> Result<String> {
    let heading = task.lines().next().context("empty agent task")?;
    let id = uuid::Uuid::new_v4();
    fs::write(case.evidence.join(format!("task-{id}.txt")), task)?;
    let mut calls = OpenOptions::new()
        .create(true)
        .append(true)
        .open(case.evidence.join("agents.jsonl"))?;
    writeln!(
        calls,
        "{}",
        json!({"heading":heading,"cwd":std::env::current_dir()?,"task_artifact":format!("task-{id}.txt")})
    )?;
    let build: Value = serde_json::from_slice(&fs::read(case.root.join("responses/build.json"))?)?;
    let repo = worktree(case)?;
    let response = if task.starts_with("=== [RECIPE PROGRESS] Step: classify-and-decompose ===") {
        json!({"task_type":"Development","goal":build["task"],"success_criteria":["verified implementation pushed and PR open"],"no_merge":true,"workstreams":[{"name":"answer","description":build["task"],"recipe":"default-workflow"}],"recommended_workflow":"default-workflow","workflow_reason":"one implementation workstream"}).to_string()
    } else if task.starts_with("# Step 2: Rewrite") {
        build["requirements"].to_string()
    } else if task.starts_with("# Step 5e:") {
        json!({"components":[{"name":"answer","action":"modify","purpose":"return 42"}],"files_to_change":["src/lib.rs"],"new_files":["answer.txt"],"test_files":["src/lib.rs"],"implementation_order":["implement answer and run cargo test"],"risks":[],"security_considerations":["no external input"]}).to_string()
    } else if task.starts_with("# Step 6b:") {
        json!({"status":"OK","feedback":"answer 42 acceptance matches design"}).to_string()
    } else if task.starts_with("# Step 7:") {
        fs::write(
            repo.join("src/lib.rs"),
            "pub fn answer() -> u32 { 0 }\n#[cfg(test)] mod tests { #[test] fn answer_is_42() { assert_eq!(super::answer(),42); } }\n",
        )?;
        let output = case
            .command(&case.root.join("bin/cargo"))?
            .args(["test", "--workspace", "--locked"])
            .current_dir(&repo)
            .output()?;
        fs::write(case.evidence.join("red-phase.log"), &output.stdout)?;
        ensure!(
            !output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("answer_is_42"),
            "TDD red phase was not an observed assertion failure"
        );
        "Files modified: src/lib.rs; cargo test failed at answer_is_42 as required before implementation.".into()
    } else if task.starts_with("# Step 8: Implement") {
        implement(case, &repo)?;
        let evidence = verify(case, &repo)?;
        format!(
            "Files modified: answer.txt, src/lib.rs\nDiff summary: implement answer 42.\nVerification: {evidence}"
        )
    } else if task.starts_with("# Step 8c:") {
        let evidence = verify(case, &repo)?;
        json!({"verdict":"WORK_VERIFIED","evidence":[evidence],"rationale":"actual answer content and cargo test verified"}).to_string()
    } else if task.starts_with("# Step 12:") {
        let workspace = verify(case, &repo)?;
        let precommit = check(case, &repo, "pre-commit", &["run", "--all-files"])?;
        json!({"status":"PASS","exit_code":0,"precommit_exit_code":0,"workspace_exit_code":0,"workspace_command":"cargo test --workspace --locked","validation_scope":"FULL_WORKSPACE","test_threads":"DEFAULT","evidence":[workspace,precommit]}).to_string()
    } else if task.starts_with("# Step 13:") {
        let first = verify(case, &repo)?;
        let second = check(case, &repo, "/usr/bin/cmp", &["answer.txt", "expected.txt"])?;
        json!({"status":"PASS","exit_code":0,"scenarios":[{"status":"PASS","exit_code":0,"name":"answer public function returns 42"},{"status":"PASS","exit_code":0,"name":"answer file matches expected bytes"}],"evidence":[first,second]}).to_string()
    } else if task.starts_with("Evaluate goal achievement after round 1.") {
        let prs = host(case)?.open_prs()?;
        ensure!(prs.len() == 1, "reflection found no single open PR");
        json!({"goal_status":"ACHIEVED","summary":format!("origin and open PR #{} at {}",prs[0].number,prs[0].head_sha)}).to_string()
    } else if task.starts_with("Outside-in testing validation check.") {
        "Outside-in evidence retained: cargo public API assertion and byte-exact answer file check."
            .into()
    } else if task.starts_with("Produce a concise execution summary.")
        || task.starts_with("# Agentic Finalization Narrative")
    {
        let prs = host(case)?.open_prs()?;
        format!(
            "Observed open PRs: {}. Completed check artifacts are retained separately.",
            serde_json::to_string(&prs)?
        )
    } else if task.starts_with("# Terminal-State Adjudication") {
        let prs = host(case)?.open_prs()?;
        ensure!(
            prs.len() == 1,
            "adjudication cannot find open PR carrying work"
        );
        format!(
            "PR #{} is open at {}.\nTERMINAL_VERDICT: SUCCESS",
            prs[0].number, prs[0].head_sha
        )
    } else if task.starts_with("Determine QA prerequisites using the trusted qa-team") {
        for tool in ["cargo", "rustc", "gh", "codex", "claude"] {
            ensure!(
                case.root.join("bin").join(tool).is_file(),
                "missing staged tool {tool}"
            );
        }
        json!({"prerequisites":["cargo","rustc","gh"],"blockers":[],"attempts":[]}).to_string()
    } else if task.starts_with("Use noninteractive command execution")
        && task.contains("Establish the PR identity")
    {
        preflight(case, task)?
    } else if task.starts_with("Return task_description and requirements from the inputs below") {
        json!({"task_description":build["task"],"requirements":"Implement answer 42. [amplihack:no-merge] Publish and leave PR open."}).to_string()
    } else if task.starts_with("Observe the exact PR from") {
        let pr = host(case)?.pr(1)?;
        ensure!(pr.state == "OPEN", "early merge before finalization");
        json!({"platform_state":"OPEN","action":"continue","blockers":[],"attempts":[]}).to_string()
    } else if task.starts_with("Read PR #") && task.contains("resolved_concerns") {
        let pr = host(case)?.pr(1)?;
        json!({"pr":"1","head_sha":pr.head_sha,"resolved_concerns":"","round_label":text_context("autodrive_round_label")?,"blockers":[],"attempts":[]}).to_string()
    } else if task.starts_with("# Crusty review") {
        let reviewed = text_context("reviewed_head_sha")?;
        let review_dir = PathBuf::from(text_context("review_dir")?);
        ensure!(
            observe_git(&review_dir, &["show", &format!("{reviewed}:answer.txt")])? == "42",
            "reviewed commit does not implement answer"
        );
        json!({"crusty_verdict":"CLEAN","concerns":[],"summary":format!("reviewed {reviewed}: answer function and content satisfy scoped task")}).to_string()
    } else if task
        .starts_with("Use noninteractive command execution with GH_PAGER=cat. Post this Crusty")
    {
        let pr = host(case)?.pr(1)?;
        post(
            case,
            &format!(
                "Crusty verdict: CLEAN at {}\n\nObserved answer implementation and verified tests.",
                pr.head_sha
            ),
        )?;
        "Crusty verdict posted at observed head.".into()
    } else if task.starts_with("Assess whether this Crusty review objective is complete") {
        let record = PathBuf::from(text_context("autodrive_round_record")?);
        let result = fs::read(&record).and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes).map_err(std::io::Error::other)
        });
        let complete = result.as_ref().is_ok_and(|v| {
            matches!(v["crusty_verdict"].as_str(), Some("CLEAN" | "MINOR_ONLY"))
                && v["reviewed_head_sha"] == v["head_sha"]
                && v["commits_this_round"] == 0
        });
        let state = text_context("autodrive_completion_state")?;
        checked(
            case.command(&case.root.join("bin/amplihack"))?
                .args([
                    "orch",
                    "helper",
                    "autodrive-completion-write",
                    "--private-state",
                    &state,
                    "--action",
                    if complete { "complete" } else { "evaluate" },
                    "--reason",
                    if complete {
                        "clean review at recorded head"
                    } else {
                        "round not complete at recorded head"
                    },
                ])
                .output()?,
            "Crusty completion writer",
        )?
    } else if task.starts_with("Clear inherited git state first")
        && task.contains("Synchronize only the linked worktree")
    {
        case.git(&repo, &["fetch", "origin", "main"])?;
        case.git(&repo, &["merge", "--ff-only", "origin/main"])?;
        json!({"base":"main","synced":"true","conflict":false,"head_sha":observe_git(&repo,&["rev-parse","HEAD"] )?,"git_observed":true,"blockers":[],"attempts":[]}).to_string()
    } else if task.starts_with("Clear inherited git state first")
        && task.contains("Execute criterion 1")
    {
        let helper = case
            .home
            .join(".amplihack/amplifier-bundle/tools/autodrive_qa_evidence.sh");
        let path = text_context("autodrive_qa_evidence")?;
        let mut command = case.command(Path::new("/usr/bin/env"))?;
        command
            .arg(format!("REPO_PATH={}", repo.display()))
            .arg("AUTODRIVE_QA_BASE=main")
            .arg(format!("AUTODRIVE_QA_EVIDENCE={path}"))
            .arg("/bin/bash")
            .arg(helper)
            .current_dir(&repo);
        checked(command.output()?, "qa measurement")?
    } else if task.starts_with("Clear inherited git state first")
        && task.contains("Observe every CI check")
    {
        verify(case, &repo)?;
        let mut hosting = host(case)?;
        hosting.set_checks(1, Checks::Green)?;
        let pr = hosting.pr(1)?;
        driver::save(
            &case.evidence.join("green-transition.json"),
            &json!({"head":pr.head_sha,"cause":"explicit controller transition after local verification"}),
        )?;
        json!({"ci_status":"GREEN","ci_total":1,"ci_pending":0,"ci_failing":0,"ci_signal":"fixture-verification explicitly completed","head_sha":pr.head_sha,"blockers":[],"attempts":[]}).to_string()
    } else if task.starts_with("Judge criterion 3") {
        let pr = host(case)?.pr(1)?;
        let expected = format!("Crusty verdict: CLEAN at {}", pr.head_sha);
        ensure!(
            pr.comments.iter().any(|comment| comment["body"]
                .as_str()
                .is_some_and(|body| body.lines().next() == Some(expected.as_str()))),
            "no authorized Crusty comment at observed head"
        );
        json!({"crusty_status":"DONE_CLEAN","crusty_reason":"authorized fixture review at observed head","crusty_reviewed_head_sha":pr.head_sha,"crusty_first_unreviewed_sha":""}).to_string()
    } else if task.starts_with("Use noninteractive command execution")
        && task.contains("Collect complete platform observations")
    {
        let pr = host(case)?.pr(1)?;
        ensure!(pr.checks == Checks::Green, "platform checks not green");
        json!({"platform":"github","host":"github.com","repository":"fixture/project","pr":1,"head_sha":pr.head_sha,"pr_state":pr.state,"merge_state":"CLEAN","approval_status":"MET","approval_source":"local hosting fixture configuration","approval_blocker":"","blockers":[],"attempts":[]}).to_string()
    } else if task.starts_with("# merge-ready assessment") {
        let pr = host(case)?.pr(1)?;
        let evidence = context("qa_evidence")?;
        ensure!(
            pr.checks == Checks::Green && evidence["qa_status"] == "PASS",
            "readiness lacks passing measured evidence"
        );
        let reviewed = text_context("reviewed_head_sha")?;
        ensure!(
            reviewed == pr.head_sha,
            "readiness head differs from reviewed commit"
        );
        let review = PathBuf::from(text_context("review_dir")?);
        ensure!(
            observe_git(&review, &["show", &format!("{reviewed}:README.md")])?.contains("42"),
            "readiness lacks user documentation at the assessed commit"
        );
        let changed = observe_git(&review, &["diff", "--name-only", "origin/main", &reviewed])?;
        ensure!(
            changed
                .lines()
                .all(|path| ["src/lib.rs", "answer.txt", "README.md"].contains(&path)),
            "readiness found unrelated changes"
        );
        let crusty = context("crusty_evidence")?;
        ensure!(
            crusty["crusty_status"] == "DONE_CLEAN"
                && crusty["crusty_reviewed_head_sha"] == pr.head_sha,
            "readiness lacks accepted Crusty evidence at head"
        );
        let criteria = [
            "1 QA: current-round Rust suite PASS; Gadugi exemption measured by trusted helper",
            "2 Documentation: README at assessed commit describes answer 42",
            "3 Quality: authorized Crusty CLEAN comment at assessed head",
            "4 Checks: fixture-verification explicitly green at assessed head",
            "5 Metadata: open PR 1 with head/base, title and scoped body",
            "6 Reviews: fixture policy requires zero approvals, no requested changes",
            "7 Conflicts: Git and hosting observe clean base relationship",
            "8 Protection: required fixture-verification check passed",
            "9 Issue: closes issue 7",
            "10 Discussion: no blocking fixture threads",
            "11 Finalization: production finalizer must merge at matched head and confirm merged state",
            "12 Evidence: write-up retained in PR discussion and description",
            "13 Scope: only answer.txt, src/lib.rs and README.md changed",
        ];
        ensure!(
            !pr.title.is_empty() && pr.body.contains("#7"),
            "readiness metadata is incomplete"
        );
        let body = format!(
            "Merge-ready assessment at {}\n\n{}\n\nReady.",
            pr.head_sha,
            criteria.join("\n")
        );
        let edited = host(case)?.invoke(
            &repo,
            &[
                "pr",
                "edit",
                "1",
                "--body",
                &format!("{}\n\n{}", pr.body, body),
            ],
        )?;
        ensure!(edited.exit_code == 0, "readiness description update failed");
        post(case, &body)?;
        body
    } else if task.starts_with("Observe PR #") && task.contains("before finalisation") {
        let pr = host(case)?.pr(1)?;
        format!("PR #1 is {}, not draft, at {}.", pr.state, pr.head_sha)
    } else if task.contains("one word")
        && task.contains("ci: pass")
        && task.starts_with("Read the merge-ready assessment between the markers")
    {
        ensure!(
            host(case)?.pr(1)?.checks == Checks::Green,
            "outcome cannot be ready with pending checks"
        );
        "ci: pass\nready".into()
    } else if task.starts_with("# Step ") && supported_review(heading) {
        if heading.starts_with("# Step 5d:")
            || heading.starts_with("# Step 2b:")
            || heading.starts_with("# Step 2c:")
            || heading.starts_with("# Step 5:")
            || heading.starts_with("# Step 5d:")
        {
            format!(
                "Scoped task: {}. Repository: {}. No external services or input parsing.",
                build["task"],
                repo.display()
            )
        } else if heading.starts_with("# Step 6:") || heading.starts_with("# Step 6c:") {
            fs::write(
                repo.join("README.md"),
                "# Answer\n\nanswer() returns 42. answer.txt contains 42 followed by a newline.\nRun cargo test --workspace --locked and pre-commit run --all-files.\n",
            )?;
            "Documentation written: README.md describes behavior and real checks.".into()
        } else {
            let evidence = verify(case, &repo)?;
            if heading.contains("17b:") || heading.contains("17c:") || heading.contains("17d:") {
                post(case, &format!("{heading}: verified {evidence}"))?;
            }
            format!(
                "{heading}\nVerified answer 42 and current-source tests: {evidence}. No further edits required."
            )
        }
    } else {
        bail!("unsupported agent task heading: {heading}; task retained as task-{id}.txt");
    };
    fs::write(case.evidence.join(format!("response-{id}.txt")), &response)?;
    Ok(response)
}
fn supported_review(heading: &str) -> bool {
    [
        "# Step 2b:",
        "# Step 2c:",
        "# Step 5:",
        "# Step 5d:",
        "# Step 6:",
        "# Step 6c:",
        "# Step 9:",
        "# Step 9b:",
        "# Step 10:",
        "# Step 10b:",
        "# Step 10c:",
        "# Step 10d:",
        "# Step 11:",
        "# Step 11b:",
        "# Step 17b:",
        "# Step 17c:",
        "# Step 17d:",
        "# Step 17e:",
        "# Step 18a:",
        "# Step 18b:",
        "# Step 19a:",
        "# Step 19b:",
        "# Step 20:",
        "# Step 20c:",
        "# Step 21:",
    ]
    .iter()
    .any(|prefix| heading.starts_with(prefix))
}
