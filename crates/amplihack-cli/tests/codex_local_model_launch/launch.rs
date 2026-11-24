//! What a local-model launch hands Codex, and what it builds (AC7, AC8,
//! AC11, AC13, AC14, SEC-F6), against what an ordinary launch does.

use super::harness::*;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// AC7, by the flag.
#[test]
fn the_flag_starts_codex_on_the_generated_home() {
    let f = Fixture::new();
    let launch = f.local(NAME, &[]);
    launch.assert_started();
    assert_child_env(&launch, &f, NAME);
}

/// AC7, by the variable, which is how a recipe agent step selects it.
#[test]
fn the_variable_starts_codex_on_the_generated_home() {
    let f = Fixture::new();
    let launch = f.launch(
        f.command(&["codex", "--", "exec", "-"])
            .env(LOCAL_MODEL_ENV, NAME),
    );
    launch.assert_started();
    assert_child_env(&launch, &f, NAME);
}

fn assert_child_env(launch: &Launched, f: &Fixture, name: &str) {
    let home = f.generated(name);
    assert_eq!(
        launch.env("CODEX_HOME").map(Path::new),
        Some(home.as_path()),
        "CODEX_HOME"
    );
    assert_eq!(launch.env(HOOK_TEXT_ENV), Some("none"));
    assert_eq!(launch.env(LOCAL_MODEL_ENV), Some(name));
    assert_eq!(
        launch.env("AMPLIHACK_HOOK_PROTOCOL"),
        None,
        "the hook command sets the protocol, never the launch"
    );
    assert!(
        !launch
            .argv()
            .iter()
            .any(|a| a == "--profile" || a == "-p" || a.starts_with("--profile=")),
        "no --profile is passed: {:?}",
        launch.argv()
    );
}

/// AC8: the settings in `config.toml`. Settings are compared, not layout.
#[test]
fn config_toml_holds_the_profile_and_amplihack_settings() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let home = f.generated(NAME);
    let config = read_toml(&home.join("config.toml"));

    // The profile's model and provider.
    assert_eq!(config["model"].as_str(), Some("inkling-2-mini"));
    assert_eq!(config["model_provider"].as_str(), Some("llamacpp-local"));
    assert_eq!(config["model_context_window"].as_integer(), Some(65536));
    let provider = &config["model_providers"]["llamacpp-local"];
    assert_eq!(
        provider["base_url"].as_str(),
        Some("http://127.0.0.1:8080/v1")
    );
    assert_eq!(provider["stream_max_retries"].as_integer(), Some(2));

    // amplihack's settings override the profile's.
    assert_eq!(config["project_doc_max_bytes"].as_integer(), Some(0));
    assert_eq!(config["web_search"].as_str(), Some("disabled"));
    let instructions = config["model_instructions_file"]
        .as_str()
        .expect("model_instructions_file is a string");
    let instructions = Path::new(instructions);
    assert!(instructions.is_absolute(), "{instructions:?}");
    assert_eq!(instructions, home.join("instructions.md"));
    let text = fs::read(instructions).unwrap();
    assert!(
        !text.is_empty() && text.len() <= 1024,
        "{} bytes",
        text.len()
    );
    assert!(std::str::from_utf8(&text).is_ok(), "UTF-8");

    let features = config["features"].as_table().expect("[features]");
    for off in ["multi_agent", "goals", "view_image", "plugins", "apps"] {
        assert_eq!(features[off].as_bool(), Some(false), "features.{off}");
    }
    assert_eq!(features["hooks"].as_bool(), Some(true), "features.hooks");
    assert_eq!(
        features["some_future_feature"].as_bool(),
        Some(true),
        "other profile features are kept"
    );
    let bundled = config["skills"]["bundled"]
        .as_table()
        .expect("[skills.bundled]");
    assert_eq!(bundled["enabled"].as_bool(), Some(false));
    assert_eq!(bundled["extra"].as_str(), Some("kept"));

    // The profile has no approval_policy, so the default is filled in. The
    // user's own config.toml says "on-request", and must not be read.
    assert_eq!(config["approval_policy"].as_str(), Some("never"));

    // Dropped from the profile.
    for dropped in [
        "marketplaces",
        "plugins",
        "projects",
        "hooks",
        "profile",
        "profiles",
    ] {
        assert!(
            config.get(dropped).is_none(),
            "{dropped} must not reach the generated config: {config:#?}"
        );
    }
    // Nothing else from the user's config.toml either.
    assert!(config.get("sandbox_mode").is_none());
}

#[test]
fn the_profile_approval_policy_is_kept() {
    let f = Fixture::new();
    f.write_profile(
        "strict",
        &format!("approval_policy = \"on-request\"\n{}", base_profile()),
    );
    f.local("strict", &[]).assert_started();
    let config = read_toml(&f.generated("strict").join("config.toml"));
    assert_eq!(config["approval_policy"].as_str(), Some("on-request"));
}

pub(crate) fn base_profile() -> &'static str {
    "model = \"m\"\nmodel_provider = \"p\"\n\n[model_providers.p]\nbase_url = \"http://127.0.0.1:1/v1\"\n"
}

/// AC8: hooks.json holds PreToolUse only, amplihack's entry once and first,
/// the plugin wrapper gone and the user's guards kept as they were.
#[test]
fn hooks_json_keeps_the_user_guards_and_one_amplihack_entry() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let hooks = read_json(&f.generated(NAME).join("hooks.json"));
    let events: BTreeSet<&str> = hooks["hooks"]
        .as_object()
        .expect("hooks object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(events, BTreeSet::from(["PreToolUse"]));
    assert_eq!(
        hooks.as_object().unwrap().len(),
        1,
        "only the hooks key: {hooks:#}"
    );

    let groups = hooks["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(
        groups.len(),
        3,
        "amplihack's group and the two guards: {hooks:#}"
    );

    let first = &groups[0];
    assert!(first.get("matcher").is_none(), "no matcher: {first}");
    let first_commands = commands(first);
    assert_eq!(first_commands.len(), 1, "{first}");
    let words = shell_words(&first_commands[0]);
    assert_eq!(
        words,
        [
            "env".to_string(),
            "AMPLIHACK_HOOK_PROTOCOL=codex".to_string(),
            f.hooks_bin().display().to_string(),
            "pre-tool-use".to_string(),
        ],
        "{}",
        first_commands[0]
    );
    assert_eq!(first["hooks"][0]["type"], "command");

    assert_eq!(&groups[1..], kept_user_groups(f.home()).as_slice());

    let all: Vec<String> = groups.iter().flat_map(commands).collect();
    let amplihack_entries = all.iter().filter(|c| c.contains("pre-tool-use")).count();
    assert_eq!(amplihack_entries, 1, "{all:#?}");
    assert!(
        !all.iter()
            .any(|c| c.contains("/bin/hook") || c.contains("XPIA_ENABLED")),
        "the plugin wrapper is dropped in both forms: {all:#?}"
    );
}

/// The few shell-quoting forms the generated command uses: single quotes,
/// and `'\''` for a quote inside them.
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    word.push(q);
                }
            }
            '\\' => {
                in_word = true;
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            ' ' => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            other => {
                in_word = true;
                word.push(other);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// AC8 and SEC-F6: the files, their modes, and nothing amplihack did not
/// put there (no plugin, no skills, no marketplace).
#[test]
fn the_generated_home_holds_only_the_owned_files_privately() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let home = f.generated(NAME);
    let listing: BTreeSet<String> = snapshot(&home)
        .keys()
        .map(|p| p.display().to_string())
        .collect();
    let expected: BTreeSet<String> = [
        "config.toml",
        "hooks.json",
        "instructions.md",
        "rules",
        RULES_FILE,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(listing, expected);

    for dir in [f.codex_local(), home.clone(), home.join("rules")] {
        assert_eq!(mode(&dir), 0o700, "{}", dir.display());
    }
    for file in OWNED {
        assert_eq!(mode(&home.join(file)), 0o600, "{file}");
    }
    let rules = fs::read(home.join(RULES_FILE)).unwrap();
    assert!(!rules.is_empty(), "the rules file has content");
}

/// The rules file is the one `amplihack install` writes for an ordinary
/// Codex home: compare with what the same binary renders there.
#[test]
fn the_rules_file_carries_the_configured_deny_commands() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let config = f.home().join(".amplihack/config.toml");
    fs::write(
        &config,
        "[policy]\ndeny_commands = [\"issue-3172-forbidden-tool\"]\n",
    )
    .unwrap();
    // A group- or other-writable user config is ignored (#2589).
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    f.local(NAME, &[]).assert_started();
    let rules = fs::read_to_string(f.generated(NAME).join(RULES_FILE)).unwrap();
    assert!(
        rules.contains("issue-3172-forbidden-tool"),
        "policy.deny_commands reaches the rules file:\n{rules}"
    );
}

/// AC11: an ordinary launch hands both variables through as it got them and
/// builds nothing.
#[test]
fn an_ordinary_launch_passes_codex_home_and_hook_text_through() {
    let f = Fixture::new();
    for hook_text in [None, Some("full"), Some("none")] {
        for codex_home in [Some(f.user_codex()), None] {
            let mut command = f.command(&["codex", "--", "exec", "-"]);
            match &codex_home {
                Some(home) => command.env("CODEX_HOME", home),
                None => command.env_remove("CODEX_HOME"),
            };
            if let Some(value) = hook_text {
                command.env(HOOK_TEXT_ENV, value);
            }
            let launch = f.launch(&mut command);
            launch.assert_started();
            assert_eq!(
                launch.env("CODEX_HOME").map(Path::new),
                codex_home.as_deref(),
                "{hook_text:?}"
            );
            assert_eq!(launch.env(HOOK_TEXT_ENV), hook_text, "{codex_home:?}");
            assert_eq!(launch.env(LOCAL_MODEL_ENV), None);
            assert!(
                !f.codex_local().exists(),
                "an ordinary launch builds nothing"
            );
        }
    }
}

/// AC13: the flag wins over the variable.
#[test]
fn the_flag_wins_over_the_variable() {
    let f = Fixture::new();
    f.write_profile("alpha", base_profile());
    f.write_profile("beta", base_profile());
    let launch = f.launch(
        f.command(&["codex", "--local-model", "alpha"])
            .env(LOCAL_MODEL_ENV, "beta"),
    );
    launch.assert_started();
    assert_child_env(&launch, &f, "alpha");
    assert!(!f.generated("beta").exists());
}

/// AC13: an empty variable counts as unset.
#[test]
fn an_empty_variable_gives_an_ordinary_launch() {
    let f = Fixture::new();
    let launch = f.launch(
        f.command(&["codex", "--", "exec", "-"])
            .env(LOCAL_MODEL_ENV, ""),
    );
    launch.assert_started();
    assert_eq!(
        launch.env("CODEX_HOME").map(Path::new),
        Some(f.user_codex().as_path())
    );
    assert_eq!(launch.env(HOOK_TEXT_ENV), None);
    assert!(!f.codex_local().exists());
}

/// AC14: the hook-trust flag appears exactly once, first, whatever the
/// caller passed, interactive or `exec`; `-c features.hooks=true` (S2)
/// appears once, before the caller's arguments.
#[test]
fn hook_trust_appears_exactly_once() {
    let f = Fixture::new();
    let cases: &[(&[&str], &[&str])] = &[
        (&[], &[HOOK_TRUST, "-c", HOOKS_ON]),
        (
            &["resume", "--last"],
            &[HOOK_TRUST, "-c", HOOKS_ON, "resume", "--last"],
        ),
        (&[HOOK_TRUST], &["-c", HOOKS_ON, HOOK_TRUST]),
        (&[HOOK_TRUST, "exec", "-"], &[]),
        (&["exec", "-"], &[]),
        (&["exec", HOOK_TRUST, "-"], &[]),
        (&["-c", "model_reasoning_effort=\"low\"", "exec", "-"], &[]),
    ];
    for (tail, exact) in cases {
        let launch = f.local(NAME, tail);
        launch.assert_started();
        assert_eq!(launch.count(HOOK_TRUST), 1, "{tail:?}: {:?}", launch.argv());
        assert_eq!(launch.count(HOOKS_ON), 1, "{tail:?}: {:?}", launch.argv());
        let on = launch.argv().iter().position(|a| a == HOOKS_ON).unwrap();
        assert_eq!(launch.argv()[on - 1], "-c", "{tail:?}: {:?}", launch.argv());
        if let Some(first) = tail.iter().find(|a| **a != HOOK_TRUST && **a != "-c") {
            let caller = launch.argv().iter().position(|a| a == first).unwrap();
            assert!(on < caller, "{tail:?}: ours first: {:?}", launch.argv());
        }
        if !exact.is_empty() {
            assert_eq!(launch.argv(), *exact, "{tail:?}");
        }
        if !tail.contains(&HOOK_TRUST) {
            assert_eq!(launch.argv()[0], HOOK_TRUST, "{tail:?}: put first");
        }
        for arg in tail.iter().filter(|a| **a != HOOK_TRUST) {
            assert!(
                launch.argv().iter().any(|a| a == arg),
                "{tail:?}: the caller's {arg:?} passes through"
            );
        }
        assert!(
            !launch
                .argv()
                .iter()
                .any(|a| a == "--dangerously-bypass-approvals-and-sandbox" || a == "--yolo"),
            "{tail:?}"
        );
    }
}

/// D12 and S2: a caller who turns hooks off still can. Their own `-c` or
/// `--disable hooks` reaches Codex unchanged, after amplihack's
/// `-c features.hooks=true`, so Codex applies theirs last.
#[test]
fn a_caller_can_still_turn_hooks_off() {
    let f = Fixture::new();
    for tail in [
        &["-c", "features.hooks=false", "exec", "-"][..],
        &["--disable", "hooks", "exec", "-"][..],
    ] {
        let launch = f.local(NAME, tail);
        launch.assert_started();
        let argv = launch.argv();
        let ours = argv.iter().position(|a| a == HOOKS_ON).unwrap();
        let theirs = argv
            .windows(2)
            .position(|pair| pair[0] == tail[0] && pair[1] == tail[1])
            .unwrap_or_else(|| panic!("{tail:?} passes through: {argv:?}"));
        assert!(ours < theirs, "{tail:?}: {argv:?}");
    }
}

/// A `--` among the Codex arguments ends Codex's options: a hook-trust flag
/// after it is prompt text, so amplihack still adds its own.
///
/// Interactive, because under `exec` the prompt after `--` is moved to
/// stdin by the #2030 rewrite and never reaches argv; interactive Codex's
/// positional text is passed through as it is.
#[test]
fn hook_trust_after_codex_end_of_options_does_not_count() {
    let f = Fixture::new();
    let launch = f.local(NAME, &["--", HOOK_TRUST]);
    launch.assert_started();
    assert_eq!(launch.argv()[0], HOOK_TRUST, "{:?}", launch.argv());
    assert_eq!(launch.count(HOOK_TRUST), 2, "{:?}", launch.argv());

    // Under `exec` the same text is the prompt and goes to stdin; the flag
    // amplihack adds is still there, once and first.
    let launch = f.local(NAME, &["exec", "--", HOOK_TRUST]);
    launch.assert_started();
    assert_eq!(launch.argv()[0], HOOK_TRUST, "{:?}", launch.argv());
    assert_eq!(launch.count(HOOK_TRUST), 1, "{:?}", launch.argv());
}

/// AC14: one stderr line names the flag, the profile and the home when
/// amplihack added the flag, and never quotes a user hook command.
#[test]
fn the_hook_trust_notice_names_what_amplihack_added() {
    let f = Fixture::new();
    let home = f.generated(NAME).display().to_string();
    for tail in [&[][..], &["exec", "-"][..]] {
        let launch = f.local(NAME, tail);
        launch.assert_started();
        let stderr = launch.stderr();
        let lines: Vec<&str> = stderr
            .lines()
            .filter(|l| l.contains(HOOK_TRUST) && l.contains(NAME) && l.contains(&home))
            .collect();
        assert_eq!(lines.len(), 1, "{tail:?}: one notice line:\n{stderr}");
        let rest = lines[0].replace(&home, "").replace(HOOK_TRUST, "");
        assert!(
            rest.split(|c: char| !c.is_ascii_digit()).any(|t| t == "2"),
            "{tail:?}: the notice gives the two user handlers carried over: {}",
            lines[0]
        );
        assert!(
            !stderr.contains("codex-guard-adapter") && !stderr.contains("block-tmux-kill"),
            "{tail:?}: no command text on stderr:\n{stderr}"
        );
    }

    let launch = f.local(NAME, &[HOOK_TRUST]);
    launch.assert_started();
    assert!(
        !launch
            .stderr()
            .lines()
            .any(|l| l.contains(HOOK_TRUST) && l.contains(&home)),
        "no notice when the caller passed the flag:\n{}",
        launch.stderr()
    );
}

/// The launch neither installs nor queries the Codex plugin.
#[test]
fn a_local_launch_does_no_plugin_work() {
    let f = Fixture::new();
    f.local(NAME, &["exec", "-"]).assert_started();
    let calls = fs::read_to_string(f.home().join("calls.log")).unwrap_or_default();
    assert!(
        !calls.lines().any(|l| l.starts_with("plugin")),
        "no plugin calls: {calls}"
    );
}
