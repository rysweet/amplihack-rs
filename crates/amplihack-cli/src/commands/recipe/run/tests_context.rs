use super::*;
use std::collections::BTreeMap;

// -------------------------------------------------------------------------
// parse_context_args — key=value parsing
// -------------------------------------------------------------------------

/// Valid single key=value pair must be parsed without errors.
#[test]
fn test_parse_context_args_valid_single_pair() {
    let args = vec!["task_description=hello world".to_string()];
    let (ctx, errs) = parse_context_args(&args);
    assert!(errs.is_empty(), "No errors expected. Got: {errs:?}");
    assert_eq!(
        ctx.get("task_description").map(String::as_str),
        Some("hello world")
    );
}

/// Multiple valid key=value pairs must all be parsed correctly.
#[test]
fn test_parse_context_args_multiple_pairs() {
    let args = vec![
        "foo=bar".to_string(),
        "baz=qux".to_string(),
        "repo_path=/tmp".to_string(),
    ];
    let (ctx, errs) = parse_context_args(&args);
    assert!(errs.is_empty(), "No errors expected. Got: {errs:?}");
    assert_eq!(ctx.len(), 3);
    assert_eq!(ctx.get("foo").map(String::as_str), Some("bar"));
    assert_eq!(ctx.get("baz").map(String::as_str), Some("qux"));
    assert_eq!(ctx.get("repo_path").map(String::as_str), Some("/tmp"));
}

/// Empty context args must produce an empty map with no errors.
#[test]
fn test_parse_context_args_empty_input() {
    let (ctx, errs) = parse_context_args(&[]);
    assert!(errs.is_empty(), "Empty input must produce no errors");
    assert!(ctx.is_empty(), "Empty input must produce empty context map");
}

/// An arg without '=' must produce an error with a helpful message.
#[test]
fn test_parse_context_args_invalid_no_equals_sign() {
    let args = vec!["no-equals-sign".to_string()];
    let (ctx, errs) = parse_context_args(&args);
    assert_eq!(errs.len(), 1, "Exactly one error expected. Got: {errs:?}");
    assert!(
        errs[0].contains("key=value"),
        "Error message must mention 'key=value' format. Got: {}",
        errs[0]
    );
    assert!(ctx.is_empty(), "No context should be parsed on error");
}

/// A value that itself contains '=' must be preserved correctly.
/// The split must only happen on the FIRST '='.
#[test]
fn test_parse_context_args_value_contains_equals() {
    let args = vec!["url=https://example.com?a=1&b=2".to_string()];
    let (ctx, errs) = parse_context_args(&args);
    assert!(errs.is_empty(), "No errors expected. Got: {errs:?}");
    assert_eq!(
        ctx.get("url").map(String::as_str),
        Some("https://example.com?a=1&b=2"),
        "Value with embedded '=' must not be truncated"
    );
}

// -------------------------------------------------------------------------
// infer_missing_context — env var inference and default merging
// -------------------------------------------------------------------------

/// User-provided context must override recipe defaults.
#[test]
fn test_infer_missing_context_user_values_override_recipe_defaults() {
    let mut recipe_defaults = BTreeMap::new();
    recipe_defaults.insert(
        "task_description".to_string(),
        serde_yaml::Value::String("recipe default".to_string()),
    );

    let mut user_context = BTreeMap::new();
    user_context.insert("task_description".to_string(), "user override".to_string());

    let (merged, inferred) = infer_missing_context(&recipe_defaults, &user_context);

    assert_eq!(
        merged.get("task_description").map(String::as_str),
        Some("user override"),
        "User context must override recipe defaults"
    );
    assert!(
        inferred.is_empty(),
        "No inference should occur when user provides the value. Got: {inferred:?}"
    );
}

/// When task_description is missing from user context and AMPLIHACK_TASK_DESCRIPTION
/// env var is set, it must be inferred automatically.
#[test]
fn test_infer_missing_context_infers_task_description_from_env() {
    // SAFETY: test-only env manipulation
    unsafe { std::env::set_var("AMPLIHACK_TASK_DESCRIPTION", "from env var") };

    let mut recipe_defaults = BTreeMap::new();
    recipe_defaults.insert(
        "task_description".to_string(),
        serde_yaml::Value::String(String::new()), // empty default
    );

    let (merged, inferred) = infer_missing_context(&recipe_defaults, &BTreeMap::new());

    unsafe { std::env::remove_var("AMPLIHACK_TASK_DESCRIPTION") };

    assert_eq!(
        merged.get("task_description").map(String::as_str),
        Some("from env var"),
        "task_description must be inferred from AMPLIHACK_TASK_DESCRIPTION"
    );
    assert!(
        inferred.iter().any(|s| s.contains("task_description")),
        "Inferred list must mention task_description. Got: {inferred:?}"
    );
}

/// When repo_path is a required context key, it defaults to "." if no env var is set.
#[test]
fn test_infer_missing_context_repo_path_defaults_to_dot() {
    // Ensure no override is present
    unsafe { std::env::remove_var("AMPLIHACK_REPO_PATH") };

    let mut recipe_defaults = BTreeMap::new();
    recipe_defaults.insert(
        "repo_path".to_string(),
        serde_yaml::Value::String(String::new()),
    );

    let (merged, _inferred) = infer_missing_context(&recipe_defaults, &BTreeMap::new());

    assert_eq!(
        merged.get("repo_path").map(String::as_str),
        Some("."),
        "repo_path must default to '.' when AMPLIHACK_REPO_PATH is not set"
    );
}

/// Issue #1567: a recipe context key cannot fake or override the pinned Claude
/// login profile, in any letter case, because AMPLIHACK_* names are reserved.
#[test]
fn test_context_env_pairs_drops_claude_profile_keys() {
    let mut context = BTreeMap::new();
    context.insert("AMPLIHACK_CLAUDE_PROFILE".to_string(), "evil".to_string());
    context.insert("amplihack_claude_profile".to_string(), "evil".to_string());
    context.insert(
        "AMPLIHACK_CLAUDE_PROFILE_ROOT".to_string(),
        "/tmp/evil".to_string(),
    );
    context.insert("task_description".to_string(), "kept".to_string());

    let pairs = super::execute::context_env_pairs(&context, usize::MAX);

    assert!(
        pairs
            .iter()
            .all(|(k, _)| !k.starts_with("AMPLIHACK_CLAUDE_PROFILE")),
        "profile keys must never be exported from recipe context: {pairs:?}"
    );
    assert!(
        pairs
            .iter()
            .any(|(k, v)| k == "TASK_DESCRIPTION" && v == "kept")
    );
}

// -------------------------------------------------------------------------
// Issue #1570: recipe context from files (`--context-file`,
// `--context-from-file`), merged by `merge_context_sources`.
//
// Contract assumed by these tests (docs/reference/large-payload-delivery.md,
// "Recipe context from files"):
//
//   // crates/amplihack-cli/src/commands/recipe/run/context_sources.rs
//   pub(super) fn merge_context_sources(
//       context_file: Option<&std::path::Path>,
//       context_from_file: &[String],               // raw `KEY=PATH` args
//       cli_context: &BTreeMap<String, String>,     // parsed `-c` pairs
//   ) -> anyhow::Result<BTreeMap<String, String>>;
//
// Precedence, lowest to highest: `--context-file`, `--context-from-file`,
// `-c`. `infer_missing_context` runs on the result afterwards. Errors are
// fixed text that names the key, path or argument and never the contents.
//
//   RecipeCommands::Run {
//       ..,
//       context_file: Option<String>,       // `--context-file <PATH>`, once
//       context_from_file: Vec<String>,     // `--context-from-file KEY=PATH`, repeatable
//   }
// -------------------------------------------------------------------------

mod issue_1570_context_sources {
    use super::super::context_sources::merge_context_sources;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// Text placed in every rejected file; no error message may contain it.
    const SENTINEL: &str = "SENTINEL_1570_FILE_CONTENTS_MUST_NOT_LEAK";

    const SIXTEEN_MIB: u64 = 16 * 1024 * 1024;

    fn write(dir: &Path, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write fixture");
        path
    }

    fn from_file_arg(key: &str, path: &Path) -> String {
        format!("{key}={}", path.display())
    }

    fn merge_err(
        context_file: Option<&Path>,
        context_from_file: &[String],
        cli: &BTreeMap<String, String>,
    ) -> String {
        let err = merge_context_sources(context_file, context_from_file, cli)
            .expect_err("merge_context_sources must reject this input");
        let rendered = format!("{err:#}");
        let debug = format!("{err:?}");
        assert!(
            !rendered.contains(SENTINEL) && !debug.contains(SENTINEL),
            "error message must never contain file contents: {rendered}"
        );
        rendered
    }

    fn big_value() -> String {
        // 200 KiB with multi-byte characters, newlines, quotes and shell
        // metacharacters so any re-encoding or truncation shows up.
        let unit = "line é 'q' \"dq\" $(touch /tmp/pwned) `x` \\ ;\n";
        let mut value = String::new();
        while value.len() < 200 * 1024 {
            value.push_str(unit);
        }
        value
    }

    #[test]
    fn precedence_is_context_file_then_context_from_file_then_dash_c() {
        let dir = tempfile::tempdir().unwrap();
        let json = write(
            dir.path(),
            "ctx.json",
            r#"{"a":"from_json","b":"from_json","c":"from_json"}"#,
        );
        let b = write(dir.path(), "b.txt", "from_file");
        let c = write(dir.path(), "c.txt", "from_file");
        let mut cli = BTreeMap::new();
        cli.insert("c".to_string(), "from_cli".to_string());
        cli.insert("d".to_string(), "from_cli".to_string());

        let merged = merge_context_sources(
            Some(&json),
            &[from_file_arg("b", &b), from_file_arg("c", &c)],
            &cli,
        )
        .expect("merge must succeed");

        assert_eq!(merged.get("a").map(String::as_str), Some("from_json"));
        assert_eq!(merged.get("b").map(String::as_str), Some("from_file"));
        assert_eq!(merged.get("c").map(String::as_str), Some("from_cli"));
        assert_eq!(merged.get("d").map(String::as_str), Some("from_cli"));
        assert_eq!(merged.len(), 4, "{merged:?}");
    }

    #[test]
    fn no_file_sources_returns_dash_c_pairs_unchanged() {
        let mut cli = BTreeMap::new();
        cli.insert("repo_path".to_string(), ".".to_string());
        let merged = merge_context_sources(None, &[], &cli).unwrap();
        assert_eq!(merged, cli);
    }

    #[test]
    fn context_file_json_values_are_converted_to_context_strings() {
        let dir = tempfile::tempdir().unwrap();
        let json = write(
            dir.path(),
            "ctx.json",
            r#"{
              "s": "plain text",
              "n": null,
              "t": true,
              "f": false,
              "i": 3,
              "r": 0.85,
              "arr": ["security", "reliability"],
              "obj": {"branch": "fix/1570"},
              "worktree_setup": {"worktree_path": "/w", "branch_name": "fix/1570", "created": false}
            }"#,
        );
        let merged = merge_context_sources(Some(&json), &[], &BTreeMap::new()).unwrap();
        let get = |k: &str| merged.get(k).map(String::as_str);
        assert_eq!(get("s"), Some("plain text"));
        assert_eq!(get("n"), Some(""));
        assert_eq!(get("t"), Some("true"));
        assert_eq!(get("f"), Some("false"));
        assert_eq!(get("i"), Some("3"));
        assert_eq!(get("r"), Some("0.85"));
        assert_eq!(get("arr"), Some(r#"["security","reliability"]"#));
        assert_eq!(get("obj"), Some(r#"{"branch":"fix/1570"}"#));

        let setup = get("worktree_setup").expect("worktree_setup present");
        assert!(
            !setup.contains('\n') && !setup.contains(": ") && !setup.contains(", "),
            "objects must be compact JSON, got {setup:?}"
        );
        let parsed: serde_json::Value = serde_json::from_str(setup).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({"worktree_path": "/w", "branch_name": "fix/1570", "created": false})
        );
    }

    #[test]
    fn context_from_file_value_is_raw_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "v.txt", "line1\n  line2  \n\n");
        let merged =
            merge_context_sources(None, &[from_file_arg("v", &path)], &BTreeMap::new()).unwrap();
        assert_eq!(
            merged.get("v").map(String::as_str),
            Some("line1\n  line2  \n\n"),
            "contents must not be trimmed or parsed"
        );
    }

    #[test]
    fn context_from_file_reads_200_kib_value_byte_exact() {
        let dir = tempfile::tempdir().unwrap();
        let value = big_value();
        let path = write(dir.path(), "big.txt", &value);
        let merged =
            merge_context_sources(None, &[from_file_arg("big", &path)], &BTreeMap::new()).unwrap();
        assert_eq!(merged.get("big").map(String::len), Some(value.len()));
        assert!(
            merged.get("big") == Some(&value),
            "value must be byte-exact"
        );
    }

    #[test]
    fn context_file_reads_200_kib_value_byte_exact() {
        let dir = tempfile::tempdir().unwrap();
        let value = big_value();
        let json = write(
            dir.path(),
            "ctx.json",
            serde_json::to_vec(&serde_json::json!({ "task_description": value })).unwrap(),
        );
        let merged = merge_context_sources(Some(&json), &[], &BTreeMap::new()).unwrap();
        assert!(
            merged.get("task_description") == Some(&value),
            "value must be byte-exact"
        );
    }

    #[test]
    fn context_from_file_accepts_a_file_of_exactly_16_mib() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "max.txt", vec![b'a'; SIXTEEN_MIB as usize]);
        let merged =
            merge_context_sources(None, &[from_file_arg("max", &path)], &BTreeMap::new()).unwrap();
        assert_eq!(
            merged.get("max").map(String::len),
            Some(SIXTEEN_MIB as usize)
        );
    }

    // ---- errors -----------------------------------------------------------

    #[test]
    fn context_file_top_level_array_is_rejected_with_fixed_message() {
        let dir = tempfile::tempdir().unwrap();
        let json = write(dir.path(), "ctx.json", format!(r#"["{SENTINEL}"]"#));
        let err = merge_err(Some(&json), &[], &BTreeMap::new());
        let expected = format!(
            "--context-file {}: top-level JSON value must be an object",
            json.display()
        );
        assert!(err.contains(&expected), "expected {expected:?} in {err:?}");
    }

    #[test]
    fn context_file_top_level_scalar_is_rejected_with_fixed_message() {
        let dir = tempfile::tempdir().unwrap();
        let json = write(dir.path(), "ctx.json", format!(r#""{SENTINEL}""#));
        let err = merge_err(Some(&json), &[], &BTreeMap::new());
        let expected = format!(
            "--context-file {}: top-level JSON value must be an object",
            json.display()
        );
        assert!(err.contains(&expected), "expected {expected:?} in {err:?}");
    }

    #[test]
    fn context_from_file_without_equals_is_rejected_showing_the_argument() {
        let err = merge_err(None, &["task_description".to_string()], &BTreeMap::new());
        assert!(
            err.contains("--context-from-file: expected KEY=PATH"),
            "fixed message missing: {err:?}"
        );
        assert!(
            err.contains("task_description"),
            "argument missing: {err:?}"
        );
    }

    #[test]
    fn context_from_file_reserved_amplihack_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "v.txt", SENTINEL);
        let err = merge_err(
            None,
            &[from_file_arg("AMPLIHACK_HOME", &path)],
            &BTreeMap::new(),
        );
        assert!(
            err.contains("--context-from-file: key 'AMPLIHACK_HOME' is reserved"),
            "fixed message missing: {err:?}"
        );
    }

    #[test]
    fn context_file_reserved_amplihack_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let json = write(
            dir.path(),
            "ctx.json",
            format!(r#"{{"AMPLIHACK_CLAUDE_PROFILE":"{SENTINEL}"}}"#),
        );
        let err = merge_err(Some(&json), &[], &BTreeMap::new());
        assert!(err.contains("--context-file"), "source missing: {err:?}");
        assert!(
            err.contains("key 'AMPLIHACK_CLAUDE_PROFILE' is reserved"),
            "fixed message missing: {err:?}"
        );
    }

    #[test]
    fn context_file_missing_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.json");
        let err = merge_err(Some(&missing), &[], &BTreeMap::new());
        assert!(err.contains(&missing.display().to_string()), "{err:?}");
    }

    #[test]
    fn context_from_file_missing_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.txt");
        let err = merge_err(None, &[from_file_arg("v", &missing)], &BTreeMap::new());
        assert!(err.contains(&missing.display().to_string()), "{err:?}");
    }

    #[test]
    fn context_file_invalid_json_names_the_path_not_the_contents() {
        let dir = tempfile::tempdir().unwrap();
        let json = write(dir.path(), "ctx.json", format!("{{\"k\": {SENTINEL}"));
        let err = merge_err(Some(&json), &[], &BTreeMap::new());
        assert!(err.contains(&json.display().to_string()), "{err:?}");
    }

    #[test]
    fn context_from_file_invalid_keys_are_rejected_naming_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "v.txt", SENTINEL);
        for key in ["bad-key", "bad.key", "bad key", "bäd"] {
            let err = merge_err(None, &[from_file_arg(key, &path)], &BTreeMap::new());
            assert!(err.contains(key), "error must name key {key:?}: {err:?}");
        }
    }

    #[test]
    fn context_from_file_empty_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "v.txt", SENTINEL);
        let _ = merge_err(None, &[format!("={}", path.display())], &BTreeMap::new());
    }

    #[test]
    fn context_file_invalid_keys_are_rejected_naming_the_key() {
        let dir = tempfile::tempdir().unwrap();
        for key in ["a=b", "bad-key", "bad.key", "", "bad key"] {
            let body = serde_json::to_vec(&serde_json::json!({ key: SENTINEL })).unwrap();
            let json = write(dir.path(), "ctx.json", body);
            let err = merge_err(Some(&json), &[], &BTreeMap::new());
            if !key.is_empty() {
                assert!(err.contains(key), "error must name key {key:?}: {err:?}");
            }
        }
    }

    #[test]
    fn context_from_file_nul_value_is_rejected_naming_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "v.txt", format!("{SENTINEL}\0tail"));
        let err = merge_err(None, &[from_file_arg("nul_key", &path)], &BTreeMap::new());
        assert!(err.contains("nul_key"), "{err:?}");
    }

    #[test]
    fn context_file_nul_value_is_rejected_naming_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let body =
            serde_json::to_vec(&serde_json::json!({ "nul_key": format!("{SENTINEL}\0tail") }))
                .unwrap();
        let json = write(dir.path(), "ctx.json", body);
        let err = merge_err(Some(&json), &[], &BTreeMap::new());
        assert!(err.contains("nul_key"), "{err:?}");
    }

    fn oversized_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(SIXTEEN_MIB + 1).unwrap();
        path
    }

    #[test]
    fn context_from_file_larger_than_16_mib_is_rejected_naming_path_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = oversized_file(dir.path(), "huge.txt");
        let err = merge_err(None, &[from_file_arg("huge", &path)], &BTreeMap::new());
        assert!(err.contains(&path.display().to_string()), "{err:?}");
        assert!(
            err.contains(&(SIXTEEN_MIB + 1).to_string()),
            "error must name the size in bytes: {err:?}"
        );
    }

    #[test]
    fn context_file_larger_than_16_mib_is_rejected_naming_path_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = oversized_file(dir.path(), "huge.json");
        let err = merge_err(Some(&path), &[], &BTreeMap::new());
        assert!(err.contains(&path.display().to_string()), "{err:?}");
        assert!(
            err.contains(&(SIXTEEN_MIB + 1).to_string()),
            "error must name the size in bytes: {err:?}"
        );
    }

    #[test]
    fn context_from_file_non_utf8_is_rejected_naming_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = SENTINEL.as_bytes().to_vec();
        bytes.extend_from_slice(&[0xff, 0xfe, 0xfd]);
        let path = write(dir.path(), "bin.txt", bytes);
        let err = merge_err(None, &[from_file_arg("bin", &path)], &BTreeMap::new());
        assert!(err.contains(&path.display().to_string()), "{err:?}");
    }

    #[test]
    fn context_file_non_utf8_is_rejected_naming_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = format!("{{\"k\":\"{SENTINEL}").into_bytes();
        bytes.extend_from_slice(&[0xff, 0xfe, b'"', b'}']);
        let path = write(dir.path(), "bin.json", bytes);
        let err = merge_err(Some(&path), &[], &BTreeMap::new());
        assert!(err.contains(&path.display().to_string()), "{err:?}");
    }

    // ---- clap -------------------------------------------------------------

    fn parse_run(args: &[&str]) -> (Option<PathBuf>, Vec<String>, Vec<String>) {
        let mut argv = vec!["amplihack", "recipe", "run", "default-workflow"];
        argv.extend_from_slice(args);
        let cli = crate::Cli::try_parse_from(argv).expect("recipe run must parse");
        match cli.command {
            crate::Commands::Recipe {
                command:
                    crate::RecipeCommands::Run {
                        context,
                        context_file,
                        context_from_file,
                        ..
                    },
            } => (
                context_file.as_deref().map(|p| Path::new(p).to_path_buf()),
                context_from_file,
                context,
            ),
            other => panic!("expected recipe run, got {other:?}"),
        }
    }

    #[test]
    fn clap_parses_context_file_and_repeated_context_from_file() {
        let (file, from_file, context) = parse_run(&[
            "--context-file",
            "/tmp/ctx.json",
            "--context-from-file",
            "task_description=/tmp/issue.md",
            "--context-from-file=cycle_history=/tmp/history.txt",
            "-c",
            "repo_path=.",
        ]);
        assert_eq!(file, Some(PathBuf::from("/tmp/ctx.json")));
        assert_eq!(
            from_file,
            vec![
                "task_description=/tmp/issue.md".to_string(),
                "cycle_history=/tmp/history.txt".to_string()
            ]
        );
        assert_eq!(context, vec!["repo_path=.".to_string()]);
    }

    #[test]
    fn clap_context_file_sources_default_to_empty() {
        let (file, from_file, _) = parse_run(&[]);
        assert_eq!(file, None);
        assert!(from_file.is_empty());
    }

    #[test]
    fn clap_rejects_context_file_given_twice_or_without_a_value() {
        let base = ["amplihack", "recipe", "run", "default-workflow"];
        let twice = [
            &base[..],
            &["--context-file", "a.json", "--context-file", "b.json"],
        ]
        .concat();
        assert!(crate::Cli::try_parse_from(twice).is_err());
        let no_value = [&base[..], &["--context-file"]].concat();
        assert!(crate::Cli::try_parse_from(no_value).is_err());
        let no_value = [&base[..], &["--context-from-file"]].concat();
        assert!(crate::Cli::try_parse_from(no_value).is_err());
    }

    #[test]
    fn context_from_file_nul_error_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "v.txt", "a\0b");
        let err = merge_err(None, &[from_file_arg("v", &path)], &BTreeMap::new());
        assert!(err.contains("contains a NUL byte"), "{err:?}");
        assert!(err.contains(&path.display().to_string()), "{err:?}");
    }

    #[test]
    fn a_long_invalid_key_is_cut_to_64_characters() {
        let dir = tempfile::tempdir().unwrap();
        let key = format!("bad-{}", "k".repeat(500));
        let json = write(dir.path(), "ctx.json", format!(r#"{{"{key}":"v"}}"#));
        let err = merge_err(Some(&json), &[], &BTreeMap::new());
        let shown: String = key.chars().take(64).collect();
        assert!(err.contains(&format!("'{shown}…'")), "{err:?}");
        assert!(
            !err.contains(&key),
            "the whole key must not be shown: {err:?}"
        );
    }
}

// --- the no-capacity warning before the run ---------------------------------

#[cfg(unix)]
mod capacity_warning {
    use super::super::*;
    use amplihack_utils::claude_capacity::{self as cap, CapacityFile, State};
    use amplihack_utils::root_sandbox::SkipPermissionsEnv;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn claude(_: &Path) -> amplihack_utils::agent_binary::Resolution {
        amplihack_utils::agent_binary::Resolution {
            binary: "claude".to_string(),
            source: amplihack_utils::agent_binary::ResolutionSource::Env,
            context_file: None,
            unusable_contexts: Vec::new(),
            session_marker: None,
        }
    }

    fn private_dir(p: &Path) {
        std::fs::create_dir_all(p).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// Runs a one-step bash recipe through a stub runner with a temporary
    /// HOME and profile root holding `state`. Returns the pre-flight output
    /// and whether the runner ran.
    fn run_with_capacity(state: Option<State>, toggle: &str, depth: &str) -> (String, bool) {
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // The run folder goes under a private work root, not the caller's
        // (issue #2067).
        let _work_root = crate::test_support::IsolatedWorkRoot::new();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let root = tmp.path().join("profiles");
        private_dir(&home);
        private_dir(&root);
        private_dir(&root.join(cap::STATE_DIR));
        if let Some(state) = state {
            let t = chrono::Utc::now().timestamp();
            let f = CapacityFile {
                version: 1,
                state,
                threshold: 90,
                over: vec!["a".into()],
                earliest_reset: chrono::DateTime::from_timestamp(t + 7200, 0),
                written_at: chrono::DateTime::from_timestamp(t, 0).unwrap(),
                writer: "statusline".into(),
            };
            let p = cap::path(&root);
            std::fs::write(&p, serde_json::to_vec(&f).unwrap()).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let ran = tmp.path().join("ran");
        let runner = tmp.path().join("recipe-runner-rs");
        std::fs::write(
            &runner,
            format!(
                "#!/bin/sh\ncase \"$1\" in --capabilities) printf '%b\\n' '\\173\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]\\175'; exit 0 ;; esac\n: > '{}'\n\
                 printf '%s' '{{\"recipe_name\":\"probe\",\"success\":true,\"step_results\":[],\"context\":{{}}}}'\n",
                ran.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755)).unwrap();
        let recipe = tmp.path().join("probe.yaml");
        std::fs::write(
            &recipe,
            "name: probe\nsteps:\n  - id: echo\n    type: bash\n    command: echo hi\n",
        )
        .unwrap();
        // #1984: the outer run's identity is cleared; the depth under test is set below.
        let _orchestration = crate::test_support::clear_outer_run_env();
        let _env = crate::test_support::EnvGuard::set([
            ("HOME", home.to_str().unwrap()),
            ("AMPLIHACK_CLAUDE_PROFILE_ROOT", root.to_str().unwrap()),
            ("AMPLIHACK_CLAUDE_AUTO_SWITCH", toggle),
            ("AMPLIHACK_SESSION_DEPTH", depth),
            ("RECIPE_RUNNER_RS_PATH", runner.to_str().unwrap()),
            ("TZ", "UTC"),
        ]);
        let mut out = Vec::new();
        let _ = run_recipe_with(
            recipe.to_str().unwrap(),
            &[],
            false,
            false,
            "table",
            Some(tmp.path().to_str().unwrap()),
            None,
            &RootSandboxPreflight {
                agent_binary: claude,
                decision: || SkipPermissionsEnv::NotRoot,
                governor: None,
            },
            &mut out,
        );
        (String::from_utf8(out).unwrap(), ran.exists())
    }

    #[test]
    fn low_capacity_warns_with_the_reset_and_still_runs() {
        let (out, ran) = run_with_capacity(Some(State::Low), "1", "0");
        assert!(
            out.contains("amplihack recipe run: ⚠ all Claude accounts ≥90% · next reset "),
            "{out}"
        );
        assert!(out.contains("run 'amplihack profile usage'"), "{out}");
        assert!(out.contains("(continuing anyway)"), "{out}");
        assert!(
            !out.contains("next reset unknown"),
            "the reset is known: {out}"
        );
        assert!(ran, "never refuses");
    }

    #[test]
    fn exhausted_capacity_warns_and_still_runs() {
        let (out, ran) = run_with_capacity(Some(State::Exhausted), "1", "0");
        assert!(
            out.contains("amplihack recipe run: ⚠ all Claude accounts at limit · next reset "),
            "{out}"
        );
        assert!(ran);
    }

    #[test]
    fn no_warning_when_off_ok_missing_or_nested() {
        for (state, toggle, depth, why) in [
            (Some(State::Exhausted), "0", "0", "off"),
            (Some(State::Ok), "1", "0", "ok"),
            (None, "1", "0", "missing"),
            (Some(State::Exhausted), "1", "1", "nested recipe run"),
        ] {
            let (out, _) = run_with_capacity(state, toggle, depth);
            assert!(!out.contains("Claude accounts"), "{why}: {out}");
        }
    }
}

// -------------------------------------------------------------------------
// Issue #1976 (decision 17.113): the removed `should_publish` context key.
//
// The key is the text before the first `=`, trimmed and compared exactly.
// Every value is refused. The message names the key once and never the value.
// -------------------------------------------------------------------------
mod issue_1976_removed_keys {
    use super::super::removed_keys::{
        REMOVED_CONTEXT_KEYS, reject_removed_context_keys, removed_key,
    };

    fn reject(pairs: &[&str]) -> (Option<i32>, String) {
        let pairs: Vec<String> = pairs.iter().map(|p| (*p).to_string()).collect();
        let mut out = Vec::new();
        let code = reject_removed_context_keys(&pairs, &mut out)
            .err()
            .map(|e| crate::command_error::exit_code(&e).expect("a CliExitError"));
        (code, String::from_utf8(out).expect("utf-8 message"))
    }

    #[test]
    fn the_table_lists_should_publish() {
        assert!(
            REMOVED_CONTEXT_KEYS
                .iter()
                .any(|(key, _)| *key == "should_publish"),
            "{REMOVED_CONTEXT_KEYS:?}"
        );
    }

    #[test]
    fn every_value_of_should_publish_is_matched() {
        for arg in [
            "should_publish=false",
            "should_publish=true",
            "should_publish=True",
            "should_publish=no",
            "should_publish=0",
            "should_publish=",
            "should_publish=a=b",
            "should_publish",
        ] {
            assert_eq!(removed_key(arg), Some("should_publish"), "{arg:?}");
        }
    }

    /// #2814: #2808 removed the separate review checkout, so a run given
    /// `review_dir` or `review_launch_dir` would accept and ignore it.
    #[test]
    fn the_retired_review_checkout_keys_are_refused_not_ignored() {
        for key in ["review_dir", "review_launch_dir"] {
            assert!(
                REMOVED_CONTEXT_KEYS
                    .iter()
                    .any(|(listed, _)| *listed == key),
                "{key} is not listed: {REMOVED_CONTEXT_KEYS:?}"
            );
            let (code, out) = reject(&["task_description=x", &format!("{key}=/tmp/anywhere")]);
            assert_eq!(code, Some(1), "{out}");
            assert!(out.contains(key), "{out}");
            assert!(out.contains("#2808"), "{out}");
            assert!(!out.contains("/tmp/anywhere"), "the value is echoed: {out}");
        }
    }

    #[test]
    fn whitespace_around_the_key_is_trimmed() {
        for arg in [
            " should_publish=false",
            "should_publish =false",
            "\tshould_publish\t=true",
        ] {
            assert_eq!(removed_key(arg), Some("should_publish"), "{arg:?}");
        }
    }

    #[test]
    fn other_keys_are_not_matched() {
        for arg in [
            "should_publish_extra=false",
            "xshould_publish=false",
            "Should_Publish=false",
            "SHOULD_PUBLISH=false",
            "task_description=should_publish=false",
            "terminal_state.should_publish=false",
            "create_issue=false",
            "",
            "=should_publish",
        ] {
            assert_eq!(removed_key(arg), None, "{arg:?}");
        }
    }

    #[test]
    fn a_removed_key_fails_with_exit_1_and_the_decided_message() {
        let (code, out) = reject(&["task_description=x", "should_publish=false"]);
        assert_eq!(code, Some(1), "{out}");
        assert_eq!(
            out.trim_end(),
            "Error: context key 'should_publish' was removed by #1976 (decision 17.113). \
             It never stopped a run from publishing; do not rely on it. \
             Remove '-c should_publish=…' from the command."
        );
        for needle in ["should_publish", "removed", "#1976"] {
            assert!(out.contains(needle), "missing {needle:?}: {out}");
        }
    }

    #[test]
    fn a_key_passed_twice_is_named_once() {
        let (code, out) = reject(&["should_publish=false", " should_publish = true"]);
        assert_eq!(code, Some(1));
        assert_eq!(out.lines().count(), 1, "{out}");
        assert_eq!(
            out.matches("context key 'should_publish'").count(),
            1,
            "{out}"
        );
    }

    #[test]
    fn the_value_is_never_echoed() {
        let (code, out) = reject(&["should_publish=SECRET123"]);
        assert_eq!(code, Some(1));
        assert!(!out.contains("SECRET123"), "{out}");
        assert!(!out.contains("should_publish=SECRET"), "{out}");
    }

    #[test]
    fn no_removed_key_passes_and_writes_nothing() {
        for pairs in [
            &[][..],
            &["task_description=publish should_publish=false"][..],
            &["should_publish_extra=1", "Should_Publish=false"][..],
        ] {
            let (code, out) = reject(pairs);
            assert_eq!(code, None, "{pairs:?}: {out}");
            assert!(out.is_empty(), "{pairs:?}: {out}");
        }
    }
}

// -------------------------------------------------------------------------
// Issue #2582: context_transport. When the runner reports `context_file_v1`,
// recipe context reaches it in a 0600 file under the run's private runtime
// folder and the runner's argv carries only `--context-file <path>`. Without
// the capability, today's `--set` path runs byte for byte.
// -------------------------------------------------------------------------

mod context_transport_2582 {
    use super::super::context_transport::{
        CONTEXT_FILE_CAPABILITY, ContextTransport, DOWNGRADE_NOTICE, refuse_reserved_keys,
        supports_context_file, write_context_file,
    };
    use super::super::execute::pass_context;
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// Distinctive synthetic values, each at least 12 characters with a
    /// unique token, so a match can only come from this test's context.
    const TASK: &str = "ZQXV2582TASKWORD fix the widget";
    const BODY: &str = "ZQXV2582BODYWORD {\"json\": [1, 2]} $(touch pwned) `x`";

    fn context() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("task_description".to_string(), TASK.to_string()),
            ("pr_body".to_string(), BODY.to_string()),
            ("flag".to_string(), "true".to_string()),
        ])
    }

    fn report(capabilities: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "version": "fixture",
            "capabilities": capabilities,
        })
    }

    fn with_capability() -> serde_json::Value {
        report(&["codex_exec", "durable_recovery_v1", CONTEXT_FILE_CAPABILITY])
    }

    fn without_capability() -> serde_json::Value {
        report(&["codex_exec", "durable_recovery_v1"])
    }

    /// A private runtime folder, as `RuntimeGuard` creates it (0700).
    fn runtime() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("runtime");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        (tmp, dir)
    }

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn is_root() -> bool {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }

    #[test]
    fn capability_constant_is_context_file_v1() {
        assert_eq!(CONTEXT_FILE_CAPABILITY, "context_file_v1");
    }

    #[test]
    fn capability_match_is_exact() {
        assert!(supports_context_file(&with_capability()));
        assert!(!supports_context_file(&without_capability()));
        assert!(!supports_context_file(&report(&[
            "context_file_v10",
            "context_file",
            "CONTEXT_FILE_V1",
            " context_file_v1",
        ])));
        assert!(!supports_context_file(&serde_json::json!({})));
        assert!(!supports_context_file(&serde_json::json!({
            "capabilities": "context_file_v1"
        })));
        assert!(!supports_context_file(&serde_json::Value::Null));
    }

    #[test]
    fn file_transport_argv_is_path_only() {
        let (_tmp, runtime) = runtime();
        let context = context();
        let transport = ContextTransport::choose(&with_capability(), &runtime, &context).unwrap();
        let ContextTransport::File(file) = &transport else {
            panic!("the capability must select the file transport");
        };
        let expected = runtime.join("context").join("context.json");
        assert_eq!(file.path(), expected.as_path());

        let mut command = Command::new("echo");
        let bytes = transport.apply(&mut command, &context).unwrap();
        let argv = args(&command);
        assert_eq!(
            argv,
            ["--context-file".to_string(), expected.display().to_string()]
        );
        for value in context.values() {
            assert!(
                !argv.iter().any(|a| a.contains(value.as_str())),
                "a context value reached argv"
            );
        }
        assert!(!argv.iter().any(|a| a == "--set" || a == "-c"));
        assert_eq!(
            bytes,
            "--context-file".len()
                + 1
                + expected.as_os_str().len()
                + 1
                + 2 * std::mem::size_of::<usize>(),
            "the argv cost of the file path feeds the env budget (D8)"
        );
    }

    #[test]
    fn file_holds_the_context_as_a_json_object_of_strings() {
        let (_tmp, runtime) = runtime();
        let context = context();
        let file = write_context_file(&runtime, &context).unwrap();
        let bytes = fs::read(file.path()).unwrap();
        let parsed: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.len(), context.len());
        for (key, value) in &context {
            assert_eq!(
                parsed.get(key),
                Some(&serde_json::Value::String(value.clone())),
                "{key} must be written as the string amplihack holds (D1)"
            );
        }
    }

    #[test]
    fn context_file_modes() {
        let (_tmp, runtime) = runtime();
        let file = write_context_file(&runtime, &context()).unwrap();
        let dir = runtime.join("context");
        assert_eq!(file.path(), dir.join("context.json").as_path());
        assert_eq!(mode(&dir), 0o700, "context folder must be 0700");
        assert_eq!(mode(file.path()), 0o600, "context file must be 0600");
        assert!(
            fs::symlink_metadata(file.path()).unwrap().is_file(),
            "context file must be a regular file"
        );
    }

    #[test]
    fn context_file_is_not_under_the_exported_tmp() {
        let (_tmp, runtime) = runtime();
        fs::create_dir(runtime.join("tmp")).unwrap();
        let file = write_context_file(&runtime, &context()).unwrap();
        assert!(!file.path().starts_with(runtime.join("tmp")));
        assert!(file.path().starts_with(&runtime));
    }

    #[test]
    fn context_file_removed_on_drop() {
        let (_tmp, runtime) = runtime();
        let file = write_context_file(&runtime, &context()).unwrap();
        let path = file.path().to_path_buf();
        assert!(path.is_file());
        drop(file);
        assert!(!path.exists(), "the context file must be removed");
        assert!(
            !runtime.join("context").exists(),
            "the context folder must be removed"
        );
        assert!(
            runtime.is_dir(),
            "the runtime folder belongs to RuntimeGuard"
        );
    }

    /// Issue #2582 (review 17b finding 2 / security S3): the one case where
    /// `ContextFile::drop` is actually LOAD-BEARING.
    ///
    /// Everywhere else, `RuntimeGuard::release()` ends with
    /// `remove_dir_all(<runtime>)`, which removes the context file whether or
    /// not the `Drop` ever ran — so no "nothing left behind" assertion can
    /// distinguish a working teardown from a deleted one. The exception is
    /// `RuntimeRemoval::KeptInUse`: a process still holds the runtime folder,
    /// `remove_dir_all` is deliberately NOT called, and the folder stays on
    /// disk until a later sweep. There, `ContextFile::drop` is the only thing
    /// removing a 0600 file that can hold auto-drive authorization material,
    /// and it is exactly the case with a live exposure.
    ///
    /// This test reproduces that shape directly: the runtime folder survives
    /// the release, and the context file must still be gone.
    #[test]
    fn context_file_is_removed_even_when_the_runtime_folder_is_retained() {
        use crate::commands::cleanup::procs::FakeProcesses;
        use crate::commands::cleanup::run_dir::{RuntimeGuard, RuntimeRemoval};

        let tmp = tempfile::tempdir().unwrap();
        let runtime = tmp.path().join("runtime");
        fs::create_dir(&runtime).unwrap();
        fs::create_dir(runtime.join("tmp")).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();

        // The context file, holding a distinctive secret.
        let mut context = self::context();
        context.insert(
            "autodrive_authorization".to_string(),
            "ZQXV2582AUTHWORD".to_string(),
        );
        let file = write_context_file(&runtime, &context).unwrap();
        let path = file.path().to_path_buf();
        assert!(path.is_file());

        // A process whose working folder is inside the runtime folder, in a
        // table this test wrote (issue #2935), so the folder must be kept.
        let processes = FakeProcesses::seen();
        processes.hold(&runtime.join("tmp"));

        // The run's own teardown order: the context file is dropped, then the
        // runtime folder is released.
        drop(file);
        let removal = RuntimeGuard::seen_by(runtime.clone(), processes)
            .unwrap()
            .remove()
            .unwrap();

        // The precondition that makes this test meaningful. If this ever
        // becomes `Removed`, the assertions below are proved by
        // `remove_dir_all` instead of by `ContextFile::drop`, and this test
        // has stopped testing anything.
        assert_eq!(
            removal,
            RuntimeRemoval::KeptInUse,
            "this test is only meaningful while the runtime folder is retained"
        );
        assert!(
            runtime.is_dir(),
            "a runtime folder in use is left in place, so remove_dir_all did NOT run"
        );

        // The actual claim.
        assert!(
            !path.exists(),
            "the context file must be gone even though the runtime folder was kept: {}",
            path.display()
        );
        assert!(!runtime.join("context").exists(), "and its 0700 folder too");
        // Nothing anywhere under the retained tree still holds the secret.
        let mut found = Vec::new();
        let mut stack = vec![runtime.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if fs::read(&p)
                    .map(|b| String::from_utf8_lossy(&b).contains("ZQXV2582AUTHWORD"))
                    .unwrap_or(false)
                {
                    found.push(p);
                }
            }
        }
        assert!(
            found.is_empty(),
            "authorization material left in the retained runtime folder: {found:?}"
        );
    }

    #[test]
    fn context_file_drop_leaves_unexpected_entries_alone() {
        // Cleanup is remove_file then remove_dir, never remove_dir_all.
        let (_tmp, runtime) = runtime();
        let file = write_context_file(&runtime, &context()).unwrap();
        let stray = runtime.join("context").join("stray.txt");
        fs::write(&stray, "kept").unwrap();
        drop(file);
        assert!(!runtime.join("context").join("context.json").exists());
        assert_eq!(fs::read_to_string(&stray).unwrap(), "kept");
    }

    #[test]
    fn transport_drop_removes_the_file() {
        let (_tmp, runtime) = runtime();
        let context = context();
        let transport = ContextTransport::choose(&with_capability(), &runtime, &context).unwrap();
        assert!(runtime.join("context").join("context.json").is_file());
        drop(transport);
        assert!(!runtime.join("context").exists());
    }

    #[test]
    fn fallback_matches_today_byte_for_byte() {
        let (_tmp, runtime) = runtime();
        let context = context();
        let transport =
            ContextTransport::choose(&without_capability(), &runtime, &context).unwrap();
        assert!(matches!(transport, ContextTransport::Set));
        assert!(
            !runtime.join("context").exists(),
            "the fallback writes no file"
        );

        // NOT `assert_eq!(transport.apply(..), pass_context(..))`: since the
        // refactor `ContextTransport::Set::apply` IS `pass_context`, so such a
        // comparison is `f(x) == f(x)` and cannot fail. The argv is therefore
        // pinned against a literal instead — that is what "byte for byte"
        // actually has to mean for a reader.
        let mut via_transport = Command::new("echo");
        let transport_bytes = transport.apply(&mut via_transport, &context).unwrap();

        // BTreeMap order: flag, pr_body, task_description.
        assert_eq!(
            args(&via_transport),
            vec![
                "--set".to_string(),
                "flag=true".to_string(),
                "--set".to_string(),
                format!("pr_body={BODY}"),
                "--set".to_string(),
                format!("task_description={TASK}"),
            ],
            "the --set argv must be exactly the pre-#2582 form"
        );
        assert!(args(&via_transport).iter().all(|a| a != "--context-file"));

        // And the byte count is the documented argv cost of those pairs:
        // `--set\0`, `KEY=VALUE\0` and two argv pointers per entry.
        let expected: usize = context
            .iter()
            .map(|(k, v)| {
                "--set".len() + 1 + k.len() + 1 + v.len() + 1 + 2 * std::mem::size_of::<usize>()
            })
            .sum();
        assert_eq!(transport_bytes, expected);
    }

    #[test]
    fn empty_context_passes_no_flag() {
        let (_tmp, runtime) = runtime();
        let empty = BTreeMap::new();
        for report in [with_capability(), without_capability()] {
            let transport = ContextTransport::choose(&report, &runtime, &empty).unwrap();
            assert!(matches!(transport, ContextTransport::None));
            let mut command = Command::new("echo");
            assert_eq!(transport.apply(&mut command, &empty).unwrap(), 0);
            assert_eq!(command.get_args().count(), 0);
        }
        assert!(!runtime.join("context").exists());
    }

    #[test]
    fn reserved_key_refused_on_both_paths() {
        let (_tmp, runtime) = runtime();
        let mut context = context();
        context.insert(
            "amplihack_claude_profile".to_string(),
            "ZQXV2582PROFILEWORD".to_string(),
        );
        for report in [with_capability(), without_capability()] {
            let error = match ContextTransport::choose(&report, &runtime, &context) {
                Ok(_) => panic!("a reserved key must be refused"),
                Err(error) => error.to_string(),
            };
            assert!(error.contains("amplihack_claude_profile"), "{error}");
            assert!(error.contains("reserved"), "{error}");
            assert!(!error.contains("ZQXV2582"), "values must not be printed");
            assert!(
                !runtime.join("context").exists(),
                "no file may be written before the refusal"
            );
        }
        assert!(refuse_reserved_keys(&context).is_err());
        assert!(refuse_reserved_keys(&self::context()).is_ok());
    }

    /// Issue #2582 (security review S1, widened after re-review): a context
    /// key whose value is RUN, or which chooses what is run, must be refused
    /// — on BOTH transports and in ANY letter case — never merely dropped
    /// from the env mirror by the denylist. Dropping is not enough: the
    /// `--set` path would still hand the pair to the runner.
    ///
    /// The list is deliberately the whole family rather than the one name
    /// #2582 made topical. `GIT_SSH_COMMAND` and friends are run by git
    /// itself (the repo already scrubs them in `install::clone` and flags
    /// them in the hooks' tmux guard), and `{TOOL}_BINARY_PATH` is resolved
    /// by `launch_target` as a user-supplied override at the highest
    /// precedence for ANY tool name — so the next tool added there is
    /// covered by the suffix rule without anyone remembering to come back.
    #[test]
    fn executable_selector_keys_are_reserved_on_both_paths() {
        for key in [
            // The runner selector (#2582).
            "recipe_runner_rs_path",
            "RECIPE_RUNNER_RS_PATH",
            "Recipe_Runner_Rs_Path",
            // Git/SSH names whose value git executes.
            "git_ssh_command",
            "git_ssh",
            "git_proxy_command",
            "git_askpass",
            "ssh_askpass",
            "git_external_diff",
            "git_sequence_editor",
            "git_editor",
            "git_pager",
            // `{TOOL}_BINARY_PATH`, including a tool that does not exist yet.
            "claude_binary_path",
            "codex_binary_path",
            "copilot_binary_path",
            "some_future_tool_binary_path",
        ] {
            let (_tmp, runtime) = runtime();
            let mut context = context();
            context.insert(key.to_string(), "/tmp/ZQXV2582EVILRUNNER".to_string());

            for report in [with_capability(), without_capability()] {
                let error = match ContextTransport::choose(&report, &runtime, &context) {
                    Ok(_) => panic!("{key} must be refused, not accepted"),
                    Err(error) => error.to_string(),
                };
                assert!(error.contains("reserved"), "{error}");
                assert!(
                    !error.contains("ZQXV2582"),
                    "the value must not be printed: {error}"
                );
                assert!(
                    !runtime.join("context").exists(),
                    "no file may be written before the refusal"
                );
            }

            assert!(refuse_reserved_keys(&context).is_err(), "{key}");

            // And the `--set` path refuses it identically, so neither
            // transport is a way around the rule.
            let mut command = Command::new("echo");
            assert!(pass_context(&mut command, &context).is_err(), "{key}");
        }
    }

    /// The operator-visible refusal text, pinned against a literal.
    ///
    /// This used to `assert_eq!` the message from `refuse_reserved_keys`
    /// against the one from `pass_context`. Since the refactor `pass_context`
    /// CALLS `refuse_reserved_keys`, so that comparison was `f(x) == f(x)`:
    /// it could not fail, and it would have gone on passing if the message
    /// had been reworded or emptied. The text itself is the contract, so the
    /// text is what is asserted.
    #[test]
    fn refusal_message_is_pinned_and_shared_by_both_entry_points() {
        const EXPECTED: &str = "recipe context key \"AMPLIHACK_X\" is reserved: the AMPLIHACK_ namespace, Git execution configuration and any name that selects or runs a program cannot be set from recipe context";

        let context = BTreeMap::from([("AMPLIHACK_X".to_string(), "v".to_string())]);
        let moved = refuse_reserved_keys(&context).unwrap_err().to_string();
        assert_eq!(moved, EXPECTED);

        // `pass_context` reaches the same refusal, and the caller sees the
        // same words — now asserted against the literal, not against `moved`.
        let mut command = Command::new("echo");
        let today = pass_context(&mut command, &context)
            .unwrap_err()
            .to_string();
        assert_eq!(today, EXPECTED);
        assert!(
            command.get_args().count() == 0,
            "a refused context must put nothing on the argv"
        );
    }

    #[test]
    fn oversize_pair_only_refused_on_fallback() {
        let (_tmp, runtime) = runtime();
        let big = "y".repeat(200 * 1024);
        let context = BTreeMap::from([("task_description".to_string(), big.clone())]);

        let file = ContextTransport::choose(&with_capability(), &runtime, &context).unwrap();
        let mut command = Command::new("echo");
        file.apply(&mut command, &context)
            .expect("the file path has no per-pair MAX_ARG_STRLEN limit (D6)");
        let written = fs::read(runtime.join("context").join("context.json")).unwrap();
        assert!(written.len() > big.len());
        drop(file);

        let set = ContextTransport::choose(&without_capability(), &runtime, &context).unwrap();
        let mut command = Command::new("echo");
        let error = set.apply(&mut command, &context).unwrap_err().to_string();
        assert!(error.contains("131072"), "{error}");
        assert!(!error.contains("yyyy"));
    }

    #[test]
    fn context_dir_preexisting_refused() {
        // A real folder.
        let (_tmp, runtime) = runtime();
        fs::create_dir(runtime.join("context")).unwrap();
        assert!(write_context_file(&runtime, &context()).is_err());
        assert!(!runtime.join("context").join("context.json").exists());

        // A symbolic link named `context` pointing at a folder elsewhere.
        let (tmp, runtime) = self::runtime();
        let elsewhere = tmp.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, runtime.join("context")).unwrap();
        assert!(write_context_file(&runtime, &context()).is_err());
        assert!(
            fs::read_dir(&elsewhere).unwrap().next().is_none(),
            "nothing may be written through the link"
        );
        assert!(
            fs::symlink_metadata(runtime.join("context"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the planted link is left as it is"
        );
    }

    /// Asserts the module's central negative claim for one way of making the
    /// write fail: the capability IS present, the write fails, and the run
    /// must end as an error rather than quietly degrading to `--set`.
    fn assert_no_fallback_on_write_failure(runtime: &Path) {
        let context = context();
        match ContextTransport::choose(&with_capability(), runtime, &context) {
            Ok(ContextTransport::Set) => panic!("a write failure must never fall back to --set"),
            Ok(_) => panic!("a write failure must be an error"),
            Err(error) => {
                let text = format!("{error:#}");
                // Pin WHICH failure this is. Without it, an earlier failure
                // mode added to `choose` later would let all three cases
                // report a pass for a reason that has nothing to do with the
                // write.
                assert!(
                    text.contains("failed to write recipe context file"),
                    "the refusal must be the write failure, not some earlier \
                     error: {text}"
                );
                assert!(!text.contains("ZQXV2582"), "values must not be printed");
            }
        }
    }

    /// Issue #2582, the central negative claim: with the capability present,
    /// a failed context-file write ends the run and NEVER degrades to `--set`.
    ///
    /// The trigger is an existing entry at `<runtime>/context`, which
    /// `write_context_file` refuses with `AlreadyExists`. That is a *root-safe*
    /// trigger: unlike a 0500 folder, it does not depend on DAC enforcement,
    /// so this test really runs in a root CI image instead of reporting a pass
    /// it never earned.
    #[test]
    fn capability_present_write_failure_does_not_fall_back() {
        // An existing regular file where the context folder must go.
        let (_tmp, runtime) = runtime();
        fs::write(runtime.join("context"), b"planted").unwrap();
        assert_no_fallback_on_write_failure(&runtime);

        // An existing folder of the same name.
        let (_tmp, runtime) = self::runtime();
        fs::create_dir(runtime.join("context")).unwrap();
        assert_no_fallback_on_write_failure(&runtime);

        // A runtime folder that does not exist at all.
        let tmp = tempfile::tempdir().unwrap();
        assert_no_fallback_on_write_failure(&tmp.path().join("not-created"));
    }

    /// The same claim for the permission-denied trigger, which only a non-root
    /// euid can produce. Kept separate from the root-safe case above so that
    /// skipping it cannot silently take the central claim with it.
    #[test]
    fn capability_present_unwritable_runtime_does_not_fall_back() {
        if is_root() {
            // Root ignores DAC, so a 0500 folder does not stop the write.
            return;
        }
        let (_tmp, runtime) = runtime();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o500)).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_no_fallback_on_write_failure(&runtime)
        }));
        // Restore the mode before propagating, so the tempdir can be removed.
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn write_error_has_no_values() {
        // Root-safe trigger first, so the no-values claim is asserted even in
        // a root CI image where the permission-denied case below cannot run.
        let (_tmp, planted) = runtime();
        fs::write(planted.join("context"), b"planted").unwrap();
        let text = match write_context_file(&planted, &context()) {
            Ok(_) => panic!("an existing entry at <runtime>/context must fail the write"),
            Err(error) => format!("{error:#}"),
        };
        assert!(
            text.contains("failed to write recipe context file"),
            "{text}"
        );
        assert!(!text.contains("ZQXV2582"), "{text}");
        assert!(!text.contains("widget"), "{text}");

        if is_root() {
            return;
        }
        let (_tmp, runtime) = runtime();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o500)).unwrap();
        let result = write_context_file(&runtime, &context());
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let text = match result {
            Ok(_) => panic!("writing into a 0500 folder must fail"),
            Err(error) => format!("{error:#}"),
        };
        assert!(
            text.contains("failed to write recipe context file"),
            "{text}"
        );
        assert!(!text.contains("ZQXV2582"), "{text}");
        assert!(!text.contains("widget"), "{text}");
    }

    #[test]
    fn missing_runtime_dir_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("not-created");
        assert!(write_context_file(&gone, &context()).is_err());
        assert!(!gone.exists(), "the runtime folder is not created here");
    }

    // ---------------------------------------------------------------------
    // The operator-visible record of the chosen transport.
    //
    // The pinned runner reports the capability on every host, so a run that
    // took the `--set` path means a runner older than the pin was supplied
    // through `RECIPE_RUNNER_RS_PATH`. Without a record that is visible only
    // in `/proc`, which is the thing this change removes. The record is at
    // info level and holds the transport name and nothing else: no key, no
    // value, and no count derived from one.
    // ---------------------------------------------------------------------

    /// What the `fmt` subscriber would write for one transport's record, with
    /// `info` as the maximum level, so a record emitted below it is invisible
    /// here and the test fails on the empty string.
    ///
    /// This is strictly *more* permissive than production, where the binary
    /// installs `EnvFilter::from_default_env()` and an unset `RUST_LOG` means
    /// `ERROR`. So no assertion built on this helper can show that an
    /// operator sees anything at all; it can only show what the record
    /// carries when something is listening. The claim that the `--set`
    /// downgrade is actually visible belongs to
    /// `the_set_downgrade_prints_to_stderr_without_rust_log` below and to the
    /// integration assertion in
    /// `tests/issue_2582_degraded_runner_no_set.rs`.
    fn captured_record(transport: &ContextTransport) -> String {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone, Default)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buffer {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Buffer {
            type Writer = Buffer;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        tracing::subscriber::with_default(subscriber, || transport.record());
        let bytes = buffer.0.lock().unwrap().clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The notice names the capability whose absence it reports. The constant
    /// spells the name out because a `const` cannot be formatted, so without
    /// this a rename of [`CONTEXT_FILE_CAPABILITY`] would leave the operator
    /// reading about a capability that no longer exists.
    #[test]
    fn the_notice_names_the_capability_and_the_override() {
        assert!(
            DOWNGRADE_NOTICE.contains(CONTEXT_FILE_CAPABILITY),
            "{DOWNGRADE_NOTICE}"
        );
        assert!(
            DOWNGRADE_NOTICE.contains("RECIPE_RUNNER_RS_PATH"),
            "the notice must name what to change: {DOWNGRADE_NOTICE}"
        );
        assert!(
            DOWNGRADE_NOTICE.starts_with("amplihack: "),
            "the house prefix: {DOWNGRADE_NOTICE}"
        );
        assert!(
            !DOWNGRADE_NOTICE.contains('\n'),
            "one line: {DOWNGRADE_NOTICE}"
        );
    }

    /// The `--set` downgrade prints to stderr; the other two arms do not.
    ///
    /// This is the half of the control a unit test can hold. `captured_record`
    /// above cannot: it installs a subscriber at `INFO`, which is strictly
    /// more permissive than production, where `bins/amplihack` installs
    /// `EnvFilter::from_default_env()` and an unset `RUST_LOG` means `ERROR`
    /// — so the `tracing` record is written nowhere in a default run and no
    /// assertion on it shows that an operator sees anything. Nor can a test
    /// here read the bytes: `eprintln!` under `cargo test` goes to libtest's
    /// capture buffer rather than fd 2, so a test that searched fd 2 would
    /// pass whether or not the line were emitted at all. What this asserts is
    /// the decision `record` acts on; that the bytes really reach stderr is
    /// asserted against the real binary in
    /// `tests/issue_2582_degraded_runner_no_set.rs`.
    #[test]
    fn only_the_set_transport_has_a_downgrade_notice() {
        let (_tmp, runtime) = runtime();
        let context = context();
        let set = ContextTransport::choose(&without_capability(), &runtime, &context).unwrap();
        let notice = set
            .downgrade_notice()
            .expect("the --set downgrade must be printed, or the control exists only in prose");
        assert_eq!(notice, DOWNGRADE_NOTICE);
        // The line is printed because the context is already exposed; it must
        // not add to the exposure.
        for key in context.keys() {
            assert!(
                !notice.contains(key.as_str()),
                "a key was printed: {notice}"
            );
        }
        for value in context.values() {
            assert!(
                !notice.contains(value.as_str()),
                "a value was printed: {notice}"
            );
        }
        assert!(
            !notice.contains("ZQXV2582"),
            "a value was printed: {notice}"
        );

        // `file` is what a normal run takes, and a notice on every run teaches
        // an operator to skip the one that matters.
        for (transport, name) in [
            (
                ContextTransport::choose(&with_capability(), &runtime, &context).unwrap(),
                "file",
            ),
            (
                ContextTransport::choose(&with_capability(), &runtime, &BTreeMap::new()).unwrap(),
                "none",
            ),
        ] {
            assert_eq!(
                transport.downgrade_notice(),
                None,
                "the {name} transport must print nothing"
            );
        }
    }

    #[test]
    fn transport_names_are_none_file_and_set() {
        let (_tmp, runtime) = runtime();
        let context = context();
        let empty = BTreeMap::new();
        assert_eq!(
            ContextTransport::choose(&with_capability(), &runtime, &empty)
                .unwrap()
                .name(),
            "none"
        );
        assert_eq!(
            ContextTransport::choose(&with_capability(), &runtime, &context)
                .unwrap()
                .name(),
            "file"
        );
        assert_eq!(
            ContextTransport::choose(&without_capability(), &runtime, &context)
                .unwrap()
                .name(),
            "set"
        );
    }

    #[test]
    fn the_record_is_at_info_and_carries_the_name_alone() {
        let (_tmp, runtime) = runtime();
        let context = context();
        for (transport, name) in [
            (
                ContextTransport::choose(&with_capability(), &runtime, &context).unwrap(),
                "file",
            ),
            (
                ContextTransport::choose(&without_capability(), &runtime, &context).unwrap(),
                "set",
            ),
            (
                ContextTransport::choose(&with_capability(), &runtime, &BTreeMap::new()).unwrap(),
                "none",
            ),
        ] {
            let text = captured_record(&transport);
            assert!(
                !text.trim().is_empty(),
                "the transport must be recorded at info level, not below it"
            );
            assert!(text.contains("INFO"), "the record must be at info: {text}");
            assert!(
                text.contains("recipe context transport"),
                "the record must be findable by its message: {text}"
            );
            let (_, fields) = text
                .split_once("recipe context transport")
                .expect("the record's message");
            assert_eq!(
                fields.trim(),
                format!("transport={name}"),
                "the record carries the transport name and nothing else: {text}"
            );
            for key in context.keys() {
                assert!(!text.contains(key.as_str()), "a key was recorded: {text}");
            }
            for value in context.values() {
                assert!(
                    !text.contains(value.as_str()),
                    "a value was recorded: {text}"
                );
            }
            assert!(!text.contains("ZQXV2582"), "a value was recorded: {text}");
            // Against the fields, not the whole line: the subscriber's
            // timestamp carries arbitrary digits, so a whole-line search for
            // `context.len()` would pass or fail by the clock. The exact
            // equality above is the stronger statement anyway — it pins the
            // fields to the transport name and nothing else — and this keeps
            // the reason a count is unwanted written down: a count of a
            // one-key context says which key it was.
            assert!(
                !fields.contains(&context.len().to_string()),
                "a count derived from the context was recorded: {text}"
            );
        }
    }

    // ---------------------------------------------------------------------
    // A planted entry at `<runtime_dir>/context`.
    //
    // `context_dir_preexisting_refused` covers a real folder and a symbolic
    // link to a folder that exists, which is the case a `create_dir_all` (or
    // a `DirBuilder` relaxed to `recursive(true)`) would accept and then
    // write through. These are the other three shapes: a dangling link, a
    // link to a file, and a regular file. `mkdir` refuses each, so nothing is
    // created at the link's target and nothing is written.
    // ---------------------------------------------------------------------

    #[test]
    fn a_planted_entry_at_the_context_path_is_refused_with_nothing_written() {
        // A dangling symbolic link: `create_dir_all` would create the target.
        let (_dangling, runtime) = runtime();
        let target = runtime.join("planted-target");
        std::os::unix::fs::symlink(&target, runtime.join("context")).unwrap();
        let error = format!(
            "{:#}",
            write_context_file(&runtime, &context()).unwrap_err()
        );
        assert!(error.contains("already exists"), "{error}");
        assert!(!error.contains("ZQXV2582"), "{error}");
        assert!(
            fs::symlink_metadata(&target).is_err(),
            "nothing may be created through a dangling link"
        );
        assert!(
            fs::symlink_metadata(runtime.join("context"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the planted link is left as it is"
        );

        // A symbolic link to a file that exists elsewhere.
        let (tmp, runtime) = self::runtime();
        let elsewhere = tmp.path().join("elsewhere.txt");
        fs::write(&elsewhere, b"keep me").unwrap();
        std::os::unix::fs::symlink(&elsewhere, runtime.join("context")).unwrap();
        assert!(write_context_file(&runtime, &context()).is_err());
        assert_eq!(
            fs::read(&elsewhere).unwrap(),
            b"keep me",
            "the link's target is untouched"
        );

        // A regular file where the folder would go.
        let (_file, runtime) = self::runtime();
        fs::write(runtime.join("context"), b"not a folder").unwrap();
        assert!(write_context_file(&runtime, &context()).is_err());
        assert_eq!(
            fs::read(runtime.join("context")).unwrap(),
            b"not a folder",
            "the planted file is left as it is"
        );
    }
}
