//! Assignment 2223: executable contracts for language-aware verification,
//! capture integrity, durable resumption and independent classification gates.
//! Exercises the production evidence CLI through captured native executions.

#[path = "default_workflow_terminal_state/language_verification_fixtures.rs"]
mod fixtures;
use fixtures::*;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

#[test]
fn precommit_checkpoints_original_implementation_before_long_checks() {
    let text = fs::read_to_string(
        workspace_root().join("amplifier-bundle/recipes/workflow-precommit-test.yaml"),
    )
    .unwrap();
    let recipe: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    let steps = recipe["steps"].as_sequence().unwrap();
    let checkpoint = steps
        .iter()
        .position(|s| {
            s["command"]
                .as_str()
                .is_some_and(|c| c.contains("checkpoint-implementation"))
        })
        .expect("persist original implementation records before long Step 12 checks");
    let checks = steps
        .iter()
        .position(|s| s["id"].as_str() == Some("step-12-run-precommit"))
        .unwrap();
    assert!(checkpoint < checks);
}

#[test]
fn verification_recipe_uses_deterministic_capture_validation() {
    let step = recipe_step("workflow-precommit-test", "verification-terminal-evidence");
    let command = step["command"].as_str().unwrap();
    assert!(command.contains("workflow_terminal_evidence.sh"));
    assert!(command.contains("verify"));
    assert!(
        !command.contains(".workspace_command =="),
        "a Cargo-shaped report cannot replace captured project checks"
    );
    assert_eq!(
        step["output"].as_str(),
        Some("verification_terminal_evidence")
    );
    assert_eq!(step["parse_json"].as_bool(), Some(true));
}

#[test]
fn legacy_placeholder_positive_does_not_establish_execution() {
    let mut f = Fixture::javascript();
    // This was accepted by the old Cargo-only jq predicate. No artifacts exist.
    f.context["precommit_results"] = json!({"status":"PASS","exit_code":0,
        "precommit_exit_code":0,"workspace_exit_code":0,
        "workspace_command":"cargo test --workspace --locked","validation_scope":"FULL_WORKSPACE",
        "test_threads":"DEFAULT","evidence":["controlled-precommit-fixture"]});
    f.context["local_testing_gate"] = json!({"status":"PASS","exit_code":0,
        "scenarios":[{"status":"PASS","exit_code":0},{"status":"PASS","exit_code":0}],
        "evidence":["controlled-scenario-fixture"]});
    verification(
        &f.step("workflow-precommit-test", "verification-terminal-evidence"),
        false,
    );
}

#[test]
fn local_dispatch_precedes_github_plumbing() {
    let mut f = Fixture::javascript();
    f.env.insert(
        "WORKFLOW_GITHUB_REPO_HELPER".into(),
        f.tmp
            .path()
            .join("absent-github-parser")
            .display()
            .to_string(),
    );
    let p = f.register();
    f.execute_all(&p);
    verification(&f.verify(&p), true);
}

#[test]
fn native_javascript_checks_keep_actual_runner_and_command_identities() {
    let mut f = Fixture::javascript();
    let p = f.register();
    let captures = f.execute_all(&p);
    verification(&f.verify(&p), true);
    for c in captures {
        let e = f.execution(&c);
        assert_eq!(e["command"]["toolchain"], "javascript");
        assert!(!e["command"]["display"].as_str().unwrap().contains("cargo"));
        assert_eq!(e["repository"]["root"], json!(f.repo));
        assert_eq!(e["source_before"]["head"], f.head());
        assert_eq!(e["source_after"], e["source_before"]);
        assert_eq!(e["process"]["termination"], "exited");
        assert_eq!(e["process"]["exit_code"], 0);
        assert!(matches!(
            f.artifact(&e["artifacts"]["exit_receipt"]).as_slice(),
            b"0" | b"0\n"
        ));
        f.artifact(&e["artifacts"]["raw_log"]);
        f.artifact(&e["capture_binding"]["metadata"]);
    }
}

macro_rules! native_positive {
    ($name:ident,$language:literal) => {
        #[test]
        #[ignore = "native proof: run sequentially under the worker's measured memory admission"]
        fn $name() {
            let mut f = Fixture::native_project($language);
            let p = f.register();
            let captures = f.execute_all(&p);
            verification(&f.verify(&p), true);
            assert!(
                captures
                    .iter()
                    .all(|c| f.execution(c)["command"]["toolchain"] == $language)
            );
        }
    };
}
native_positive!(native_rust_full_workspace_default_threads, "rust");
native_positive!(
    native_python_preserves_repository_uv_unittest_command,
    "python"
);
native_positive!(native_go_executes_module_and_consumer_commands, "go");
native_positive!(
    native_dotnet_executes_documented_assertion_harness,
    "dotnet"
);

#[test]
#[ignore = "native TypeScript proof requires offline npm cache with Vitest and TypeScript; no global installation"]
fn native_typescript_vitest_typecheck_and_two_real_consumers() {
    let mut f = Fixture::javascript();
    for name in ["contract.js", "contract.test.js", "consumer.js"] {
        fs::remove_file(f.repo.join(name)).unwrap();
    }
    f.file("package.json",r#"{"name":"evidence-typescript","private":true,"type":"module","scripts":{"test":"vitest run contract.test.ts","typecheck":"tsc --noEmit"},"devDependencies":{"vitest":"5.0.3","typescript":"5.9.3"}}"#);
    f.file("tsconfig.json",r#"{"compilerOptions":{"target":"ES2022","module":"ESNext","moduleResolution":"bundler","strict":true,"skipLibCheck":true},"include":["contract.ts"]}"#);
    f.file("contract.ts","export function decode(v: number[]): number[] { if (!Array.isArray(v)) throw new TypeError('array required'); return v.map(x=>x*2); }\n");
    f.file("contract.test.ts","import {expect,test} from 'vitest'; import {decode} from './contract'; test('contract',()=>expect(decode([2])).toEqual([4]));\n");
    f.file("consumer.test.ts","import {expect,test} from 'vitest'; import {decode} from './contract'; test('basic consumer',()=>expect(decode([2])).toEqual([4])); test('edge consumer',()=>{ expect(decode([])).toEqual([]); expect(()=>decode(null as never)).toThrow('array required'); });\n");
    f.file("README.md","Maintained TypeScript package. Test: npm test. Typecheck: npm run typecheck. Basic consumer: ./node_modules/.bin/vitest run consumer.test.ts -t basic. Edge consumer: ./node_modules/.bin/vitest run consumer.test.ts -t edge. No pre-commit configuration or extra quality command prescribed.\n");
    let cache =
        std::env::var("WORKFLOW_TEST_NPM_CACHE").expect("explicit offline cache for native proof");
    f.env.insert("npm_config_cache".into(), cache);
    f.native(
        "npm",
        &[
            "install",
            "--offline",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
        ],
    );
    f.commit("TypeScript consumer project");
    f.plan = f.identity();
    f.plan["components"] = json!([{"id":"root","root":".","toolchain":"typescript","disposition":"required",
        "inspected":[f.reference("package.json"),f.reference("README.md"),f.reference("tsconfig.json")],
        "justification":"maintained TypeScript root package"}]);
    f.plan["exclusions"] = json!([{"id":"no-hooks","component_ids":["root"],"subject":"precommit",
        "reason":"no prescribed hooks","policy_refs":[f.reference("README.md")]}]);
    f.plan["obligations"] = json!([
        f.obligation("tests", "test", "typescript", "vitest", &["npm", "test"]),
        f.obligation(
            "typecheck",
            "typecheck",
            "typescript",
            "tsc",
            &["npm", "run", "typecheck"]
        ),
        f.obligation(
            "basic",
            "consumer",
            "typescript",
            "vitest",
            &[
                "./node_modules/.bin/vitest",
                "run",
                "consumer.test.ts",
                "-t",
                "basic"
            ]
        ),
        f.obligation(
            "edge",
            "consumer",
            "typescript",
            "vitest",
            &[
                "./node_modules/.bin/vitest",
                "run",
                "consumer.test.ts",
                "-t",
                "edge"
            ]
        )
    ]);
    let p = f.register();
    let captures = f.execute_all(&p);
    verification(&f.verify(&p), true);
    let records = captures.iter().map(|c| f.execution(c)).collect::<Vec<_>>();
    assert_eq!(records[0]["command"]["runner"], "vitest");
    assert_eq!(records[1]["command"]["runner"], "tsc");
    assert_eq!(records[2]["command"]["runner"], "vitest");
    assert_ne!(records[2]["execution_id"], records[3]["execution_id"]);
}

#[test]
fn inspectable_hook_inapplicability_has_no_fabricated_execution() {
    let mut f = Fixture::javascript();
    let p = f.register();
    f.execute_all(&p);
    verification(&f.verify(&p), true);
    assert!(
        !f.context["precommit_results"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["obligation_id"] == "no-hooks")
    );
    let plan: Value = serde_json::from_slice(&f.artifact(&p["plan"])).unwrap();
    assert_eq!(plan["exclusions"][0]["subject"], "precommit");
}

#[test]
fn configured_quality_check_cannot_be_omitted() {
    let mut f = Fixture::javascript();
    f.plan["obligations"].as_array_mut().unwrap().remove(1);
    let r = f.call("plan", &f.plan);
    r.exit(1);
    assert_eq!(r.json()["status"], "UNPROVEN");
}

#[test]
fn configured_hooks_cannot_be_excluded_as_missing() {
    let mut f = Fixture::javascript();
    f.file(".pre-commit-config.yaml","repos:\n  - repo: local\n    hooks:\n      - id: syntax\n        name: JavaScript syntax\n        entry: node --check contract.js\n        language: system\n        pass_filenames: false\n");
    f.commit("configured quality hook");
    f.plan = f.node_plan("javascript");
    let r = f.call("plan", &f.plan);
    r.exit(1);
    assert_eq!(r.json()["status"], "UNPROVEN");
}

#[test]
fn typescript_without_configured_typecheck_is_unproven() {
    let mut f = Fixture::javascript();
    let mut package: Value =
        serde_json::from_slice(&fs::read(f.repo.join("package.json")).unwrap()).unwrap();
    package["scripts"]["typecheck"] = json!("tsc --noEmit");
    f.file("package.json", serde_json::to_vec(&package).unwrap());
    f.file("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    f.commit("configured TypeScript check");
    f.plan = f.node_plan("typescript");
    let r = f.call("plan", &f.plan);
    r.exit(1);
    assert_eq!(r.json()["status"], "UNPROVEN");
}

#[test]
fn mixed_repository_cannot_hide_rust_by_selecting_node() {
    let mut f = mixed_rust_policy_fixture(&["cargo", "test", "--workspace", "--locked"]);
    f.plan = f.node_plan("javascript");
    let r = f.call("plan", &f.plan);
    r.exit(1);
    assert_eq!(r.json()["status"], "UNPROVEN");
}

#[test]
fn unknown_component_keeps_plan_unproven() {
    let mut f = Fixture::javascript();
    f.plan["components"][0]["toolchain"] = json!("unknown");
    f.plan["components"][0]["disposition"] = json!("unresolved");
    let r = f.call("plan", &f.plan);
    r.exit(1);
    assert_eq!(r.json()["status"], "UNPROVEN");
}

#[test]
fn plan_registration_cannot_rebind_existing_content_id() {
    let mut f = Fixture::javascript();
    let first = f.register();
    f.plan["obligations"][0]["argv"] = json!(["node", "--test", "contract.test.js"]);
    let second = f.register();
    assert_ne!(first["plan_id"], second["plan_id"]);
    let retained: Value = serde_json::from_slice(&f.artifact(&first["plan"])).unwrap();
    assert_eq!(retained["obligations"][0]["argv"], json!(["npm", "test"]));
}

macro_rules! invalid_request {
    ($name:ident,$payload:expr,$code:literal) => {
        #[test]
        fn $name() {
            let f = Fixture::javascript();
            f.run_raw(&["plan", "--request-file", "-"], $payload)
                .error($code);
        }
    };
}
invalid_request!(
    duplicate_json_keys_rejected,
    br#"{"schema_version":1,"schema_version":1}"#,
    "INVALID_REQUEST"
);
invalid_request!(
    unknown_schema_rejected,
    br#"{"schema_version":99}"#,
    "UNSUPPORTED_SCHEMA"
);
invalid_request!(
    wrong_version_type_rejected,
    br#"{"schema_version":"1"}"#,
    "INVALID_REQUEST"
);
invalid_request!(trailing_document_rejected, b"{}\n{}", "INVALID_REQUEST");
invalid_request!(nul_rejected, b"{\"schema_version\":1}\0", "INVALID_REQUEST");
invalid_request!(malformed_json_rejected, b"{", "INVALID_REQUEST");
invalid_request!(
    nonfinite_number_rejected,
    b"{\"schema_version\":NaN}",
    "INVALID_REQUEST"
);

#[test]
fn unknown_authoritative_request_fields_fail_closed() {
    let f = Fixture::javascript();
    let mut request = f.plan.clone();
    request["trust_submitted_receipts"] = json!(true);
    f.call("plan", &request).error("INVALID_REQUEST");
}

#[test]
fn duplicate_obligation_ids_fail_closed() {
    let mut f = Fixture::javascript();
    f.plan["obligations"][1]["id"] = f.plan["obligations"][0]["id"].clone();
    f.call("plan", &f.plan).error("INVALID_REQUEST");
}

#[test]
fn request_file_and_stdin_have_identical_semantics() {
    let f = Fixture::javascript();
    let payload = serde_json::to_vec(&f.plan).unwrap();
    let path = f.tmp.path().join("request.json");
    write(&path, &payload);
    let stdin = f.call("plan", &f.plan);
    stdin.exit(0);
    let file = f.run_raw(&["plan", "--request-file", path.to_str().unwrap()], b"");
    file.exit(0);
    assert_eq!(stdin.json()["plan_id"], file.json()["plan_id"]);
}

#[test]
fn conflicting_selected_context_cannot_change_authority() {
    let f = Fixture::javascript();
    let mut r = f.request();
    r["context_file"] = json!(f.tmp.path().join("other.json"));
    f.call("restore", &r).error("INVALID_CONTEXT");
}

#[test]
fn capture_compares_argv_with_registered_command() {
    let f = Fixture::javascript();
    let p = f.register();
    let mut r = f.capture_request(&p, "tests");
    r["argv"] = json!(["node", "--version"]);
    f.call("capture", &r).error("COMMAND_MISMATCH");
}

#[test]
fn capture_compares_cwd_with_registered_execution_directory() {
    let f = Fixture::javascript();
    let p = f.register();
    let mut r = f.capture_request(&p, "tests");
    fs::create_dir(f.repo.join("elsewhere")).unwrap();
    r["cwd"] = json!(f.repo.join("elsewhere"));
    f.call("capture", &r).error("CWD_MISMATCH");
}

#[test]
fn capture_does_not_accept_another_repository() {
    let f = Fixture::javascript();
    let other = Fixture::javascript();
    let p = f.register();
    let mut r = f.capture_request(&p, "tests");
    r["repository_root"] = json!(other.repo);
    f.call("capture", &r).error("IDENTITY_MISMATCH");
}

#[test]
fn producer_field_cannot_authorise_a_different_step() {
    let f = Fixture::javascript();
    let p = f.register();
    let mut r = f.capture_request(&p, "basic");
    r["producer_step"] = json!("step-12-run-precommit");
    f.call("capture", &r).error("IDENTITY_MISMATCH");
}

#[test]
fn failed_process_keeps_real_numeric_exit_and_cannot_verify() {
    let mut f = Fixture::javascript();
    f.file(
        "failure.js",
        "console.error('intentional check failure'); process.exit(7);\n",
    );
    f.commit("failed check fixture");
    f.plan = f.node_plan("javascript");
    f.plan["obligations"][0]["argv"] = json!(["node", "failure.js"]);
    let p = f.register();
    let r = f.call("capture", &f.capture_request(&p, "tests"));
    r.exit(1);
    assert_eq!(r.json()["process_exit_code"], 7);
    let e = f.execution(&r.json());
    assert_eq!(e["process"]["exit_code"], 7);
    assert_eq!(f.artifact(&e["artifacts"]["exit_receipt"]), b"7\n");
    assert!(
        String::from_utf8(f.artifact(&e["artifacts"]["raw_log"]))
            .unwrap()
            .contains("intentional check failure")
    );
    verification(&f.verify(&p), false);
}

#[test]
fn successful_silent_process_has_a_real_empty_log() {
    let mut f = Fixture::javascript();
    f.file("silent.js", "");
    f.commit("silent check");
    f.plan = f.node_plan("javascript");
    f.plan["obligations"][0]["argv"] = json!(["node", "silent.js"]);
    let p = f.register();
    let c = f.capture(&p, "tests");
    let e = f.execution(&c);
    assert!(f.artifact(&e["artifacts"]["raw_log"]).is_empty());
    // Silence alone does not cover the remaining obligations.
    verification(&f.verify(&p), false);
}

#[test]
fn signaled_process_cannot_be_confused_with_normal_zero() {
    let mut f = Fixture::javascript();
    f.file("signal.js", "process.kill(process.pid,'SIGTERM');\n");
    f.commit("signal fixture");
    f.plan = f.node_plan("javascript");
    f.plan["obligations"][0]["argv"] = json!(["node", "signal.js"]);
    let p = f.register();
    let r = f.call("capture", &f.capture_request(&p, "tests"));
    r.exit(1);
    let e = f.execution(&r.json());
    assert_eq!(e["process"]["termination"], "signaled");
    assert!(e["process"]["exit_code"].is_null());
    verification(&f.verify(&p), false);
}

#[test]
fn source_mutated_by_check_is_not_evidence_for_post_check_source() {
    let mut f = Fixture::javascript();
    f.file(
        "mutate.js",
        "import fs from 'node:fs'; fs.appendFileSync('contract.js','// mutation\\n');\n",
    );
    f.commit("mutating check");
    f.plan = f.node_plan("javascript");
    f.plan["obligations"][0]["argv"] = json!(["node", "mutate.js"]);
    let p = f.register();
    let r = f.call("capture", &f.capture_request(&p, "tests"));
    assert_eq!(r.json()["process_exit_code"], 0);
    let record = f.execution(&r.json());
    assert_ne!(
        record["source_before"]["fingerprint"],
        record["source_after"]["fingerprint"]
    );
    verification(&f.verify(&p), false);
}

macro_rules! stale_source {
    ($name:ident,$mutate:expr) => {
        #[test]
        fn $name() {
            let mut f = Fixture::javascript();
            let p = f.register();
            f.execute_all(&p);
            verification(&f.verify(&p), true);
            let original = f.head();
            ($mutate)(&mut f);
            verification(&f.verify(&p), false);
            assert!(!original.is_empty());
        }
    };
}
stale_source!(
    tracked_edit_invalidates_unchanged_head,
    |f: &mut Fixture| {
        let head = f.head();
        f.file("contract.js", "export const changed=true;\n");
        assert_eq!(f.head(), head);
    }
);
stale_source!(
    untracked_source_invalidates_unchanged_head,
    |f: &mut Fixture| f.file("new-source.js", "export const newSource=true;\n")
);
stale_source!(deleted_source_invalidates_capture, |f: &mut Fixture| {
    fs::remove_file(f.repo.join("contract.js")).unwrap()
});
stale_source!(
    new_commit_never_relabels_old_tested_head,
    |f: &mut Fixture| {
        f.file("contract.js", "export const changed=true;\n");
        f.commit("later source");
    }
);
stale_source!(
    content_preserving_commit_still_invalidates_head,
    |f: &mut Fixture| {
        f.native(
            "git",
            &["commit", "--allow-empty", "-m", "new source epoch"],
        );
    }
);
stale_source!(
    relevant_configuration_edit_invalidates_capture,
    |f: &mut Fixture| f.file(
        "package.json",
        r#"{"name":"changed","scripts":{"test":"exit 0"}}"#
    )
);
stale_source!(
    repository_identity_change_invalidates_capture,
    |f: &mut Fixture| {
        f.native(
            "git",
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/other/project.git",
            ],
        );
    }
);

fn captured_js() -> (Fixture, Value, Vec<Value>) {
    let mut f = Fixture::javascript();
    let p = f.register();
    let captures = f.execute_all(&p);
    verification(&f.verify(&p), true);
    (f, p, captures)
}

macro_rules! artifact_mutation {
    ($name:ident,$field:literal,$mutate:expr) => {
        #[test]
        fn $name() {
            let (f, p, c) = captured_js();
            let e = f.execution(&c[0]);
            let path = f.artifact_path(&e["artifacts"][$field]);
            ($mutate)(&f, &path);
            verification(&f.verify(&p), false);
        }
    };
}
artifact_mutation!(
    missing_raw_log_denied,
    "raw_log",
    |_: &Fixture, p: &Path| fs::remove_file(p).unwrap()
);
artifact_mutation!(
    tampered_raw_log_denied,
    "raw_log",
    |_: &Fixture, p: &Path| write(p, b"fabricated PASS\n")
);
artifact_mutation!(
    missing_numeric_exit_denied,
    "exit_receipt",
    |_: &Fixture, p: &Path| fs::remove_file(p).unwrap()
);
artifact_mutation!(
    nonzero_numeric_exit_denied,
    "exit_receipt",
    |_: &Fixture, p: &Path| write(p, b"7\n")
);
artifact_mutation!(
    empty_exit_denied,
    "exit_receipt",
    |_: &Fixture, p: &Path| write(p, b"")
);
artifact_mutation!(
    nonnumeric_exit_denied,
    "exit_receipt",
    |_: &Fixture, p: &Path| write(p, b"PASS\n")
);
artifact_mutation!(
    multiple_numeric_exits_denied,
    "exit_receipt",
    |_: &Fixture, p: &Path| write(p, b"0\n0\n")
);
artifact_mutation!(
    signed_exit_denied,
    "exit_receipt",
    |_: &Fixture, p: &Path| write(p, b"+0\n")
);
artifact_mutation!(
    out_of_range_exit_denied,
    "exit_receipt",
    |_: &Fixture, p: &Path| write(p, b"256\n")
);
artifact_mutation!(
    whitespace_exit_denied,
    "exit_receipt",
    |_: &Fixture, p: &Path| write(p, b" 0 \n")
);
#[test]
fn injected_receipt_never_creates_a_sentinel_during_verification() {
    let (f, p, c) = captured_js();
    let e = f.execution(&c[0]);
    let sentinel = f.tmp.path().join("injected");
    write(
        f.artifact_path(&e["artifacts"]["exit_receipt"]),
        format!("$(touch {}); echo 0\n", sentinel.display()),
    );
    verification(&f.verify(&p), false);
    assert!(!sentinel.exists());
}

#[cfg(unix)]
#[test]
fn raw_log_symlink_cannot_escape_store() {
    use std::os::unix::fs::symlink;
    let (f, p, c) = captured_js();
    let e = f.execution(&c[0]);
    let path = f.artifact_path(&e["artifacts"]["raw_log"]);
    let outside = f.tmp.path().join("outside-log");
    fs::copy(&path, &outside).unwrap();
    fs::remove_file(&path).unwrap();
    symlink(outside, &path).unwrap();
    verification(&f.verify(&p), false);
}

#[test]
fn raw_log_directory_is_not_readable_execution_output() {
    let (f, p, c) = captured_js();
    let e = f.execution(&c[0]);
    let path = f.artifact_path(&e["artifacts"]["raw_log"]);
    fs::remove_file(&path).unwrap();
    fs::create_dir(path).unwrap();
    verification(&f.verify(&p), false);
}

macro_rules! record_mutation {
    ($name:ident,$pointer:literal,$value:expr) => {
        #[test]
        fn $name() {
            let (f, p, c) = captured_js();
            let path = f.artifact_path(&c[0]["record"]);
            let mut e: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            *e.pointer_mut($pointer).expect("documented execution field") = $value;
            write(path, serde_json::to_vec(&e).unwrap());
            verification(&f.verify(&p), false);
        }
    };
}
record_mutation!(
    forged_command_metadata_denied,
    "/command/argv",
    json!(["node", "--version"])
);
record_mutation!(
    forged_runner_metadata_denied,
    "/command/runner",
    json!("cargo")
);
record_mutation!(
    forged_toolchain_metadata_denied,
    "/command/toolchain",
    json!("rust")
);
record_mutation!(
    forged_executable_digest_denied,
    "/command/executable/sha256",
    json!("0".repeat(64))
);
record_mutation!(
    forged_repository_root_denied,
    "/repository/root",
    json!("/elsewhere")
);
record_mutation!(
    forged_repository_identity_denied,
    "/repository/identity",
    json!("other/repo")
);
record_mutation!(
    forged_worktree_denied,
    "/repository/git_dir",
    json!("/different/.git")
);
record_mutation!(
    forged_workflow_start_denied,
    "/workflow/started_at",
    json!("2026-10-27T21:23:14Z")
);
record_mutation!(
    forged_source_head_denied,
    "/source_before/head",
    json!("f".repeat(40))
);
record_mutation!(
    forged_source_fingerprint_denied,
    "/source_after/fingerprint",
    json!("0".repeat(64))
);
record_mutation!(
    incomplete_capture_denied,
    "/process/termination",
    json!("incomplete")
);
record_mutation!(edited_process_status_denied, "/process/exit_code", json!(7));
record_mutation!(
    traversal_raw_log_denied,
    "/artifacts/raw_log/path",
    json!("../../outside")
);
record_mutation!(
    absolute_raw_log_denied,
    "/artifacts/raw_log/path",
    json!("/etc/passwd")
);

#[test]
fn supplied_hashes_cannot_replace_independent_capture_binding() {
    let (f, p, c) = captured_js();
    let e = f.execution(&c[0]);
    fs::remove_file(f.artifact_path(&e["capture_binding"]["metadata"])).unwrap();
    verification(&f.verify(&p), false);
}

#[test]
fn unknown_execution_id_never_credits_submitted_pass() {
    let (mut f, p, _) = captured_js();
    f.context["precommit_results"]["checks"][0]["execution_ids"] = json!(["invented-execution"]);
    verification(&f.verify(&p), false);
}

#[test]
fn reported_failure_wins_over_successful_captures() {
    let (mut f, p, _) = captured_js();
    f.context["precommit_results"]["status"] = json!("FAIL");
    verification(&f.verify(&p), false);
}

#[test]
fn two_scenarios_cannot_reuse_one_execution() {
    let (mut f, p, _) = captured_js();
    f.context["local_testing_gate"]["scenarios"][1]["execution_id"] =
        f.context["local_testing_gate"]["scenarios"][0]["execution_id"].clone();
    verification(&f.verify(&p), false);
}

#[test]
fn scenario_without_assertions_does_not_count() {
    let (mut f, p, _) = captured_js();
    f.context["local_testing_gate"]["scenarios"][1]["assertions"] = json!([]);
    verification(&f.verify(&p), false);
}

#[test]
fn one_basic_scenario_is_insufficient() {
    let (mut f, p, _) = captured_js();
    f.context["local_testing_gate"]["scenarios"]
        .as_array_mut()
        .unwrap()
        .pop();
    verification(&f.verify(&p), false);
}

#[test]
fn mismatched_step_report_plans_cannot_be_assembled() {
    let (mut f, p, _) = captured_js();
    f.context["local_testing_gate"]["plan_id"] = json!("another-plan");
    verification(&f.verify(&p), false);
}

fn mixed_rust_policy_fixture(argv: &[&str]) -> Fixture {
    // Policy-only fixture: a valid workspace, without simulated Cargo output.
    let mut f = Fixture::javascript();
    f.file("native/Cargo.toml","[package]\nname='native-contract'\nversion='0.1.0'\nedition='2021'\n[workspace]\nmembers=['.']\n");
    f.file(
        "native/src/lib.rs",
        "pub fn decode(v:i32)->i32 {v*2}\n#[test] fn contract(){assert_eq!(decode(2),4);}\n",
    );
    f.file(
        "native/Cargo.lock",
        "version = 3\n\n[[package]]\nname = \"native-contract\"\nversion = \"0.1.0\"\n",
    );
    f.commit("maintained mixed-language workspace");
    f.plan = f.node_plan("javascript");
    let component = json!({"id":"native","root":"native","toolchain":"rust","disposition":"required",
        "inspected":[f.reference("native/Cargo.toml"),f.reference("native/Cargo.lock")],"justification":"maintained workspace"});
    f.plan["components"].as_array_mut().unwrap().push(component);
    let mut obligation = f.obligation("rust-tests", "test", "rust", "cargo", argv);
    obligation["component_ids"] = json!(["native"]);
    obligation["cwd"] = json!(f.repo.join("native"));
    obligation["scope"] = json!("FULL_WORKSPACE");
    obligation["threading"] = json!("DEFAULT");
    obligation["selected_from"] = json!([f.reference("native/Cargo.toml")]);
    f.plan["obligations"]
        .as_array_mut()
        .unwrap()
        .push(obligation);
    f.plan["exclusions"][0]["component_ids"] = json!(["root", "native"]);
    f
}

#[test]
fn full_rust_and_node_policy_registers_but_does_not_imply_execution() {
    let f = mixed_rust_policy_fixture(&["cargo", "test", "--workspace", "--locked"]);
    let p = f.register();
    verification(&f.verify(&p), false);
}

macro_rules! narrowed_rust {
    ($name:ident,$argv:expr) => {
        #[test]
        fn $name() {
            let f = mixed_rust_policy_fixture($argv);
            let r = f.call("plan", &f.plan);
            r.exit(1);
            assert_eq!(r.json()["status"], "UNPROVEN");
        }
    };
}
narrowed_rust!(
    rust_package_only_is_not_full_workspace,
    &["cargo", "test", "-p", "native", "--locked"]
);
narrowed_rust!(
    rust_package_selection_does_not_satisfy_scope,
    &["cargo", "test", "--workspace", "--locked", "-p", "native"]
);
narrowed_rust!(
    rust_exclusion_is_not_full_workspace,
    &[
        "cargo",
        "test",
        "--workspace",
        "--locked",
        "--exclude",
        "other"
    ]
);
narrowed_rust!(
    rust_filter_is_not_full_workspace,
    &["cargo", "test", "--workspace", "--locked", "one_test"]
);
narrowed_rust!(
    rust_no_run_does_not_execute_tests,
    &["cargo", "test", "--workspace", "--locked", "--no-run"]
);
narrowed_rust!(
    rust_skip_does_not_execute_full_workspace,
    &[
        "cargo",
        "test",
        "--workspace",
        "--locked",
        "--",
        "--skip",
        "one_test"
    ]
);
narrowed_rust!(
    rust_explicit_thread_arg_is_not_default,
    &[
        "cargo",
        "test",
        "--workspace",
        "--locked",
        "--",
        "--test-threads=1"
    ]
);
narrowed_rust!(
    shell_printing_cargo_is_not_cargo_execution,
    &["bash", "-c", "echo cargo test --workspace --locked"]
);

#[test]
#[ignore = "native Rust denial: run sequentially under measured memory admission"]
fn rust_environment_thread_override_cannot_be_labelled_default() {
    let mut f = Fixture::native_project("rust");
    f.env.insert("RUST_TEST_THREADS".into(), "1".into());
    let p = f.register();
    f.execute_all(&p);
    verification(&f.verify(&p), false);
}

#[test]
#[ignore = "native mixed-language proof: run sequentially under measured memory admission"]
fn mixed_node_and_rust_require_all_captures() {
    let mut f = Fixture::native_project("rust");
    f.file(
        "web/package.json",
        r#"{"name":"web","private":true,"scripts":{"test":"node --test contract.test.cjs"}}"#,
    );
    f.file("web/contract.test.cjs","const {test}=require('node:test'); const assert=require('node:assert/strict'); test('native consumer',()=>assert.equal([2].map(x=>x*2)[0],4));\n");
    f.commit("additional maintained Node component");
    f.plan["components"][0]["inspected"] =
        json!([f.reference("Cargo.toml"), f.reference("README.md")]);
    let component = json!({"id":"web","root":"web","toolchain":"javascript","disposition":"required","inspected":[f.reference("web/package.json")],"justification":"maintained package"});
    f.plan["components"].as_array_mut().unwrap().push(component);
    f.plan["exclusions"][0]["component_ids"] = json!(["root", "web"]);
    let mut o = f.obligation(
        "web-tests",
        "test",
        "javascript",
        "node:test",
        &["npm", "test"],
    );
    o["component_ids"] = json!(["web"]);
    o["cwd"] = json!(f.repo.join("web"));
    o["selected_from"] = json!([f.reference("web/package.json")]);
    f.plan["obligations"].as_array_mut().unwrap().push(o);
    let p = f.register();
    f.execute_all(&p);
    verification(&f.verify(&p), true);
    f.context["precommit_results"]["checks"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["obligation_id"] != "tests");
    verification(&f.verify(&p), false);
}

#[test]
fn resumed_valid_cohort_restores_original_typed_records_without_mutating_input() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    let original_implementation = f.context["implementation_terminal_evidence"].clone();
    let original_verification = f.context["verification_terminal_evidence"].clone();
    for key in [
        "implementation_terminal_evidence",
        "verification_terminal_evidence",
        "implementation",
        "verdict_json",
        "precommit_results",
        "local_testing_gate",
    ] {
        f.context.as_object_mut().unwrap().remove(key);
    }
    f.context["resume_checkpoint"] = json!("checkpoint-after-review-feedback");
    f.save_context();
    let before = fs::read(f.context_path()).unwrap();
    let r = f.restore();
    r.exit(0);
    let response = r.json();
    assert_eq!(response["implementation_valid"], true);
    assert_eq!(response["verification_valid"], true);
    assert_eq!(response["implementation_refresh_required"], "false");
    assert_eq!(response["verification_refresh_required"], "false");
    let derived = f.derived(&response);
    assert_eq!(
        derived["implementation_terminal_evidence"],
        original_implementation
    );
    assert_eq!(
        derived["verification_terminal_evidence"],
        original_verification
    );
    assert_eq!(derived["unrelated"], f.context["unrelated"]);
    assert_eq!(fs::read(f.context_path()).unwrap(), before);
}

macro_rules! authoritative_invalid {
    ($name:ident,$value:expr) => {
        #[test] fn $name() {
            let mut f=Fixture::javascript(); f.verified_cohort();
            f.context["implementation_terminal_evidence"]=$value;
            // Stale positive aliases must not revive an authoritative record.
            f.env.insert("IMPLEMENTATION_COMPLETED".into(),"true".into());
            f.env.insert("RECIPE_VAR_implementation_terminal_evidence".into(),json!({"implementation_completed":"true","terminal_no_op":"false","terminal_state":"IMPLEMENTATION_COMPLETED","terminal_reason":"stale"}).to_string());
            let r=f.restore(); assert!(!r.0.status.success(),"{}",r.describe());
            assert_eq!(r.json()["implementation_valid"],false);
            let derived=f.derived(&r.json()); assert_ne!(derived["implementation_terminal_evidence"]["implementation_completed"],"true");
        }
    };
}
authoritative_invalid!(
    explicit_negative_beats_cached_positive,
    json!({"implementation_completed":"false","terminal_no_op":"false","terminal_state":"IMPLEMENTATION_UNPROVEN","terminal_reason":"original failed"})
);
authoritative_invalid!(null_is_not_absence, json!(null));
authoritative_invalid!(empty_object_is_not_absence, json!({}));
authoritative_invalid!(
    malformed_terminal_string_is_not_absence,
    json!("not a typed record")
);
authoritative_invalid!(
    boolean_completion_is_not_the_string_contract,
    json!({"implementation_completed":true,"terminal_no_op":"false","terminal_state":"IMPLEMENTATION_COMPLETED","terminal_reason":"wrong type"})
);
authoritative_invalid!(
    terminal_extra_fields_are_invalid,
    json!({"implementation_completed":"true","terminal_no_op":"false","terminal_state":"IMPLEMENTATION_COMPLETED","terminal_reason":"extra","trusted":true})
);

#[test]
fn checkpoint_missing_implementation_never_invents_completion_from_review_or_prose() {
    let mut f = Fixture::javascript();
    f.context["implementation"] =
        json!("STATUS: COMPLETE; tests passed; published and independently CLEAN");
    f.context["build_result"] = json!({"status":"PASS"});
    f.context["review_result"] = json!({"crusty_verdict":"CLEAN"});
    let r = f.call("checkpoint-implementation", &f.request());
    r.exit(1);
    assert_eq!(r.json()["implementation_state"], "MISSING");
    let restored = f.restore();
    restored.exit(1);
    assert_eq!(restored.json()["implementation_valid"], false);
    assert_ne!(
        f.derived(&restored.json())["implementation_terminal_evidence"]["implementation_completed"],
        "true"
    );
}

#[test]
fn implementation_checkpoint_alone_is_not_a_verified_cohort() {
    let mut f = Fixture::javascript();
    f.install_original_implementation();
    f.checkpoint();
    let r = f.restore();
    r.exit(1);
    assert_eq!(r.json()["implementation_valid"], true);
    assert_eq!(r.json()["verification_valid"], false);
    assert_eq!(r.json()["verification_refresh_required"], "true");
}

#[test]
fn another_workflow_start_cannot_restore_cached_evidence() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    f.context
        .as_object_mut()
        .unwrap()
        .remove("implementation_terminal_evidence");
    let mut r = f.request();
    r["workflow"]["started_at"] = json!("2026-10-27T23:04:28Z");
    f.call("restore", &r).error("IDENTITY_MISMATCH");
}

#[test]
fn another_worktree_never_inherits_this_cohort() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    let other = f.tmp.path().join("other-worktree");
    f.native(
        "git",
        &["worktree", "add", "-b", "other", other.to_str().unwrap()],
    );
    let mut r = f.request();
    r["repository_root"] = json!(other);
    f.call("restore", &r).error("IDENTITY_MISMATCH");
}

#[cfg(unix)]
#[test]
fn evidence_store_and_artifacts_remain_private() {
    use std::os::unix::fs::PermissionsExt;
    let (f, _, _) = captured_js();
    for p in files_under(&f.store()) {
        let mode = fs::symlink_metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "private artifact: {}", p.display());
        let parent_mode = fs::metadata(p.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(parent_mode & 0o077, 0, "private evidence directory");
    }
}

#[test]
fn local_verification_success_does_not_fabricate_green_ci() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    let r = f.finalize("collect");
    r.exit(0);
    let collected = r.json();
    assert_eq!(collected["completion"]["verification_completed"], "true");
    assert_eq!(collected["ci"]["state"], "UNKNOWN");
    assert_eq!(collected["local_evidence"]["schema_version"], 1);
}

#[test]
fn native_consumer_fixture_asserts_basic_and_invalid_input_behavior() {
    let f = Fixture::javascript();
    for role in ["basic", "edge"] {
        let r = f.native("node", &["consumer.js", role]);
        assert!(String::from_utf8_lossy(&r.0.stdout).contains("consumer assertions passed"));
    }
    f.native("npm", &["test"]);
}

#[test]
fn resume_and_uninterrupted_execution_have_identical_classification() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    let collected = f.finalize("collect");
    collected.exit(0);
    f.context["finalization_evidence"] = collected.json();
    let uninterrupted = f.finalize("validate");
    uninterrupted.exit(0);
    let original = uninterrupted.json();
    for key in [
        "implementation_terminal_evidence",
        "verification_terminal_evidence",
        "implementation",
        "verdict_json",
        "precommit_results",
        "local_testing_gate",
        "finalization_evidence",
    ] {
        f.context.as_object_mut().unwrap().remove(key);
    }
    f.context["resume_checkpoint"] = json!("checkpoint-after-review-feedback");
    let collected = f.finalize("collect");
    collected.exit(0);
    f.context["finalization_evidence"] = collected.json();
    let resumed = f.finalize("validate");
    resumed.exit(0);
    for key in [
        "terminal_state",
        "terminal_success",
        "terminal_failure",
        "implementation_completed",
        "verification_completed",
    ] {
        assert_eq!(
            resumed.json()[key],
            original[key],
            "classification differs after resume: {key}"
        );
    }
    f.context["workflow_result"] = resumed.json();
    let completed = f.finalize("complete");
    completed.exit(0);
    assert_eq!(completed.json()["terminal_success"], "true");
}

fn install_platform_fixture(f: &mut Fixture, checks: Value) {
    // Only platform responses are simulated. These cannot supply local captures
    // or original implementation evidence. Every mutating gh call is refused.
    f.native(
        "git",
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/fixture.git",
        ],
    );
    let response = json!({"number":7,"url":"https://github.com/owner/fixture/pull/7",
        "state":"OPEN","headRefName":"feature","baseRefName":"main","headRefOid":f.head(),
        "headRepositoryOwner":{"login":"owner"},"headRepository":{"name":"fixture"},
        "isCrossRepository":false,"isDraft":false,"statusCheckRollup":checks});
    let path = f.tmp.path().join("platform.json");
    write(&path, serde_json::to_vec(&response).unwrap());
    let gh = f.tmp.path().join("bin/gh");
    write(&gh,b"#!/usr/bin/env bash\nset -eu\nprintf '%s\\n' \"$*\" >> \"$PLATFORM_CALLS\"\ncase \"${1:-} ${2:-}\" in\n 'auth status') exit 0 ;;\n 'pr view') cat \"$PLATFORM_RESPONSE\" ;;\n 'pr list') jq -s . \"$PLATFORM_RESPONSE\" ;;\n *) echo 'unexpected platform mutation' >&2; exit 99 ;;\nesac\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
    }
    f.env.insert(
        "PATH".into(),
        format!(
            "{}:{}",
            f.tmp.path().join("bin").display(),
            std::env::var("PATH").unwrap()
        ),
    );
    f.env
        .insert("PLATFORM_RESPONSE".into(), path.display().to_string());
    f.env.insert(
        "PLATFORM_CALLS".into(),
        f.tmp.path().join("platform-calls").display().to_string(),
    );
    f.context["remote_host_type"] = json!("github");
    f.context["pr_url"] = json!("https://github.com/owner/fixture/pull/7");
    f.context["pr_number"] = json!("7");
    f.context["pr_publish_result"] = json!({"pr_url":"https://github.com/owner/fixture/pull/7","pr_number":"7","status":"PUBLISHED"});
}

#[test]
fn open_pr_green_ci_and_clean_review_cannot_supply_missing_implementation() {
    let mut f = Fixture::javascript();
    install_platform_fixture(
        &mut f,
        json!([{"__typename":"CheckRun","name":"ci","status":"COMPLETED","conclusion":"SUCCESS"}]),
    );
    f.context["review_result"] = json!({"crusty_verdict":"CLEAN"});
    f.context["build_result"] = json!({"status":"SKIPPED","pr_number":"7"});
    f.context["implementation"] =
        json!("The work is complete; all checks and independent reviews passed.");
    let r = f.finalize("collect");
    r.exit(0);
    f.context["finalization_evidence"] = r.json();
    let r = f.finalize("validate");
    r.exit(1);
    let value = r.json();
    assert_eq!(value["terminal_state"], "FAILED_IMPLEMENTATION");
    assert_eq!(value["terminal_success"], "false");
    assert_eq!(value["terminal_failure"], "true");
    assert_eq!(value["finalizer_output_valid"], "false");
    assert_eq!(value["pr_url"], "https://github.com/owner/fixture/pull/7");
    assert_eq!(value["publish_state_reached"], "true");
}

#[test]
fn empty_platform_ci_stays_unknown_after_valid_local_checks() {
    let mut f = Fixture::javascript();
    install_platform_fixture(&mut f, json!([]));
    // Capture the selected repository identity, rather than adding it afterward.
    f.verified_cohort();
    let r = f.finalize("collect");
    r.exit(0);
    assert_eq!(r.json()["completion"]["verification_completed"], "true");
    assert_ne!(r.json()["ci"]["state"], "GREEN");
}

macro_rules! finalization_blocker {
    ($name:ident,$mutate:expr,$state:literal) => {
        #[test]
        fn $name() {
            let mut f = Fixture::javascript();
            f.verified_cohort();
            let r = f.finalize("collect");
            r.exit(0);
            f.context["finalization_evidence"] = r.json();
            ($mutate)(&mut f);
            let r = f.finalize("validate");
            r.exit(1);
            assert_eq!(r.json()["terminal_state"], $state);
            assert_eq!(r.json()["terminal_success"], "false");
        }
    };
}
finalization_blocker!(
    valid_local_evidence_cannot_override_failed_ci,
    |f: &mut Fixture| {
        f.context["finalization_evidence"]["ci"]["state"] = json!("FAILURE");
    },
    "BLOCKED_CI"
);
finalization_blocker!(
    dirty_worktree_failure_precedes_stale_evidence,
    |f: &mut Fixture| {
        f.file("contract.js", "export const changed=true;\n");
        f.context["finalization_evidence"]["git"]["dirty_worktree"] = json!("true");
    },
    "FAILED_DIRTY_WORKTREE"
);
finalization_blocker!(
    reporting_failure_remains_independent_of_local_success,
    |f: &mut Fixture| {
        f.context["finalizer_step_status"] = json!({"status":"failed","reporting_failure":"true"});
    },
    "FAILED_REPORTING"
);

#[test]
fn source_change_between_validation_and_completion_requires_revalidation() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    let r = f.finalize("collect");
    r.exit(0);
    f.context["finalization_evidence"] = r.json();
    let r = f.finalize("validate");
    r.exit(0);
    f.context["workflow_result"] = r.json();
    f.file("contract.js", "export const changed=true;\n");
    let r = f.finalize("complete");
    assert!(!r.0.status.success(), "{}", r.describe());
    assert_eq!(r.json()["terminal_success"], "false");
}

#[test]
fn relevant_environment_change_invalidates_captured_execution() {
    let (mut f, p, _) = captured_js();
    f.env.insert("NODE_OPTIONS".into(), "--no-warnings".into());
    verification(&f.verify(&p), false);
}

#[test]
fn unrelated_secret_is_not_retained_in_capture_metadata() {
    let mut f = Fixture::javascript();
    let marker = "private-unrelated-value-must-not-be-dumped";
    f.env
        .insert("WORKFLOW_TEST_UNRELATED_SECRET".into(), marker.into());
    let p = f.register();
    let c = f.capture(&p, "tests");
    let e = f.execution(&c);
    assert!(!e["process"]["environment"].to_string().contains(marker));
    let metadata = f.artifact(&e["capture_binding"]["metadata"]);
    assert!(!String::from_utf8_lossy(&metadata).contains(marker));
}

#[cfg(unix)]
#[test]
fn unreadable_raw_log_cannot_verify() {
    use std::os::unix::fs::PermissionsExt;
    let (f, p, c) = captured_js();
    let e = f.execution(&c[0]);
    let path = f.artifact_path(&e["artifacts"]["raw_log"]);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let r = f.verify(&p);
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    verification(&r, false);
}

#[cfg(unix)]
#[test]
fn changed_resolved_executable_cannot_use_old_capture() {
    use std::os::unix::fs::PermissionsExt;
    let (mut f, p, _) = captured_js();
    let replacement = f.tmp.path().join("bin/npm");
    write(&replacement, b"#!/bin/sh\nexit 0\n");
    fs::set_permissions(replacement, fs::Permissions::from_mode(0o700)).unwrap();
    f.env.insert(
        "PATH".into(),
        format!(
            "{}:{}",
            f.tmp.path().join("bin").display(),
            std::env::var("PATH").unwrap()
        ),
    );
    verification(&f.verify(&p), false);
}

#[cfg(target_os = "linux")]
#[test]
fn interrupted_capture_remains_an_incomplete_attempt() {
    let mut f = Fixture::javascript();
    // The command kills only its own newly spawned capture parent. It exits
    // promptly itself, so no timers or processes survive the test.
    f.file("interrupt.js","import fs from 'node:fs'; const env=fs.readFileSync('/proc/'+process.ppid+'/environ').toString().split('\\0'); if (!env.includes('AMPLIHACK_CONTEXT_FILE='+process.env.AMPLIHACK_CONTEXT_FILE) || !env.includes('TMPDIR='+process.env.TMPDIR)) process.exit(9); process.kill(process.ppid,'SIGKILL'); process.exit(0);\n");
    f.commit("interrupted capture fixture");
    f.plan = f.node_plan("javascript");
    f.plan["obligations"][0]["argv"] = json!(["node", "interrupt.js"]);
    let p = f.register();
    let r = f.call("capture", &f.capture_request(&p, "tests"));
    assert!(!r.0.status.success(), "{}", r.describe());
    assert!(
        files_under(&f.store())
            .iter()
            .any(|p| p.components().any(|c| c.as_os_str() == "attempts")),
        "reserve and retain attempt before spawn"
    );
    verification(&f.verify(&p), false);
}

#[test]
fn concurrent_verified_generations_publish_atomically() {
    let mut f = Fixture::javascript();
    let p = f.verified_cohort();
    f.save_context();
    let mut request = f.request();
    request["plan_id"] = p["plan_id"].clone();
    let path = f.tmp.path().join("concurrent-verify.json");
    write(&path, serde_json::to_vec(&request).unwrap());
    let outcomes = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            Run(f
                .command("bash")
                .arg(tool("workflow_terminal_evidence.sh"))
                .args(["verify", "--request-file", path.to_str().unwrap()])
                .output()
                .unwrap())
        });
        let b = scope.spawn(|| {
            Run(f
                .command("bash")
                .arg(tool("workflow_terminal_evidence.sh"))
                .args(["verify", "--request-file", path.to_str().unwrap()])
                .output()
                .unwrap())
        });
        [a.join().unwrap(), b.join().unwrap()]
    });
    for r in outcomes {
        verification(&r, true);
    }
    let r = f.restore();
    r.exit(0);
    assert_eq!(r.json()["verification_valid"], true);
    for path in files_under(&f.store())
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "current.json"))
    {
        let current: Value =
            serde_json::from_slice(&fs::read(path).unwrap()).expect("atomic generation pointer");
        assert!(current.is_object());
    }
}

#[test]
fn missing_selected_records_do_not_unlock_inherited_completion_aliases() {
    let mut f = Fixture::javascript();
    f.env
        .insert("IMPLEMENTATION_COMPLETED".into(), "true".into());
    f.env.insert("VERIFICATION_COMPLETED".into(), "true".into());
    let r = f.restore();
    r.exit(1);
    let value = r.json();
    assert_eq!(value["implementation_valid"], false);
    assert_eq!(value["verification_valid"], false);
}

#[test]
fn stale_implementation_cannot_be_repaired_by_rerunning_verification() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    f.file(
        "contract.js",
        "export function decode(v) { return v.map(x=>x*3); }\n",
    );
    // Update assertions honestly for the new source, but do not rerun the
    // implementation producer/verifier or relabel its original bindings.
    f.file("contract.test.js","import {test} from 'node:test'; import assert from 'node:assert/strict'; import {decode} from './contract.js'; test('new contract',()=>assert.deepEqual(decode([2]),[6]));\n");
    f.file("consumer.js","import assert from 'node:assert/strict'; import {decode} from './contract.js'; if(process.argv[2]==='basic') assert.deepEqual(decode([2]),[6]); else if(process.argv[2]==='edge') assert.deepEqual(decode([]),[]); else process.exit(7); console.log(process.argv[2]+' consumer assertions passed');\n");
    f.commit("new implementation without original verifier refresh");
    f.plan = f.node_plan("javascript");
    f.plan["obligations"][2]["scenario"]["assertions"] =
        json!(["public decoder triples [2] to [6]"]);
    f.plan["obligations"][3]["scenario"]["assertions"] =
        json!(["empty input returns an empty result"]);
    let p = f.register();
    f.execute_all(&p);
    verification(&f.verify(&p), true);
    let r = f.restore();
    r.exit(1);
    assert_eq!(r.json()["implementation_valid"], false);
    assert_eq!(r.json()["implementation_refresh_required"], "true");
}

#[test]
fn noop_permission_cannot_override_an_authoritative_failed_execution() {
    let (mut f, p, _) = captured_js();
    f.context["allow_no_op"] = json!(true);
    f.context["precommit_results"]["status"] = json!("FAIL");
    let r = f.verify(&p);
    r.exit(0);
    let value = r.json();
    assert_eq!(value["verification_completed"], "false");
    assert_eq!(value["terminal_no_op"], "false");
}

#[test]
fn explicitly_authorised_clean_noop_keeps_its_distinct_terminal_record() {
    let mut f = Fixture::javascript();
    let p = f.register();
    f.context["allow_no_op"] = json!(true);
    let r = f.verify(&p);
    r.exit(0);
    let value = r.json();
    assert_eq!(value.as_object().unwrap().len(), 4);
    assert!(value.as_object().unwrap().values().all(Value::is_string));
    assert_eq!(value["verification_completed"], "false");
    assert_eq!(value["terminal_no_op"], "true");
    assert_eq!(value["terminal_state"], "ALLOW_NO_OP");
}

#[test]
fn collection_reassesses_source_after_final_cleanup() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    // Model the existing cleanup boundary, not an execution receipt. Only this
    // fixture's selected framework copy and repository can be changed.
    let framework = f.tmp.path().join("framework");
    let tools = framework.join("amplifier-bundle/tools");
    fs::create_dir_all(&tools).unwrap();
    for entry in fs::read_dir(workspace_root().join("amplifier-bundle/tools")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), tools.join(entry.file_name())).unwrap();
        }
    }
    write(tools.join("workflow_runtime_artifacts.sh"),b"finalize_workflow_cleanup_entry() { printf 'export const afterCleanup=true;\\n' > \"$CLEANUP_SOURCE\"; }\n");
    f.env
        .insert("AMPLIHACK_HOME".into(), framework.display().to_string());
    f.env.insert(
        "CLEANUP_SOURCE".into(),
        f.repo.join("post-cleanup-source.js").display().to_string(),
    );
    let r = f.finalize("collect");
    r.exit(0);
    let collected = r.json();
    assert!(
        f.repo.join("post-cleanup-source.js").exists(),
        "cleanup fixture must have executed"
    );
    assert_eq!(collected["completion"]["verification_completed"], "false");
    assert_eq!(collected["git"]["dirty_worktree"], "true");
    assert!(collected["local_evidence"].is_object());
}

#[test]
#[ignore = "native hook proof requires real pre-commit; run sequentially under measured admission"]
fn configured_precommit_hook_is_executed_and_mapped() {
    let mut f = Fixture::javascript();
    f.file(".pre-commit-config.yaml","repos:\n  - repo: local\n    hooks:\n      - id: syntax\n        name: JavaScript syntax\n        entry: node --check contract.js\n        language: system\n        always_run: true\n        pass_filenames: false\n");
    f.file("README.md","Maintained JavaScript package. Test: npm test. Quality: npm run quality. Pre-commit: pre-commit run --all-files. Consumers: node consumer.js basic; node consumer.js edge.\n");
    f.commit("repository hook policy");
    f.plan = f.node_plan("javascript");
    f.plan["exclusions"] = json!([]);
    let hook = f.obligation(
        "hooks",
        "precommit",
        "javascript",
        "pre-commit",
        &["pre-commit", "run", "--all-files"],
    );
    f.plan["obligations"].as_array_mut().unwrap().push(hook);
    let p = f.register();
    let captures = f.execute_all(&p);
    f.context["precommit_results"]["exclusions"] = json!([]);
    verification(&f.verify(&p), true);
    assert!(
        captures
            .iter()
            .any(|c| f.execution(c)["command"]["runner"] == "pre-commit")
    );
}

#[test]
fn direct_completion_cannot_bypass_missing_implementation_guard() {
    let mut f = Fixture::javascript();
    f.context["workflow_result"] = json!({"terminal_state":"IMPLEMENTED_VERIFIED","terminal_success":"true","terminal_failure":"false","finalizer_output_valid":"true","implementation_completed":"true","verification_completed":"true"});
    let r = f.finalize("complete");
    assert!(!r.0.status.success(), "{}", r.describe());
    let v = r.json();
    assert_eq!(v["terminal_success"], "false");
    assert_eq!(
        v["workflow_result"]["terminal_state"],
        "FAILED_IMPLEMENTATION"
    );
}

#[test]
fn retained_legacy_modes_keep_report_only_and_hard_negative_contracts() {
    let f = Fixture::javascript();
    let evidence = json!({"evidence_readable":"true","hard_negative":"true","positive_artifact":"false",
        "mechanical_verdict":"FAILED","mechanical_reason":"readable evidence shows no published work",
        "merged_pr_count":0,"open_pr_count":0,"work_in_base":"false"});
    let r = f.run_raw(
        &["adjudicate", "--agent-verdict", "SUCCESS", "-"],
        &serde_json::to_vec(&evidence).unwrap(),
    );
    r.exit(1);
    assert_eq!(r.json()["terminal_verdict"], "FAILED");
}

#[test]
#[ignore = "read-only historical diagnostic; set WORKFLOW_2204_RECOVERY to retained original artifacts"]
fn retained_2204_scenarios_remain_historical_and_cannot_verify_2223() {
    let root = std::path::PathBuf::from(
        std::env::var_os("WORKFLOW_2204_RECOVERY").expect("retained diagnostic root"),
    );
    let receipt_path = root.join("step16b/exact-receipt.json");
    let diagnosis_path = root.join("finalization-diagnosis.json");
    let before = fs::read(&diagnosis_path).unwrap();
    let diagnosis: Value = serde_json::from_slice(&before).unwrap();
    let receipt: Value = serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
    let historical = "51f5eeceeeea08d63b4ed8b779396454b4148e2a";
    assert_eq!(receipt["source_head"], historical);
    assert_eq!(diagnosis["source_head"], historical);
    assert!(
        diagnosis["failure"]
            .as_str()
            .unwrap()
            .starts_with("FAILED_IMPLEMENTATION")
    );
    assert!(
        receipt["pr"]["url"]
            .as_str()
            .unwrap()
            .contains("mistt-app-ui")
    );
    for key in ["scenario_simple", "scenario_edge_integration"] {
        let scenario = &receipt["outside_in"][key];
        let bytes = fs::read(scenario["log"].as_str().unwrap()).unwrap();
        assert_eq!(sha(&bytes), scenario["log_sha256"]);
        assert_eq!(scenario["exit"], 0);
        assert!(String::from_utf8_lossy(&bytes).contains("1 passed"));
    }
    let mut f = Fixture::javascript();
    let p = f.register();
    // Preserve the original identities. These are historical summaries, not
    // captures of this framework head or missing implementation completion.
    f.context["precommit_results"] = receipt["retained_source_checks"].clone();
    f.context["local_testing_gate"] = receipt["outside_in"].clone();
    verification(&f.verify(&p), false);
    let r = f.restore();
    r.exit(1);
    assert_eq!(r.json()["implementation_valid"], false);
    assert_eq!(fs::read(diagnosis_path).unwrap(), before);
}

#[test]
fn checkpoint_entry_on_resume_preserves_the_original_valid_cohort() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    let original = f.context["implementation_terminal_evidence"].clone();
    for key in [
        "implementation_terminal_evidence",
        "implementation",
        "verdict_json",
    ] {
        f.context.as_object_mut().unwrap().remove(key);
    }
    f.checkpoint();
    let r = f.restore();
    r.exit(0);
    assert_eq!(
        f.derived(&r.json())["implementation_terminal_evidence"],
        original
    );
}

#[test]
fn checkpoint_cannot_rebind_unchanged_verifier_support_to_uncommitted_source() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    f.file("new-source.js", "export const change=true;\n");
    let r = f.call("checkpoint-implementation", &f.request());
    r.exit(1);
    assert_eq!(r.json()["implementation_state"], "STALE");
    let r = f.restore();
    r.exit(1);
    assert_eq!(r.json()["implementation_valid"], false);
}

#[test]
fn a_silent_success_cannot_replace_the_configured_test_command() {
    let mut f = Fixture::javascript();
    f.file("silent.js", "");
    f.commit("unrelated successful command");
    f.plan = f.node_plan("javascript");
    f.plan["obligations"][0]["argv"] = json!(["node", "silent.js"]);
    let p = f.register();
    f.execute_all(&p);
    verification(&f.verify(&p), false);
}

#[test]
fn adding_a_git_hook_invalidates_the_prior_configuration_binding() {
    let (f, p, _) = captured_js();
    write(f.repo.join(".git/hooks/pre-commit"), b"#!/bin/sh\nexit 1\n");
    verification(&f.verify(&p), false);
}

#[test]
fn an_independent_nested_rust_workspace_cannot_inherit_parent_coverage() {
    let f = mixed_rust_policy_fixture(&["cargo", "test", "--workspace", "--locked"]);
    f.file("native/hidden/Cargo.toml", "[workspace]\nmembers=[]\n");
    f.commit("independent nested workspace");
    let r = f.call("plan", &f.plan);
    r.exit(1);
    assert_eq!(r.json()["status"], "UNPROVEN");
}

#[test]
fn typescript_configuration_cannot_be_hidden_by_a_javascript_label() {
    let f = Fixture::javascript();
    f.file("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    f.commit("TypeScript component policy");
    let r = f.call("plan", &f.plan);
    r.exit(1);
    assert_eq!(r.json()["status"], "UNPROVEN");
}

#[test]
fn a_git_exclude_change_cannot_hide_new_source_from_freshness_checks() {
    let (f, p, _) = captured_js();
    f.file("new-source.js", "export const hiddenChange=true;\n");
    write(f.repo.join(".git/info/exclude"), b"new-source.js\n");
    verification(&f.verify(&p), false);
}

#[test]
fn an_explicit_empty_context_selector_cannot_fall_back_to_a_request_file() {
    let mut f = Fixture::javascript();
    f.env.insert("AMPLIHACK_CONTEXT_FILE".into(), "".into());
    f.call("restore", &f.request()).error("INVALID_CONTEXT");
}

#[test]
fn boolean_report_schema_versions_cannot_establish_verification() {
    let (mut f, p, _) = captured_js();
    f.context["precommit_results"]["schema_version"] = json!(true);
    verification(&f.verify(&p), false);
}

#[test]
fn final_status_reads_the_restored_cohort_through_its_legacy_adapter() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    for key in [
        "implementation_terminal_evidence",
        "verification_terminal_evidence",
        "implementation",
        "verdict_json",
        "precommit_results",
        "local_testing_gate",
    ] {
        f.context.as_object_mut().unwrap().remove(key);
    }
    let r = f.finalize("final-status");
    r.exit(0);
    assert!(String::from_utf8_lossy(&r.0.stdout).contains("terminal_success=true"));
}

#[test]
fn final_status_preserves_provider_failure_without_local_evidence() {
    let mut f = Fixture::javascript();
    f.context["pr_publish_result"] =
        json!({"state":"BLOCKED_PROVIDER","message":"publication failed"});
    let r = f.finalize("final-status");
    r.exit(1);
    assert!(String::from_utf8_lossy(&r.0.stdout).contains("terminal_state=BLOCKED_PROVIDER"));
}

#[test]
fn final_status_publication_claim_cannot_bypass_original_local_evidence() {
    let mut f = Fixture::javascript();
    f.context["pr_publish_result"] =
        json!({"state":"FOLLOWUP_CREATED","pr_url":"https://example.invalid/pull/7"});
    let r = f.finalize("final-status");
    r.exit(1);
    let output = String::from_utf8_lossy(&r.0.stdout);
    assert!(output.contains("terminal_state=FAILED_IMPLEMENTATION"));
    assert!(output.contains("terminal_success=false"));
    assert!(output.contains("publish_state_reached=true"));
    assert!(!output.contains("All 23 workflow steps completed successfully."));
}

#[test]
fn a_failed_classifier_process_cannot_return_a_successful_record() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    let tools = f.tmp.path().join("framework/amplifier-bundle/tools");
    fs::create_dir_all(&tools).unwrap();
    write(tools.join("workflow_agentic_finalization.sh"), b"#!/bin/sh\nprintf '%s\\n' '{\"terminal_state\":\"IMPLEMENTED_VERIFIED\",\"terminal_success\":\"true\",\"terminal_failure\":\"false\"}'\nexit 7\n");
    f.env.insert(
        "AMPLIHACK_HOME".into(),
        f.tmp.path().join("framework").display().to_string(),
    );
    let r = f.finalize("validate");
    r.exit(1);
    assert_eq!(r.json()["terminal_state"], "FAILED_INVALID_EVIDENCE");
    assert_eq!(r.json()["terminal_success"], "false");
}

// Step 11b review counterexamples. Each passes through the production CLI;
// positive counterparts execute native assertions and the stock producer.
fn omit_local_transport(f: &mut Fixture) {
    for key in [
        "implementation_terminal_evidence",
        "implementation",
        "verdict_json",
        "verification_terminal_evidence",
        "precommit_results",
        "local_testing_gate",
    ] {
        f.context.as_object_mut().unwrap().remove(key);
    }
}

#[test]
fn review_stock_verifier_output_requires_observed_execution() {
    let mut f = Fixture::javascript();
    f.context["implementation"] =
        json!("Files modified: contract.js\nDiff summary: inspected decoder");
    f.context["verdict_json"] = json!(
        "Investigation prose.\n{\"verdict\":\"WORK_VERIFIED\",\"evidence\":[\"contract.js\"],\"rationale\":\"inspected source\"}\n"
    );
    f.save_context();
    let producer = Run(f
        .command("bash")
        .env_remove("AMPLIHACK_CONTEXT_FILE")
        .env(
            "IMPLEMENTATION",
            f.context["implementation"].as_str().unwrap(),
        )
        .env("VERDICT_JSON", f.context["verdict_json"].as_str().unwrap())
        .arg(tool("workflow_implementation_evidence.sh"))
        .output()
        .unwrap());
    producer.exit(0);
    assert_eq!(producer.json()["implementation_completed"], "true");
    f.context["implementation_terminal_evidence"] = producer.json();
    let checkpoint = f.call("checkpoint-implementation", &f.request());
    checkpoint.exit(1);
    assert_eq!(checkpoint.json()["implementation_state"], "INVALID");
    f.install_original_implementation();
    assert_eq!(f.checkpoint()["implementation_state"], "VALID");
    assert!(
        f.context["verdict_json"]
            .as_str()
            .unwrap()
            .contains("Inspected and executed concrete decoder assertions.")
    );
    assert!(
        !f.context["verdict_json"]
            .as_str()
            .unwrap()
            .contains("\"artifacts\"")
    );
    assert!(
        !f.context["verdict_json"]
            .as_str()
            .unwrap()
            .contains("\"head\"")
    );
}

#[test]
fn review_prose_edits_cannot_refresh_old_verifier_or_revive_invalid_generation() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    f.file("added.js", "throw new Error('unverified source');\n");
    f.context["implementation"] = json!(format!(
        "{}\n",
        f.context["implementation"].as_str().unwrap()
    ));
    let checkpoint = f.call("checkpoint-implementation", &f.request());
    checkpoint.exit(1);
    omit_local_transport(&mut f);
    let restore = f.restore();
    restore.exit(1);
    assert_eq!(restore.json()["implementation_valid"], false);
    let finalization = f.finalize("final-status");
    finalization.exit(1);
    assert!(String::from_utf8_lossy(&finalization.0.stdout).contains("terminal_success=false"));
}

#[test]
fn review_latest_failed_verification_remains_authoritative_after_omission() {
    let mut f = Fixture::javascript();
    let plan = f.verified_cohort();
    f.context["precommit_results"]["status"] = json!("FAIL");
    f.context["verification_terminal_evidence"] = verification(&f.verify(&plan), false);
    omit_local_transport(&mut f);
    let restored = f.restore();
    restored.exit(1);
    assert_eq!(restored.json()["implementation_valid"], true);
    assert_eq!(restored.json()["verification_valid"], false);
    assert_eq!(
        f.derived(&restored.json())["precommit_results"]["status"],
        "FAIL"
    );
    f.finalize("final-status").exit(1);
    let collected = f.finalize("collect");
    collected.exit(0);
    assert_eq!(
        collected.json()["completion"]["verification_completed"],
        "false"
    );
    f.context["finalization_evidence"] = collected.json();
    f.finalize("validate").exit(1);
    f.finalize("complete").exit(1);
}

#[test]
fn review_no_request_recipe_preserves_original_run_id() {
    let mut f = Fixture::javascript();
    let plan = f.verified_cohort();
    let result = f.run_raw(&["verify"], b"");
    verification(&result, true);
    let restored = f.run_raw(&["restore"], b"");
    restored.exit(0);
    let cohort: Value = serde_json::from_slice(&f.artifact(&f.checkpoint()["cohort"])).unwrap();
    assert_eq!(
        cohort["identity"]["workflow"]["original_run_id"],
        "fixture-original-run"
    );
    assert_eq!(plan["status"], "READY");
    f.context["original_run_id"] = json!("another-run");
    f.restore().error("IDENTITY_MISMATCH");
}

#[test]
fn review_canonical_empty_defaults_reach_the_authorised_adapter() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    let mut command = f.command("bash");
    command
        .env_remove("AMPLIHACK_CONTEXT_FILE")
        .env("RECIPE_VAR_repo_path", &f.repo)
        .env(
            "RECIPE_VAR_worktree_setup",
            f.context["worktree_setup"].to_string(),
        );
    for key in [
        "implementation_terminal_evidence",
        "verdict_json",
        "precommit_results",
        "local_testing_gate",
    ] {
        command.env(format!("RECIPE_VAR_{key}"), f.context[key].to_string());
    }
    command.env(
        "RECIPE_VAR_implementation",
        f.context["implementation"].as_str().unwrap(),
    );
    for key in [
        "verification_terminal_evidence",
        "finalization_evidence",
        "workflow_result",
    ] {
        command.env(format!("RECIPE_VAR_{key}"), "");
    }
    let r = Run(command
        .arg(tool("workflow_terminal_evidence.sh"))
        .arg("checkpoint-implementation")
        .output()
        .unwrap());
    r.exit(0);
    assert_eq!(r.json()["implementation_state"], "VALID");
    // Selected file authority does not inherit a conflicting canonical alias.
    f.env.insert(
        "RECIPE_VAR_verification_terminal_evidence".into(),
        "malformed".into(),
    );
    f.restore().exit(0);
    f.context["verification_terminal_evidence"] = json!("malformed");
    f.restore().exit(1);
}

#[test]
fn review_unrelated_noops_cannot_satisfy_two_consumer_scenarios() {
    let mut f = Fixture::javascript();
    for index in [2, 3] {
        f.plan["obligations"][index]["argv"] = json!(["node", "-e", "void(0)"]);
    }
    let plan = f.register();
    f.execute_all(&plan);
    verification(&f.verify(&plan), false);
}

#[test]
fn review_zero_case_and_skipped_scenarios_do_not_establish_execution() {
    for skipped in [false, true] {
        let mut f = Fixture::javascript();
        let source = if skipped {
            "import {test} from 'node:test'; import assert from 'node:assert'; test.skip('basic',()=>assert.equal(2,2)); test.skip('edge',()=>assert.equal(0,0));\n"
        } else {
            "// basic edge assert references cannot replace execution\n"
        };
        f.file("consumer.js", source);
        f.commit("unexecuted assertions");
        f.plan = f.node_plan("javascript");
        for index in [2, 3] {
            let case = if index == 2 { "basic" } else { "edge" };
            f.plan["obligations"][index]["argv"] = json!([
                "node",
                "--test",
                format!("--test-name-pattern={case}"),
                "consumer.js"
            ]);
        }
        let plan = f.register();
        f.execute_all(&plan);
        verification(&f.verify(&plan), false);
    }
}

#[test]
fn review_configured_failing_git_hook_cannot_be_replaced_by_noop() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::javascript();
    f.file(
        ".git/hooks/pre-commit",
        "#!/bin/sh\necho required-hook-failed >&2\nexit 71\n",
    );
    fs::set_permissions(
        f.repo.join(".git/hooks/pre-commit"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    f.plan["exclusions"] = json!([]);
    let hook = f.obligation(
        "hooks",
        "precommit",
        "javascript",
        "node",
        &["node", "-e", "void(0)"],
    );
    f.plan["obligations"].as_array_mut().unwrap().push(hook);
    let plan = f.register();
    f.execute_all(&plan);
    f.context["precommit_results"]["exclusions"] = json!([]);
    verification(&f.verify(&plan), false);
    let actual = Run(f
        .command("git")
        .args(["hook", "run", "pre-commit"])
        .output()
        .unwrap());
    actual.exit(71);
}

#[test]
fn review_repository_substitution_is_rejected_even_at_equal_head() {
    for distinct_head in [false, true] {
        let mut f = Fixture::javascript();
        f.verified_cohort();
        let clone = f.tmp.path().join("other");
        f.native(
            "git",
            &[
                "clone",
                "--no-hardlinks",
                f.repo.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        if distinct_head {
            let r = Run(f
                .command("git")
                .arg("-C")
                .arg(&clone)
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "different head",
                ])
                .output()
                .unwrap());
            r.exit(0);
        }
        f.context.as_object_mut().unwrap().remove("worktree_setup");
        f.context["repo_path"] = json!(clone);
        f.finalize("collect").error("IDENTITY_MISMATCH");
    }
}

#[test]
fn review_collected_repository_head_is_checked_before_completion() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    let collected = f.finalize("collect");
    collected.exit(0);
    f.context["finalization_evidence"] = collected.json();
    f.context["finalization_evidence"]["git"]["head_sha"] = json!("0".repeat(40));
    f.finalize("validate").error("IDENTITY_MISMATCH");
    f.finalize("complete").error("IDENTITY_MISMATCH");
    f.finalize("final-status").error("IDENTITY_MISMATCH");
}

#[test]
fn review_negative_ci_cannot_be_hidden_by_positive_terminal_metadata() {
    for state in ["FAILURE", "FAILED", "CANCELLED", "ERROR"] {
        let mut f = Fixture::javascript();
        f.verified_cohort();
        let collected = f.finalize("collect");
        collected.exit(0);
        f.context["finalization_evidence"] = collected.json();
        f.context["finalization_evidence"]["ci"]["state"] = json!(state);
        f.context["terminal_state"] =
            json!({"terminal_state":"EXISTING_OPEN_PR","terminal_success":"true"});
        let result = f.finalize("validate");
        result.exit(1);
        assert_eq!(result.json()["terminal_success"], "false");
        assert_eq!(result.json()["terminal_state"], "BLOCKED_CI");
        f.finalize("final-status").exit(1);
        f.finalize("complete").exit(1);
    }
}

#[test]
fn review_internal_symlink_target_mutation_invalidates_execution() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::javascript();
    fs::rename(
        f.repo.join("consumer.js"),
        f.repo.join("consumer-target.js"),
    )
    .unwrap();
    symlink("consumer-target.js", f.repo.join("consumer.js")).unwrap();
    f.commit("internal executable symlink");
    f.plan = f.node_plan("javascript");
    let plan = f.register();
    f.execute_all(&plan);
    verification(&f.verify(&plan), true);
    f.file(
        "consumer-target.js",
        "// basic edge assert but no execution\n",
    );
    verification(&f.verify(&plan), false);
}

#[test]
fn review_unauthorised_external_symlink_target_is_unproven() {
    use std::os::unix::fs::symlink;
    let f = Fixture::javascript();
    fs::rename(f.repo.join("consumer.js"), f.tmp.path().join("external.js")).unwrap();
    symlink(f.tmp.path().join("external.js"), f.repo.join("consumer.js")).unwrap();
    f.call("plan", &f.plan).error("SOURCE_UNPROVEN");
}

#[test]
fn review_artifact_inspection_streams_under_a_fixed_memory_limit() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let mut f = Fixture::javascript();
    f.file(
        "large-log.py",
        "import sys\nfor _ in range(64): sys.stdout.buffer.write(b'x'*(1024*1024))\n",
    );
    let mut package: Value =
        serde_json::from_slice(&fs::read(f.repo.join("package.json")).unwrap()).unwrap();
    package["scripts"]["quality"] = json!("python3 large-log.py");
    f.file("package.json", serde_json::to_vec(&package).unwrap());
    f.commit("bounded large raw output");
    f.plan = f.node_plan("javascript");
    f.plan["obligations"][1]["argv"] = json!(["python3", "large-log.py"]);
    let plan = f.register();
    f.execute_all(&plan);
    f.save_context();
    let mut request = f.request();
    request["plan_id"] = plan["plan_id"].clone();
    let request_path = f.tmp.path().join("bounded-request.json");
    write(&request_path, serde_json::to_vec(&request).unwrap());
    let r = Run(f
        .command("bash")
        .args([
            "-c",
            "ulimit -v 98304; exec bash \"$1\" verify --request-file \"$2\"",
            "memory-bound",
        ])
        .arg(tool("workflow_terminal_evidence.sh"))
        .arg(request_path)
        .output()
        .unwrap());
    verification(&r, true);
}

#[test]
fn review_log_patterns_cross_chunk_boundaries_without_truncation() {
    let mut f = Fixture::javascript();
    let source = fs::read_to_string(f.repo.join("consumer.js")).unwrap();
    f.file(
        "consumer.js",
        source.replace(
            "console.log(role",
            "process.stdout.write('x'.repeat(65526)); console.log(role",
        ),
    );
    f.commit("boundary-spanning consumer completion");
    f.plan = f.node_plan("javascript");
    let plan = f.register();
    f.execute_all(&plan);
    verification(&f.verify(&plan), true);
}

#[test]
fn review_documented_python_suite_cannot_be_replaced_with_pass() {
    let mut f = Fixture::javascript();
    fs::remove_file(f.repo.join("package.json")).unwrap();
    f.file(
        "pyproject.toml",
        "[project]\nname='contract'\nversion='0.1.0'\n",
    );
    f.file("test_contract.py", "import unittest\nclass Contract(unittest.TestCase):\n def test_fail(self): self.fail('configured suite failed')\n");
    f.file("README.md", "Test: python3 -m unittest test_contract. Consumers use the inspected consumer.js basic and edge cases. No hooks configured.\n");
    f.commit("documented failing Python suite");
    f.plan["components"][0]["toolchain"] = json!("python");
    f.plan["components"][0]["inspected"] =
        json!([f.reference("pyproject.toml"), f.reference("README.md")]);
    f.plan["obligations"].as_array_mut().unwrap().remove(1);
    f.plan["obligations"][0] = f.obligation(
        "tests",
        "test",
        "python",
        "python",
        &["python3", "-c", "pass"],
    );
    for index in [1, 2] {
        f.plan["obligations"][index]["selected_from"] = json!([f.reference("README.md")]);
        f.plan["obligations"][index]["scenario"]["inspected"] = json!([f.reference("README.md")]);
    }
    f.plan["exclusions"][0]["policy_refs"] = json!([f.reference("README.md")]);
    let plan = f.register();
    f.execute_all(&plan);
    verification(&f.verify(&plan), false);
    let failed = Run(f
        .command("python3")
        .args(["-m", "unittest", "test_contract"])
        .output()
        .unwrap());
    failed.exit(1);
}

#[test]
fn review_git_hooks_path_and_xpia_exclusion_preserve_other_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::javascript();
    let hooks = f.repo.join(".git/custom-hooks");
    f.file(
        ".git/custom-hooks/pre-commit",
        "#!/bin/sh\nnode --check contract.js\n",
    );
    fs::set_permissions(hooks.join("pre-commit"), fs::Permissions::from_mode(0o700)).unwrap();
    f.native(
        "git",
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    f.env.insert("SKIP".into(), "xpia".into());
    f.plan["exclusions"] = json!([]);
    let hook = f.obligation(
        "hooks",
        "precommit",
        "javascript",
        "git",
        &["git", "hook", "run", "pre-commit"],
    );
    f.plan["obligations"].as_array_mut().unwrap().push(hook);
    let plan = f.register();
    f.execute_all(&plan);
    f.context["precommit_results"]["exclusions"] = json!([]);
    verification(&f.verify(&plan), true);
    f.env.insert("SKIP".into(), "xpia,required-check".into());
    verification(&f.verify(&plan), false);
}

#[test]
fn review_stock_terminal_output_can_remain_a_recipe_string() {
    let mut f = Fixture::javascript();
    f.install_original_implementation();
    let original = f.context["implementation_terminal_evidence"].to_string() + "\n";
    f.context["implementation_terminal_evidence"] = json!(original);
    let checkpoint = f.checkpoint();
    let cohort: Value = serde_json::from_slice(&f.artifact(&checkpoint["cohort"])).unwrap();
    assert_eq!(
        cohort["implementation"]["records"]["implementation_terminal_evidence"],
        original
    );
}

#[test]
fn review_all_declared_empty_object_defaults_are_absent_before_validation() {
    let f = Fixture::javascript();
    let mut command = f.command("bash");
    command
        .env_remove("AMPLIHACK_CONTEXT_FILE")
        .env("RECIPE_VAR_repo_path", &f.repo);
    for key in [
        "worktree_setup",
        "implementation",
        "implementation_terminal_evidence",
        "verification_terminal_evidence",
        "finalization_evidence",
        "workflow_result",
        "finalizer_step_status",
        "terminal_state",
        "pr_publish_result",
        "publish_terminal_evidence",
    ] {
        command.env(format!("RECIPE_VAR_{key}"), "");
    }
    let result = Run(command
        .arg(tool("workflow_terminal_evidence.sh"))
        .arg("checkpoint-implementation")
        .output()
        .unwrap());
    result.exit(1);
    assert_eq!(result.json()["implementation_state"], "MISSING");
}

#[test]
fn review_workflow_name_conflict_cannot_select_a_new_cohort() {
    let mut f = Fixture::javascript();
    f.install_original_implementation();
    let mut request = f.request();
    request["workflow"]["name"] = json!("renamed-default-workflow");
    f.call("checkpoint-implementation", &request)
        .error("IDENTITY_MISMATCH");
    f.env
        .insert("AMPLIHACK_ORIGINAL_RUN_ID".into(), "other-original".into());
    f.restore().error("IDENTITY_MISMATCH");
}

#[test]
fn review_large_spilled_context_preserves_selected_file_authority() {
    let mut f = Fixture::javascript();
    f.verified_cohort();
    f.context["unrelated"]["large_retained_payload"] = json!("retained bytes ".repeat(32768));
    f.env.insert(
        "RECIPE_VAR_implementation_terminal_evidence".into(),
        "malformed unselected alias".into(),
    );
    f.env.insert(
        "RECIPE_VAR_precommit_results".into(),
        "malformed unselected alias".into(),
    );
    f.save_context();
    let before = fs::read(f.context_path()).unwrap();
    assert!(before.len() > 256 * 1024);
    let restored = f.restore();
    restored.exit(0);
    let derived = f.derived(&restored.json());
    assert_eq!(derived["unrelated"], f.context["unrelated"]);
    assert_eq!(fs::read(f.context_path()).unwrap(), before);
    f.finalize("final-status").exit(0);
}

#[test]
fn review_failed_verification_requires_new_execution_not_positive_replay() {
    let mut f = Fixture::javascript();
    let plan = f.verified_cohort();
    let positive = f.context.clone();
    f.context["precommit_results"]["status"] = json!("FAIL");
    verification(&f.verify(&plan), false);
    f.context = positive;
    verification(&f.verify(&plan), false);
    f.execute_all(&plan);
    verification(&f.verify(&plan), true);
}

#[test]
fn review_invalid_implementation_requires_new_producer_not_positive_replay() {
    let mut f = Fixture::javascript();
    f.install_original_implementation();
    let positive = f.context.clone();
    f.context["implementation_terminal_evidence"] = json!(null);
    f.call("checkpoint-implementation", &f.request()).exit(1);
    f.context = positive;
    f.call("checkpoint-implementation", &f.request()).exit(1);
    f.install_original_implementation();
    f.checkpoint();
}

#[test]
fn review_latest_failed_capture_cannot_be_omitted_from_a_positive_report() {
    let mut f = Fixture::javascript();
    let flag = f.tmp.path().join("failure-flag");
    let source = fs::read_to_string(f.repo.join("contract.test.js")).unwrap();
    f.file("contract.test.js", format!("{source}\nimport fs from 'node:fs'; test('current execution state',()=>assert.equal(fs.existsSync({}),false));\n", serde_json::to_string(&flag).unwrap()));
    f.commit("observable later failed execution");
    f.plan = f.node_plan("javascript");
    let plan = f.verified_cohort();
    write(&flag, b"fail the next real test execution");
    let failed = f.call("capture", &f.capture_request(&plan, "tests"));
    failed.exit(1);
    assert_eq!(failed.json()["process_exit_code"], 1);
    verification(&f.verify(&plan), false);
    fs::remove_file(flag).unwrap();
    f.execute_all(&plan);
    verification(&f.verify(&plan), true);
}

#[test]
fn review_no_request_retains_the_original_custom_workflow_name() {
    let f = Fixture::javascript();
    let mut request = f.request();
    request["workflow"]["name"] = json!("custom-workflow");
    let initial = f.call("checkpoint-implementation", &request);
    initial.exit(1);
    let cohort: Value = serde_json::from_slice(&f.artifact(&initial.json()["cohort"])).unwrap();
    assert_eq!(cohort["identity"]["workflow"]["name"], "custom-workflow");
    let restored = f.run_raw(&["restore"], b"");
    restored.exit(1);
    assert!(
        restored.json().get("error").is_none(),
        "{}",
        restored.describe()
    );
    assert_eq!(
        restored.json()["generation_id"],
        initial.json()["generation_id"]
    );
    verification(&f.run_raw(&["verify"], b""), false);
}
