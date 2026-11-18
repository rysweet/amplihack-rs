//! Action adapter for the explicitly supported recipe tasks. No call-count dispatch.
use super::agent::{task_field, task_json};
use super::{
    agent, driver,
    environment::{IsolatedCase, checked, observe_git, private_dir, runtime_path},
    hosting::{Checks, HostingStore},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

fn task_path(case: &IsolatedCase, task: &str, prefix: &str, suffix: &str) -> Result<PathBuf> {
    let path = PathBuf::from(task_field(task, prefix, suffix)?.trim());
    let relative = path
        .strip_prefix(&case.root)
        .context("rendered path escapes case")?;
    super::environment::confined_path(&case.root, relative)
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
    let location = case.evidence.join("drive-location.json");
    if location.is_file() {
        let observed: super::evidence::DriveObservation =
            serde_json::from_slice(&fs::read(location)?)?;
        super::evidence::validate_drive_location(&observed)?;
        return runtime_path(&case.work, &observed.resolved_worktree);
    }
    let implementation = case.evidence.join("implementation-worktree.json");
    if driver::config(case)?["case"] == "build" && implementation.is_file() {
        let path: PathBuf = serde_json::from_slice(&fs::read(implementation)?)?;
        let relative = path
            .strip_prefix(&case.root)
            .context("implementation path escapes case")?;
        return super::environment::confined_path(&case.root, relative);
    }
    // The runner resolves working_dir before invoking the client. Use the
    // actual invocation directory, never a caller checkout fallback.
    let cwd = std::env::current_dir()?.canonicalize()?;
    ensure!(cwd.starts_with(&case.root), "agent cwd escapes case");
    Ok(cwd)
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
fn build_delivery(case: &IsolatedCase, repo: &Path) -> Result<super::hosting::PullRequest> {
    let mut hosting = host(case)?;
    let prs = hosting.open_prs()?;
    ensure!(prs.len() == 1, "delivery requires exactly one open PR");
    let pr = prs[0].clone();
    let readback = hosting.invoke(
        repo,
        &["pr", "view", "1", "--json", "state,isDraft,headRefOid"],
    )?;
    ensure!(readback.exit_code == 0, "delivery PR readback failed");
    let state: Value = serde_json::from_slice(&readback.stdout)?;
    let checks = hosting.invoke(
        repo,
        &["pr", "checks", "1", "--required", "--json", "name,state"],
    )?;
    let checks_json: Value = serde_json::from_slice(&checks.stdout)?;
    ensure!(
        checks.exit_code == 0
            && checks_json
                .as_array()
                .is_some_and(|checks| !checks.is_empty()
                    && checks.iter().all(|check| check["state"] == "SUCCESS")),
        "delivery checks are incomplete"
    );
    let observed = super::evidence::observe_build(
        repo,
        &driver::origin(case)?,
        &hosting,
        pr.number,
        "answer.txt",
    )?;
    ensure!(
        observed.local_committed
            && observed.remote_matches
            && observed.open_pr_matches
            && state["state"] == "OPEN"
            && state["isDraft"] == false
            && state["headRefOid"] == pr.head_sha
            && observe_git(repo, &["show", "HEAD:answer.txt"])? == "42",
        "delivery evidence is incomplete"
    );
    driver::save(
        &case.evidence.join(format!(
            "delivery-observation-{}.json",
            uuid::Uuid::new_v4()
        )),
        &json!({"git":observed,"pr":state,"checks":checks_json}),
    )?;
    Ok(pr)
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
    let plan: Value = serde_json::from_slice(&fs::read(case.root.join("responses/drive.json"))?)?;
    if found.is_none()
        && driver::config(case)?["case"] == "drive-control"
        && plan["control_prepare_worktree"]
            .as_bool()
            .context("drive control preparation is not declared")?
    {
        let target = cwd
            .parent()
            .context("private checkout has no parent")?
            .join("worktrees/pr-1");
        let target = runtime_path(&case.work, &target)?;
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
    let repo = runtime_path(&case.work, &repo)?;
    let current = observe_git(&repo, &["rev-parse", "HEAD"])?;
    ensure!(
        current == pr.head_sha,
        "preflight local and hosting head differ"
    );
    let provided = task_field(task, "Prefer the provided state dir ", ";")?.trim();
    let state = if provided.is_empty() {
        case.root.join("autodrive-state")
    } else {
        PathBuf::from(provided)
    };
    let state = runtime_path(&case.root, &state)?;
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
        let authorization = task_json(task, "Runner-captured invocation: ")?;
        let run_id = authorization["run_id"]
            .as_str()
            .context("runner authorization has no run identity")?;
        let policy = authorization["policy_provenance"]
            .as_str()
            .context("runner authorization has no captured policy provenance")?;
        result["schema_version"] = json!(1);
        result["binding"] = json!({"run_id":run_id,"round_id":"entry","platform":"github","host":"github.com","repository":"fixture/project","source_repository":"fixture/project","pr_number":1,"pr_url":"https://github.com/fixture/project/pull/1","head_branch":pr.branch,"base_branch":"main","head_sha":pr.head_sha,"base_sha":observe_git(&driver::origin(case)?,&["rev-parse","main"] )?,"merge_base_sha":observe_git(&repo,&["merge-base",&pr.head_sha,"origin/main"] )?,"policy_provenance":policy});
        result["entry_evidence"] = json!(format!(
            "Observed OPEN fixture/project PR 1 at {}; verified linked worktree and captured runner state",
            pr.head_sha
        ));
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
    } else if task.starts_with("# Step 14: Bump Version in Cargo.toml") {
        let manifest: toml::Value = toml::from_str(&fs::read_to_string(repo.join("Cargo.toml"))?)?;
        let current = manifest["package"]["version"]
            .as_str()
            .context("fixture package version absent")?;
        let diff = observe_git(&repo, &["diff", "main", "--stat"])?;
        ensure!(
            fs::read(repo.join("answer.txt"))? == b"42\n",
            "version assessment lacks implemented fix"
        );
        json!({"current_version":current,"new_version":current,"change_classification":"PATCH",
            "rationale":"correct existing answer() behavior; PATCH contract and operator instruction require no version change",
            "changes_summary":diff}).to_string()
    } else if task.starts_with("# Step 16b: Outside-In Testing and Fix Loop") {
        let api = verify(case, &repo)?;
        let bytes = check(case, &repo, "/usr/bin/cmp", &["answer.txt", "expected.txt"])?;
        let mut hosting = host(case)?;
        let pr = hosting.pr(1)?;
        ensure!(
            observe_git(&repo, &["rev-parse", "HEAD"])? == pr.head_sha,
            "published head differs from outside-in test target"
        );
        let body = format!(
            "{}\n\nStep 16b: Outside-In Testing Results\nRust consumer assertion: {api}\nByte-exact answer file: {bytes}\nBoth passed; zero fixes.\n",
            pr.body
        );
        ensure!(
            hosting
                .invoke(&repo, &["pr", "edit", "1", "--body", &body])?
                .exit_code
                == 0,
            "outside-in evidence publication failed"
        );
        hosting.set_checks(1, Checks::Green)?;
        driver::save(
            &case.evidence.join("outside-in-observation.json"),
            &json!({"head":pr.head_sha,"api":api,"bytes":bytes,"fixes":0}),
        )?;
        driver::save(
            &case.evidence.join("green-transition.json"),
            &json!({"head":pr.head_sha,
            "cause":"explicit build check completion after published-head API and byte verification"}),
        )?;
        format!(
            "Completed public answer() assertion and byte-exact answer file checks: {api}; {bytes}. PR description updated. Zero fixes."
        )
    } else if task.starts_with("Evaluate goal achievement after round 1.") {
        let pr = build_delivery(case, &repo)?;
        json!({"goal_status":"ACHIEVED","summary":format!("origin and open PR #{} at {}",pr.number,pr.head_sha)}).to_string()
    } else if task.starts_with("Final assessment after all execution rounds.") {
        let pr = build_delivery(case, &repo)?;
        format!(
            "Verified committed answer 42 at {}, matching origin and open non-draft PR #{} with a nonempty green required-check set.\nGOAL_STATUS: ACHIEVED",
            pr.head_sha, pr.number
        )
    } else if task.starts_with("Outside-in testing validation check.") {
        let pr = build_delivery(case, &repo)?;
        let measured: Value = serde_json::from_slice(&fs::read(
            case.evidence.join("outside-in-observation.json"),
        )?)?;
        ensure!(
            measured["head"] == pr.head_sha,
            "outside-in evidence targets another head"
        );
        for scenario in ["api", "bytes"] {
            let receipt = PathBuf::from(
                measured[scenario]["exit_receipt"]
                    .as_str()
                    .context("outside-in exit receipt absent")?,
            );
            let relative = receipt
                .strip_prefix(&case.root)
                .context("outside-in receipt escapes case")?;
            ensure!(
                fs::read(super::environment::confined_path(&case.root, relative)?)? == b"0\n",
                "outside-in scenario {scenario} did not pass"
            );
        }
        format!(
            "OUTSIDE-IN TESTING VALIDATION REPORT\nExecution mode: single\nPR(s) found: https://github.com/fixture/project/pull/{}\nWorkstream: answer\nqa-team strategy: Rust consumer API and byte-exact file checks\nStep 13 results documented: YES (PR Validation records both scenarios)\nChosen validation strategy documented: YES\nTest scenarios executed: 2\nVERDICT: PASS\nOUTSIDE_IN_VALIDATION: PASSED",
            pr.number
        )
    } else if task.starts_with("Produce a concise execution summary.") {
        let pr = build_delivery(case, &repo)?;
        format!(
            "Corrected answer() to return 42 and published the verified implementation at {}. PR remains open: https://github.com/fixture/project/pull/{}. Outside-in testing: PASSED. Goal status: ACHIEVED. No remaining implementation work; no adaptive strategy was needed.",
            pr.head_sha, pr.number
        )
    } else if task.starts_with("# Agentic Finalization Narrative") {
        let prs = host(case)?.open_prs()?;
        format!(
            "Observed open PRs: {}. Completed check artifacts are retained separately.",
            serde_json::to_string(&prs)?
        )
    } else if task.starts_with("# Terminal-State Adjudication") {
        let pr = build_delivery(case, &repo)?;
        format!(
            "Observed committed work, matching origin and open PR #{} at {}.\nTERMINAL_VERDICT: SUCCESS",
            pr.number, pr.head_sha
        )
    } else if task.starts_with("Determine QA prerequisites using the trusted qa-team") {
        for tool in ["cargo", "rustc", "gh", "codex", "claude"] {
            ensure!(
                case.root.join("bin").join(tool).is_file(),
                "missing staged tool {tool}"
            );
        }
        json!({"prerequisites":["cargo","rustc","gh"],"blockers":[],"attempts":[]}).to_string()
    } else if (task.starts_with("Use noninteractive command execution")
        || task.starts_with("Establish the PR identity and the worktree before continuing."))
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
        ensure!(
            observe_git(&repo, &["rev-parse", "HEAD"])? == pr.head_sha,
            "round context local worktree differs from hosted head"
        );
        json!({"pr":"1","head_sha":pr.head_sha,"resolved_concerns":"","round_label":task_json(task, "Return JSON ")?["round_label"],"blockers":[],"attempts":[]}).to_string()
    } else if task.starts_with("# Crusty review") {
        let reviewed = task_field(task, "Read the exact commit `", "`")?.to_string();
        let review_dir = task_path(case, task, "git -C \"", "\"")?;
        ensure!(
            observe_git(&review_dir, &["show", &format!("{reviewed}:answer.txt")])? == "42",
            "reviewed commit does not implement answer"
        );
        let diff = observe_git(&review_dir, &["show", &reviewed])?;
        fs::write(case.evidence.join(format!("review-{reviewed}.diff")), diff)?;
        let receipt = agent::Receipt::clean_review(reviewed.clone(), reviewed.clone());
        agent::validate_receipt(&review_dir, &receipt)?;
        json!({"crusty_verdict":"CLEAN","concerns":[],"summary":format!("reviewed {reviewed}: answer function and content satisfy scoped task")}).to_string()
    } else if task
        .starts_with("Use noninteractive command execution with GH_PAGER=cat. Post this Crusty")
    {
        let pr = host(case)?.pr(1)?;
        let line = format!("Crusty verdict: CLEAN at {}", pr.head_sha);
        let mut hosting = host(case)?;
        let comments = hosting.invoke(
            &repo,
            &[
                "api",
                "repos/{owner}/{repo}/issues/1/comments",
                "--paginate",
            ],
        )?;
        ensure!(
            comments.exit_code == 0,
            "cannot read prior verdict comments"
        );
        let comments: Vec<Value> = serde_json::from_slice(&comments.stdout)?;
        if !comments.iter().any(|comment| {
            comment["user"]["login"] == "fixture"
                && comment["body"]
                    .as_str()
                    .is_some_and(|body| body.lines().next() == Some(line.as_str()))
        }) {
            let body = case
                .evidence
                .join(format!("verdict-{}.txt", uuid::Uuid::new_v4()));
            fs::write(
                &body,
                format!(
                    "{line}\n\nObserved answer function and content in the exact reviewed commit.\n"
                ),
            )?;
            let result = hosting.invoke(
                &repo,
                &[
                    "pr",
                    "comment",
                    "1",
                    "--body-file",
                    body.to_str().context("verdict body path")?,
                ],
            )?;
            fs::remove_file(body)?;
            ensure!(result.exit_code == 0, "posting verdict failed");
        }
        json!({"verdict_posted":"true","head_sha":pr.head_sha}).to_string()
    } else if task.starts_with("Assess whether this Crusty review objective is complete") {
        let record = task_path(case, task, "Read the round record at\n", " and its")?;
        let result = fs::read(&record).and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes).map_err(std::io::Error::other)
        });
        let complete = result.as_ref().is_ok_and(|v| {
            matches!(v["crusty_verdict"].as_str(), Some("CLEAN" | "MINOR_ONLY"))
                && v["reviewed_head_sha"] == v["head_sha"]
                && v["commits_this_round"] == 0
        });
        let state = task_path(case, task, "--private-state \"", "\"")?;
        checked(
            case.command(&case.root.join("bin/amplihack"))?
                .args([
                    "orch",
                    "helper",
                    "autodrive-completion-write",
                    "--private-state",
                    state.to_str().context("private state path")?,
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
    } else if task.starts_with("Verify the loop result ") {
        let result = task_json(task, "Verify the loop result ")?;
        ensure!(
            result["loop_result"] == "DONE" && result["round_verdict"] == "CLEAN",
            "phase recording requires a completed clean loop"
        );
        let state = task_path(
            case,
            task,
            "existing state ",
            ". Preserve stable identifiers",
        )?;
        let tools = case.home.join(".amplihack/amplifier-bundle/tools");
        for operation in ["collect", "check"] {
            let mut command = case.command(Path::new("/bin/bash"))?;
            command
                .arg("-p")
                .arg(tools.join("autodrive_followups.sh"))
                .args([operation, "--state-dir"])
                .arg(&state);
            if operation == "check" {
                command.args(["--pr", "1"]);
            }
            checked(command.output()?, "clean-review followups")?;
        }
        checked(
            case.command(Path::new("/bin/bash"))?
                .args([
                    "-p",
                    "-c",
                    ". \"$1\" && autodrive_record_crusty_loop_done \"$2\"",
                    "phase-recorder",
                ])
                .arg(tools.join("autodrive_state.sh"))
                .arg(&state)
                .output()?,
            "record crusty phase",
        )?;
        json!({"recorded":true,"reviewed_head_sha":host(case)?.pr(1)?.head_sha,"followups":[],"blockers":[],"attempts":[]}).to_string()
    } else if task.starts_with("Reconcile every minor crusty finding") {
        let state = task_path(
            case,
            task,
            "preserve mappings in state ",
            ". Suggested tools",
        )?;
        let tools = case.home.join(".amplihack/amplifier-bundle/tools");
        for operation in ["collect", "check"] {
            let mut command = case.command(Path::new("/bin/bash"))?;
            command
                .arg("-p")
                .arg(tools.join("autodrive_followups.sh"))
                .args([operation, "--state-dir"])
                .arg(&state);
            if operation == "check" {
                command.args(["--pr", "1"]);
            }
            checked(command.output()?, "final followups")?;
        }
        json!({"mappings":[],"blockers":[],"attempts":[]}).to_string()
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
        let path = task_path(
            case,
            task,
            "Save the actual measured qa-evidence.json to ",
            " with private permissions",
        )?;
        let mut command = case.command(Path::new("/usr/bin/env"))?;
        command
            .arg(format!("REPO_PATH={}", repo.display()))
            .arg("AUTODRIVE_QA_BASE=main")
            .arg(format!("AUTODRIVE_QA_EVIDENCE={}", path.display()))
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
        let helper = case
            .home
            .join(".amplihack/amplifier-bundle/tools/autodrive_expected_checks.sh");
        let measured = case
            .command(Path::new("/bin/bash"))?
            .arg("-p")
            .arg(helper)
            .args([&pr.head_sha, "main"])
            .current_dir(&repo)
            .output()?;
        fs::write(
            case.evidence.join("expected-checks.stdout"),
            &measured.stdout,
        )?;
        fs::write(
            case.evidence.join("expected-checks.stderr"),
            &measured.stderr,
        )?;
        ensure!(
            measured.status.success(),
            "expected checks did not measure GREEN"
        );
        let line = String::from_utf8(measured.stdout)?;
        let result: Value = serde_json::from_str(
            line.lines()
                .last()
                .context("expected checks emitted no outcome")?,
        )?;
        ensure!(
            result["ci_status"] == "GREEN",
            "expected checks returned an incomplete outcome"
        );
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
        let helper = case
            .home
            .join(".amplihack/amplifier-bundle/tools/autodrive_platform_facts.sh");
        let output = case
            .command(Path::new("/bin/bash"))?
            .arg("-p")
            .arg(helper)
            .arg("1")
            .current_dir(&repo)
            .output()?;
        fs::write(case.evidence.join("platform-facts.stdout"), &output.stdout)?;
        fs::write(case.evidence.join("platform-facts.stderr"), &output.stderr)?;
        let mut measured: Value = serde_json::from_str(&checked(output, "platform measurements")?)?;
        ensure!(
            measured["head_sha"] == pr.head_sha
                && measured["approval_status"] == "MET"
                && measured["unresolved_threads"] == "0"
                && measured["merge_state"] == "CLEAN",
            "platform facts do not establish readiness"
        );
        measured["platform"] = json!("github");
        measured["host"] = json!("github.com");
        measured["repository"] = json!("fixture/project");
        measured["source_repository"] = json!("fixture/project");
        measured["pr_url"] = json!("https://github.com/fixture/project/pull/1");
        measured["pr_state"] = json!(pr.state);
        measured["head_branch"] = json!(pr.branch);
        measured["base_branch"] = json!(pr.base);
        measured["base_sha"] = json!(observe_git(
            &driver::origin(case)?,
            &["rev-parse", &pr.base]
        )?);
        measured["merge_base_sha"] = json!(observe_git(
            &repo,
            &["merge-base", &pr.head_sha, "origin/main"]
        )?);
        let mut hosting = host(case)?;
        for (field, args) in [
            (
                "pull_request",
                vec![
                    "pr",
                    "view",
                    "1",
                    "--json",
                    "title,body,statusCheckRollup,reviews",
                ],
            ),
            (
                "discussion",
                vec![
                    "api",
                    "repos/{owner}/{repo}/issues/1/comments",
                    "--paginate",
                ],
            ),
        ] {
            let response = hosting.invoke(&repo, &args)?;
            ensure!(
                response.exit_code == 0,
                "platform observation {field} failed"
            );
            measured[field] = serde_json::from_slice(&response.stdout)?;
        }
        measured["blockers"] = json!([]);
        measured["attempts"] = json!([]);
        measured.to_string()
    } else if task.starts_with("# merge-ready assessment") {
        let pr = host(case)?.pr(1)?;
        let evidence = task_json(task, "- qa-team scenarios: `")?;
        ensure!(
            pr.checks == Checks::Green
                && evidence["qa_status"] == "PASS"
                && evidence["head_sha"] == pr.head_sha,
            "readiness lacks passing measured evidence"
        );
        let reviewed = task_field(task, "Read the exact commit `", "`")?.to_string();
        ensure!(
            reviewed == pr.head_sha,
            "readiness head differs from reviewed commit"
        );
        let review = task_path(case, task, "git -C \"", "\"")?;
        ensure!(
            observe_git(&review, &["show", &format!("{reviewed}:README.md")])?.contains("42"),
            "readiness lacks user documentation at the assessed commit"
        );
        let base_sha = observe_git(&driver::origin(case)?, &["rev-parse", &pr.base])?;
        let changed = observe_git(&review, &["diff", "--name-only", &base_sha, &reviewed])?;
        ensure!(
            changed
                .lines()
                .all(|path| ["src/lib.rs", "answer.txt", "README.md"].contains(&path)),
            "readiness found unrelated changes"
        );
        let crusty = task_json(task, "- Crusty loop: `")?;
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
        driver::save(
            &case.evidence.join("ready-observation.json"),
            &json!({"head":pr.head_sha,"qa_status":evidence["qa_status"],"ci_status":"GREEN","crusty":crusty}),
        )?;
        body
    } else if task.starts_with("# Loop-health evaluation") {
        let own = task_field(task, "round's own verdict `", "`")?;
        let clean = task_field(task, "` equals `", "`")?;
        let measured = task_json(task, "```json\n")?;
        let pr = host(case)?.pr(1)?;
        let ready: Value = serde_json::from_slice(
            &fs::read(case.evidence.join("ready-observation.json"))
                .context("readiness did not complete before loop evaluation")?,
        )?;
        ensure!(
            own == "MERGE_READY"
                && clean == own
                && pr.checks == Checks::Green
                && ready["head"] == pr.head_sha
                && ready["qa_status"] == "PASS",
            "loop completion lacks clean current-round readiness"
        );
        driver::save(
            &case.evidence.join("loop-evaluation.json"),
            &json!({"measured":measured,"head":pr.head_sha,"verdict":own}),
        )?;
        json!({"loop_verdict":"DONE","not_converging":[],"reason":"current round is MERGE_READY after measured QA, expected checks and authorized review at observed head"}).to_string()
    } else if task.starts_with("State in two or three plain sentences how finalisation") {
        let result = task_json(task, "written by a tool, is: ")?;
        let pr = host(case)?.pr(1)?;
        ensure!(
            result["result"] == "merged" && pr.state == "MERGED",
            "finalization did not merge"
        );
        format!(
            "PR #1 is merged at {}. The production finalizer returned merged and hosting readback agrees.",
            pr.head_sha
        )
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
