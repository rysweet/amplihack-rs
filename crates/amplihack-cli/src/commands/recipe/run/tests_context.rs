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
