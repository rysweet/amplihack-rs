//! The generated `config.toml` (D7, D14, SF5, SF9).
//!
//! `config.rs` is pure. The contract held here:
//!
//! - `pub(super) fn parse(text: &str, path: &Path) ->
//!   anyhow::Result<toml_edit::DocumentMut>`, whose error is a fresh message
//!   naming the path and the line, never the source text, and never chained;
//! - `pub(super) fn check_shape(profile: &DocumentMut, path: &Path) ->
//!   anyhow::Result<()>`;
//! - `pub(super) fn render(profile: &DocumentMut, previous:
//!   Option<&DocumentMut>, instructions: &Path) -> anyhow::Result<String>`.
//!
//! The rendered text is compared by its settings (through `toml::Value`),
//! never by layout or wording, except where byte equality is the property
//! (determinism, the fixed point a second refresh relies on).

use super::config::{check_shape, parse, render};
use std::path::Path;
use toml_edit::DocumentMut;

const INSTRUCTIONS: &str = "/home/u/.amplihack/codex-local/inkling/instructions.md";

fn doc(text: &str) -> DocumentMut {
    text.parse().expect("fixture TOML parses")
}

fn rendered(profile: &str, previous: Option<&str>) -> toml::Value {
    let previous = previous.map(doc);
    let text =
        render(&doc(profile), previous.as_ref(), Path::new(INSTRUCTIONS)).expect("render succeeds");
    toml::from_str(&text).unwrap_or_else(|e| panic!("rendered TOML parses ({e}):\n{text}"))
}

const PROFILE: &str = r#"model = "inkling-2-mini"
model_provider = "llamacpp-local"
model_context_window = 65536

[model_providers.llamacpp-local]
name = "llama.cpp (local)"
base_url = "http://127.0.0.1:8080/v1"
wire_api = "responses"
"#;

// ---- step 1: the profile, minus what may not reach the generated home ----

#[test]
fn the_profile_is_copied() {
    let config = rendered(PROFILE, None);
    assert_eq!(config["model"].as_str(), Some("inkling-2-mini"));
    assert_eq!(config["model_provider"].as_str(), Some("llamacpp-local"));
    assert_eq!(config["model_context_window"].as_integer(), Some(65536));
    assert_eq!(
        config["model_providers"]["llamacpp-local"]["base_url"].as_str(),
        Some("http://127.0.0.1:8080/v1")
    );
}

#[test]
fn marketplaces_plugins_projects_hooks_and_profiles_are_removed() {
    let profile = format!(
        r#"profile = "hosted"
{PROFILE}
[profiles.hosted]
model = "gpt-5"
web_search = "live"

[marketplaces.amplihack-local]
source = "/m"

[plugins."amplihack@amplihack-local"]
enabled = true

[projects."/work"]
trust_level = "trusted"

[hooks.state."/x"]
trusted_hash = "from-the-profile"

[[hooks.PreToolUse]]
command = "echo profile-hook"
"#
    );
    let config = rendered(&profile, None);
    for key in [
        "marketplaces",
        "plugins",
        "projects",
        "hooks",
        "profile",
        "profiles",
    ] {
        assert!(config.get(key).is_none(), "{key} survived: {config:#?}");
    }
}

// ---- step 2: amplihack's settings override the profile ----

#[test]
fn amplihack_settings_beat_the_profile() {
    let profile = format!(
        r#"project_doc_max_bytes = 65536
model_instructions_file = "/elsewhere.md"
web_search = "live"
{PROFILE}
[features]
multi_agent = true
goals = true
view_image = true
plugins = true
apps = true
hooks = false
"#
    );
    let config = rendered(&profile, None);
    assert_eq!(config["project_doc_max_bytes"].as_integer(), Some(0));
    assert_eq!(
        config["model_instructions_file"].as_str(),
        Some(INSTRUCTIONS)
    );
    assert_eq!(config["web_search"].as_str(), Some("disabled"));
    let features = config["features"].as_table().unwrap();
    for off in ["multi_agent", "goals", "view_image", "plugins", "apps"] {
        assert_eq!(features[off].as_bool(), Some(false), "{off}");
    }
    assert_eq!(
        features["hooks"].as_bool(),
        Some(true),
        "hooks forced on (SF5)"
    );
    assert_eq!(
        config["skills"]["bundled"]["enabled"].as_bool(),
        Some(false)
    );
}

#[test]
fn the_settings_are_present_without_any_in_the_profile() {
    let config = rendered(PROFILE, None);
    assert_eq!(config["project_doc_max_bytes"].as_integer(), Some(0));
    assert_eq!(config["web_search"].as_str(), Some("disabled"));
    assert_eq!(
        config["model_instructions_file"].as_str(),
        Some(INSTRUCTIONS)
    );
    assert_eq!(config["features"]["hooks"].as_bool(), Some(true));
    assert_eq!(config["features"]["apps"].as_bool(), Some(false));
    assert_eq!(
        config["skills"]["bundled"]["enabled"].as_bool(),
        Some(false)
    );
}

#[test]
fn features_and_skills_are_merged_not_replaced() {
    for features in [
        "[features]\nsome_future_feature = true\n",
        "features = { some_future_feature = true }\n",
        "features.some_future_feature = true\n",
    ] {
        let profile = if features.starts_with('[') {
            format!("{PROFILE}\n{features}\n[skills.bundled]\nextra = \"kept\"\n")
        } else {
            format!("{features}skills.bundled.extra = \"kept\"\n{PROFILE}")
        };
        let config = rendered(&profile, None);
        assert_eq!(
            config["features"]["some_future_feature"].as_bool(),
            Some(true),
            "{features}"
        );
        assert_eq!(
            config["features"]["hooks"].as_bool(),
            Some(true),
            "{features}"
        );
        assert_eq!(
            config["skills"]["bundled"]["extra"].as_str(),
            Some("kept"),
            "{features}"
        );
        assert_eq!(
            config["skills"]["bundled"]["enabled"].as_bool(),
            Some(false),
            "{features}"
        );
    }
}

/// SF5: `profile`/`profiles` cannot switch a forced setting back, because
/// they do not reach the generated file at all.
#[test]
fn a_profiles_table_cannot_undo_a_forced_setting() {
    let profile = format!(
        "profile = \"loud\"\n{PROFILE}\n[profiles.loud]\nweb_search = \"live\"\nproject_doc_max_bytes = 99\n\n[profiles.loud.features]\nhooks = false\n"
    );
    let config = rendered(&profile, None);
    assert!(config.get("profile").is_none());
    assert!(config.get("profiles").is_none());
    assert_eq!(config["web_search"].as_str(), Some("disabled"));
}

// ---- step 3: approval_policy is a default, not an override (D7) ----

#[test]
fn approval_policy_defaults_to_never() {
    assert_eq!(
        rendered(PROFILE, None)["approval_policy"].as_str(),
        Some("never")
    );
}

#[test]
fn the_profile_approval_policy_is_kept() {
    for policy in ["on-request", "untrusted", "never", "on-failure"] {
        let config = rendered(&format!("approval_policy = \"{policy}\"\n{PROFILE}"), None);
        assert_eq!(config["approval_policy"].as_str(), Some(policy));
    }
}

// ---- step 4: tables Codex wrote are carried over ----

const PREVIOUS: &str = r#"model = "an-old-model"
web_search = "live"
approval_policy = "on-request"

[projects."/work/trusted"]
trust_level = "trusted"

[hooks.state."/work/trusted/.codex/hooks.json:pre_tool_use:0:0"]
trusted_hash = "sha256:abc"

[[hooks.PreToolUse]]
command = "echo written-by-hand"

[notice]
hide_full_access_warning = true

[model_providers.stale]
base_url = "http://stale"
"#;

#[test]
fn projects_hook_state_and_notice_are_carried_over() {
    let config = rendered(PROFILE, Some(PREVIOUS));
    let previous: toml::Value = toml::from_str(PREVIOUS).unwrap();
    assert_eq!(config["projects"], previous["projects"]);
    assert_eq!(config["hooks"]["state"], previous["hooks"]["state"]);
    assert_eq!(config["notice"], previous["notice"]);
}

#[test]
fn nothing_else_is_carried_over() {
    let config = rendered(PROFILE, Some(PREVIOUS));
    assert!(
        config["hooks"].get("PreToolUse").is_none(),
        "hook definitions are never carried over: {config:#?}"
    );
    assert_eq!(config["model"].as_str(), Some("inkling-2-mini"));
    assert_eq!(config["web_search"].as_str(), Some("disabled"));
    assert_eq!(config["approval_policy"].as_str(), Some("never"));
    assert!(config["model_providers"].get("stale").is_none());
}

/// The profile's own `projects` are dropped even when the previous file's
/// are carried.
#[test]
fn carried_projects_come_from_the_previous_file_only() {
    let profile = format!("{PROFILE}\n[projects.\"/from-profile\"]\ntrust_level = \"trusted\"\n");
    let config = rendered(&profile, Some(PREVIOUS));
    let projects = config["projects"].as_table().unwrap();
    assert!(projects.contains_key("/work/trusted"));
    assert!(!projects.contains_key("/from-profile"));
}

// ---- determinism and the fixed point a refresh relies on ----

#[test]
fn rendering_is_deterministic() {
    let profile = doc(PROFILE);
    let previous = doc(PREVIOUS);
    let a = render(&profile, Some(&previous), Path::new(INSTRUCTIONS)).unwrap();
    let b = render(&profile, Some(&previous), Path::new(INSTRUCTIONS)).unwrap();
    assert_eq!(a, b);
}

/// Rendering again over its own output gives the same bytes, so an
/// unchanged home is never rewritten (AC10).
#[test]
fn rendering_over_its_own_output_is_a_fixed_point() {
    for previous in [None, Some(PREVIOUS)] {
        let profile = doc(PROFILE);
        let previous = previous.map(doc);
        let first = render(&profile, previous.as_ref(), Path::new(INSTRUCTIONS)).unwrap();
        let second = render(&profile, Some(&doc(&first)), Path::new(INSTRUCTIONS)).unwrap();
        assert_eq!(second, first);
    }
}

// ---- profile shape (E8) ----

#[test]
fn features_and_skills_must_be_tables() {
    let path = Path::new("/home/u/.codex/inkling.config.toml");
    for (key, text) in [
        ("features", format!("features = \"all\"\n{PROFILE}")),
        ("features", format!("features = [\"hooks\"]\n{PROFILE}")),
        ("features", format!("features = 1\n{PROFILE}")),
        ("skills", format!("skills = true\n{PROFILE}")),
        ("skills", format!("skills = [1]\n{PROFILE}")),
        (
            "bundled",
            format!("skills = {{ bundled = true }}\n{PROFILE}"),
        ),
        ("bundled", format!("skills.bundled = \"yes\"\n{PROFILE}")),
        ("bundled", format!("{PROFILE}\n[skills]\nbundled = 3\n")),
    ] {
        let err = check_shape(&doc(&text), path).expect_err(&text);
        let message = format!("{err:#}");
        assert!(message.contains(&path.display().to_string()), "{message}");
        assert!(message.contains(key), "{key}: {message}");
    }
}

#[test]
fn table_shapes_of_every_spelling_are_accepted() {
    let path = Path::new("/p.toml");
    for text in [
        PROFILE.to_string(),
        format!("{PROFILE}\n[features]\nhooks = true\n\n[skills.bundled]\nenabled = true\n"),
        format!(
            "features = {{ goals = true }}\nskills = {{ bundled = {{ enabled = true }} }}\n{PROFILE}"
        ),
        format!("features.goals = true\nskills.bundled.enabled = true\n{PROFILE}"),
        format!("{PROFILE}\n[skills]\nother = 1\n"),
    ] {
        check_shape(&doc(&text), path).unwrap_or_else(|e| panic!("{e:#}\n{text}"));
    }
}

/// `render` is itself a guard: it refuses a shape it cannot merge.
#[test]
fn render_refuses_a_bad_shape() {
    let profile = doc(&format!("features = \"all\"\n{PROFILE}"));
    assert!(render(&profile, None, Path::new(INSTRUCTIONS)).is_err());
}

// ---- parse errors carry no source text (SF9) ----

#[test]
fn a_parse_error_names_the_file_and_quotes_nothing() {
    let path = Path::new("/home/u/.codex/inkling.config.toml");
    let secret = "sk-SENTINEL-config-tests-0123456789";
    for text in [
        format!("model = \"{secret}\n"),
        format!("model = \"m\"\nmodel = \"{secret}\"\n"),
        format!("[model_providers.x]\ntoken = \"{secret}\"\n[model_providers.x]\n"),
        format!("{secret} = = \n"),
    ] {
        let err = parse(&text, path).expect_err(&text);
        for rendered in [format!("{err}"), format!("{err:#}"), format!("{err:?}")] {
            assert!(
                !rendered.contains(secret),
                "source text in the error: {rendered}"
            );
            assert!(
                !rendered.contains("token ="),
                "source text in the error: {rendered}"
            );
        }
        assert!(format!("{err:#}").contains(&path.display().to_string()));
        assert_eq!(
            err.chain().count(),
            1,
            "a fresh error, never chained: {err:?}"
        );
    }
}

#[test]
fn parse_accepts_valid_toml() {
    let parsed = parse(PROFILE, Path::new("/p.toml")).unwrap();
    assert_eq!(parsed["model"].as_str(), Some("inkling-2-mini"));
}
