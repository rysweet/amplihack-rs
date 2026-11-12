use super::name::ProfileName;
use super::store::ProfileStore;
use super::{
    PROFILE_ENV, ProfileError, escape_for_error, launch_profile_dir_with, pointer, render_list,
    render_show, resolve_launch_profile, validate_launch_flags,
};
use std::fs;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn store() -> (tempfile::TempDir, ProfileStore) {
    let d = tempfile::tempdir().unwrap();
    let s = ProfileStore::new(d.path().join("root"));
    (d, s)
}

#[test]
fn list_marks_default_with_star() {
    let (_d, s) = store();
    s.add(&n("alpha")).unwrap();
    s.add(&n("beta")).unwrap();
    pointer::write(s.root(), &n("beta")).unwrap();
    assert_eq!(
        render_list(&s).unwrap(),
        "  NAME   KIND\n  alpha  subscription\n* beta   subscription\n"
    );
}

#[test]
fn list_reports_a_corrupt_default_pointer_instead_of_hiding_it() {
    let (_d, s) = store();
    s.add(&n("alpha")).unwrap();
    fs::write(s.root().join(".default"), "../outside").unwrap();

    assert!(matches!(
        render_list(&s),
        Err(ProfileError::PointerCorrupt(_))
    ));
}

#[test]
fn list_reports_a_dangling_default_pointer_instead_of_hiding_it() {
    let (_d, s) = store();
    s.add(&n("alpha")).unwrap();
    fs::write(s.root().join(".default"), "missing\n").unwrap();

    assert!(matches!(
        render_list(&s),
        Err(ProfileError::PointerDangling(_))
    ));
}

#[test]
fn show_prints_none_without_pointer() {
    let (_d, s) = store();
    assert_eq!(render_show(&s).unwrap().trim(), "none");
}

#[test]
fn show_prints_name_and_dir_only_never_contents() {
    let (_d, s) = store();
    let dir = s.add(&n("work")).unwrap();
    fs::write(dir.join(".credentials.json"), "SECRET-TOKEN").unwrap();
    pointer::write(s.root(), &n("work")).unwrap();
    let out = render_show(&s).unwrap();
    assert!(out.contains("name: work"));
    assert!(out.contains(&format!(
        "dir: {}",
        s.resolve(&n("work")).unwrap().display()
    )));
    assert!(!out.contains("SECRET-TOKEN") && !out.contains(".credentials.json"));
}

#[test]
fn resolve_launch_profile_precedence() {
    let (_d, s) = store();
    s.add(&n("flag")).unwrap();
    s.add(&n("ptr")).unwrap();
    assert!(resolve_launch_profile(&s, None, None).unwrap().is_none());
    pointer::write(s.root(), &n("ptr")).unwrap();
    let (name, dir) = resolve_launch_profile(&s, None, None).unwrap().unwrap();
    assert_eq!(name, n("ptr"));
    assert!(dir.ends_with("ptr"));
    let (name, _) = resolve_launch_profile(&s, Some("flag"), None)
        .unwrap()
        .unwrap();
    assert_eq!(name, n("flag"));
}

#[test]
fn explicit_flag_ignores_corrupt_or_dangling_pointer() {
    let (_d, s) = store();
    s.add(&n("flag")).unwrap();
    fs::write(s.root().join(".default"), "gone").unwrap();
    assert!(resolve_launch_profile(&s, Some("flag"), None).is_ok());
    assert!(matches!(
        resolve_launch_profile(&s, None, None),
        Err(ProfileError::PointerDangling(_))
    ));
}

#[test]
fn unknown_or_invalid_flag_value_errors() {
    let (_d, s) = store();
    assert!(matches!(
        resolve_launch_profile(&s, Some("nope"), None),
        Err(ProfileError::NotFound(_))
    ));
    assert!(matches!(
        resolve_launch_profile(&s, Some("../x"), None),
        Err(ProfileError::InvalidName(_))
    ));
}

#[test]
fn profile_with_docker_or_auto_is_unsupported() {
    assert!(matches!(
        validate_launch_flags(Some("w"), true, false, false),
        Err(ProfileError::UnsupportedMode(_))
    ));
    assert!(matches!(
        validate_launch_flags(Some("w"), false, false, true),
        Err(ProfileError::UnsupportedMode(_))
    ));
    assert!(validate_launch_flags(Some("w"), false, false, false).is_ok());
    assert!(validate_launch_flags(None, true, true, true).is_ok());
}

#[test]
fn errors_have_one_line_message_and_a_hint() {
    let errs = [
        ProfileError::InvalidName("x".into()),
        ProfileError::NotFound("x".into()),
        ProfileError::AlreadyExists("x".into()),
        ProfileError::ConfirmationRequired,
        ProfileError::UnsupportedMode("docker".into()),
        ProfileError::PointerDangling("x".into()),
        ProfileError::PointerCorrupt("x".into()),
        ProfileError::InvalidRoot("x".into()),
    ];
    for e in errs {
        assert!(!e.to_string().contains('\n'), "{e:?}");
        assert!(!e.hint().is_empty(), "{e:?}");
    }
    assert!(ProfileError::ConfirmationRequired.hint().contains("--yes"));
}

#[test]
fn profile_flag_with_env_activated_docker_is_an_error() {
    assert!(matches!(
        validate_launch_flags(Some("w"), false, true, false),
        Err(ProfileError::UnsupportedMode(m)) if m.contains("env-activated")
    ));
    assert!(validate_launch_flags(None, false, true, false).is_ok());
}

// ---------------------------------------------------------------------------
// Issue #1567: an inherited AMPLIHACK_CLAUDE_PROFILE pins nested launches.
// Order: --profile flag > inherited name > default pointer > none.
// ---------------------------------------------------------------------------

const OAUTH_LOGIN: &str = r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#;

fn store_with(names: &[&str]) -> (tempfile::TempDir, ProfileStore) {
    let (d, s) = store();
    for name in names {
        let dir = s.add(&n(name)).unwrap();
        // Issue #1700: an OAuth login, so the launch guard lets it start;
        // private, as Claude writes it, since one group or others could
        // write counts as unreadable whatever the umask.
        let login = dir.join(".credentials.json");
        fs::write(&login, r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&login, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    (d, s)
}

fn resolved_name(r: Option<(ProfileName, std::path::PathBuf)>) -> String {
    r.expect("a profile must resolve").0.as_str().to_string()
}

fn no_store() -> Result<ProfileStore, ProfileError> {
    panic!("the profile store must not be opened on this path")
}

#[test]
fn profile_env_names_the_inherited_variable() {
    assert_eq!(PROFILE_ENV, "AMPLIHACK_CLAUDE_PROFILE");
}

#[test]
fn resolution_covers_all_four_tiers() {
    let (_d, s) = store_with(&["flag", "inh", "ptr"]);
    // none
    assert!(resolve_launch_profile(&s, None, None).unwrap().is_none());
    // pointer
    pointer::write(s.root(), &n("ptr")).unwrap();
    assert_eq!(
        resolved_name(resolve_launch_profile(&s, None, None).unwrap()),
        "ptr"
    );
    // inherited
    assert_eq!(
        resolved_name(resolve_launch_profile(&s, None, Some("inh")).unwrap()),
        "inh"
    );
    // flag
    assert_eq!(
        resolved_name(resolve_launch_profile(&s, Some("flag"), Some("inh")).unwrap()),
        "flag"
    );
}

#[test]
fn flag_beats_inherited_and_pointer() {
    let (_d, s) = store_with(&["flag", "inh", "ptr"]);
    pointer::write(s.root(), &n("ptr")).unwrap();
    let (name, dir) = resolve_launch_profile(&s, Some("flag"), Some("inh"))
        .unwrap()
        .unwrap();
    assert_eq!(name, n("flag"));
    assert_eq!(dir, s.resolve(&n("flag")).unwrap());
}

#[test]
fn flag_never_reads_an_invalid_or_missing_inherited_value() {
    let (_d, s) = store_with(&["flag"]);
    for bad in ["../x", " X", "missing"] {
        assert_eq!(
            resolved_name(resolve_launch_profile(&s, Some("flag"), Some(bad)).unwrap()),
            "flag",
            "{bad:?}"
        );
    }
}

#[test]
fn inherited_beats_pointer() {
    let (_d, s) = store_with(&["inh", "ptr"]);
    pointer::write(s.root(), &n("ptr")).unwrap();
    let (name, dir) = resolve_launch_profile(&s, None, Some("inh"))
        .unwrap()
        .unwrap();
    assert_eq!(name, n("inh"));
    assert_eq!(dir, s.resolve(&n("inh")).unwrap());
}

#[test]
fn inherited_never_reads_a_dangling_or_corrupt_pointer() {
    let (_d, s) = store_with(&["inh"]);
    for ptr in ["gone\n", "../outside"] {
        fs::write(s.root().join(".default"), ptr).unwrap();
        assert_eq!(
            resolved_name(resolve_launch_profile(&s, None, Some("inh")).unwrap()),
            "inh",
            "pointer {ptr:?} must not be read"
        );
    }
}

#[test]
fn empty_inherited_value_counts_as_unset() {
    let (_d, s) = store_with(&["ptr"]);
    assert!(
        resolve_launch_profile(&s, None, Some(""))
            .unwrap()
            .is_none()
    );
    pointer::write(s.root(), &n("ptr")).unwrap();
    assert_eq!(
        resolved_name(resolve_launch_profile(&s, None, Some("")).unwrap()),
        "ptr"
    );
}

#[test]
fn invalid_inherited_value_is_an_error_naming_the_variable() {
    let (_d, s) = store_with(&["X", "ptr"]);
    pointer::write(s.root(), &n("ptr")).unwrap();
    // Never trimmed: " X" is not "X".
    for bad in [" X", "X ", "../x", "x/y", "a\nb", "."] {
        let err = resolve_launch_profile(&s, None, Some(bad)).unwrap_err();
        assert!(
            matches!(err, ProfileError::Inherited { .. }),
            "{bad:?} gave {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("AMPLIHACK_CLAUDE_PROFILE"), "{msg}");
        assert!(!msg.contains('\n'), "one-line message: {msg:?}");
    }
}

#[test]
fn missing_inherited_profile_fails_closed_without_reading_dangling_pointer() {
    let (_d, s) = store();
    fs::create_dir_all(s.root()).unwrap();
    fs::write(s.root().join(".default"), "gone").unwrap();
    let err = resolve_launch_profile(&s, None, Some("Missing")).unwrap_err();
    assert!(matches!(err, ProfileError::Inherited { .. }), "{err:?}");
    assert!(err.to_string().contains("\"Missing\""), "{err}");
}

#[test]
fn bad_inherited_value_never_falls_back_to_a_valid_pointer() {
    let (_d, s) = store_with(&["ptr"]);
    pointer::write(s.root(), &n("ptr")).unwrap();
    for bad in ["../x", "Missing"] {
        assert!(
            matches!(
                resolve_launch_profile(&s, None, Some(bad)),
                Err(ProfileError::Inherited { .. })
            ),
            "{bad:?} must fail closed, not use the pointer"
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinked_inherited_profile_is_refused() {
    let (d, s) = store_with(&["real"]);
    let outside = d.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, s.root().join("link")).unwrap();
    assert!(matches!(
        resolve_launch_profile(&s, None, Some("link")),
        Err(ProfileError::Inherited { .. })
    ));
}

#[cfg(unix)]
#[test]
fn inherited_profile_with_open_permissions_fails_spawn_time_check() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, s) = store_with(&["inh"]);
    let (name, dir) = resolve_launch_profile(&s, None, Some("inh"))
        .unwrap()
        .unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        s.verify_profile_dir(&name),
        Err(ProfileError::UnsafePermissions(_))
    ));
}

#[test]
fn inherited_error_has_one_line_message_and_actionable_hint() {
    let (_d, s) = store();
    let err = resolve_launch_profile(&s, None, Some("Missing")).unwrap_err();
    assert!(!err.to_string().contains('\n'));
    let hint = err.hint();
    assert!(hint.contains("unset AMPLIHACK_CLAUDE_PROFILE"), "{hint}");
    assert!(hint.contains("amplihack profile add"), "{hint}");
}

#[cfg(unix)]
#[test]
fn inherited_profile_on_unreadable_root_names_the_io_cause() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return; // root ignores mode bits
    }
    let (_d, s) = store_with(&["inh"]);
    fs::set_permissions(s.root(), fs::Permissions::from_mode(0o000)).unwrap();
    let err = resolve_launch_profile(&s, None, Some("inh")).unwrap_err();
    fs::set_permissions(s.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let msg = err.to_string();
    assert!(msg.contains("i/o error on profile root"), "{msg}");
    assert!(msg.contains("permission denied"), "{msg}");
    assert_eq!(err.hint(), "check permissions on the profile root");
}

#[test]
fn inherited_unsafe_permissions_keeps_the_cause_and_its_hint() {
    let err = super::inherited_error("inh", ProfileError::UnsafePermissions("inh".into()));
    let msg = err.to_string();
    assert!(msg.contains("unsafe permissions"), "{msg}");
    assert!(!msg.contains('\n'), "{msg:?}");
    let hint = err.hint();
    assert!(hint.contains("chmod go-rwx"), "{hint}");
    assert!(!hint.contains("profile add"), "{hint}");
}

#[test]
fn inherited_error_escapes_control_bytes_and_stays_short() {
    let (_d, s) = store();
    let mut raw = String::from("\x1b[2J\n\r");
    raw.push_str(&"a".repeat(100 * 1024));
    let err = resolve_launch_profile(&s, None, Some(&raw)).unwrap_err();
    assert!(matches!(err, ProfileError::Inherited { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(msg.len() < 300, "message is {} bytes", msg.len());
    assert!(
        !msg.bytes().any(|b| b < 0x20 || b == 0x7f),
        "raw control bytes in {msg:?}"
    );
    assert!(msg.contains("\\u{1b}"), "escape must be visible: {msg}");
}

#[test]
fn escape_inherited_cuts_at_64_chars_without_splitting_utf8() {
    assert_eq!(escape_for_error("work"), "\"work\"");
    // 70 two-byte chars: a byte slice at 64 would land mid-character.
    let long = "é".repeat(70);
    let out = escape_for_error(&long);
    assert!(out.ends_with('…'), "{out}");
    assert_eq!(out.chars().count(), 65, "{out}");
    let short = "é".repeat(10);
    assert!(!escape_for_error(&short).ends_with('…'));
}

#[test]
fn launch_with_inherited_resolves_the_inherited_dir() {
    let (_d, s) = store_with(&["inh", "ptr"]);
    pointer::write(s.root(), &n("ptr")).unwrap();
    let want = s.resolve(&n("inh")).unwrap();
    let got = launch_profile_dir_with(
        None,
        Some(std::ffi::OsStr::new("inh")),
        false,
        false,
        false,
        None,
        || Ok(s),
    )
    .unwrap();
    assert_eq!(got, Some(want));
}

#[test]
fn launch_with_empty_inherited_uses_pointer() {
    let (_d, s) = store_with(&["ptr"]);
    pointer::write(s.root(), &n("ptr")).unwrap();
    let want = s.resolve(&n("ptr")).unwrap();
    let got = launch_profile_dir_with(
        None,
        Some(std::ffi::OsStr::new("")),
        false,
        false,
        false,
        None,
        || Ok(s),
    )
    .unwrap();
    assert_eq!(got, Some(want));
}

/// (docker, docker_env, auto) for `--docker`, env-activated Docker, `--auto`.
const UNSUPPORTED_MODES: [(bool, bool, bool); 3] = [
    (true, false, false),
    (false, true, false),
    (false, false, true),
];

fn is_inherited_unsupported(r: &Result<Option<std::path::PathBuf>, ProfileError>) -> bool {
    matches!(
        r,
        Err(ProfileError::Inherited { source, .. })
            if matches!(**source, ProfileError::UnsupportedMode(_))
    )
}

// D1: the requirements override the WIP design. An inherited profile under
// Docker or --auto is refused (fail closed), not silently dropped, so a
// hand-set AMPLIHACK_CLAUDE_PROFILE never runs on the default account.
#[test]
fn inherited_with_auto_or_docker_is_refused_without_opening_the_store() {
    for value in ["work", "../x", "Missing"] {
        let v = Some(std::ffi::OsStr::new(value));
        for (docker, docker_env, auto) in UNSUPPORTED_MODES {
            let r = launch_profile_dir_with(None, v, docker, docker_env, auto, None, no_store);
            assert!(
                is_inherited_unsupported(&r),
                "{value:?} docker={docker} docker_env={docker_env} auto={auto}: {r:?}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_inherited_under_docker_or_auto_is_refused_before_parsing() {
    use std::os::unix::ffi::OsStrExt;
    let raw = std::ffi::OsStr::from_bytes(b"\xff");
    for (docker, docker_env, auto) in UNSUPPORTED_MODES {
        let r = launch_profile_dir_with(None, Some(raw), docker, docker_env, auto, None, no_store);
        assert!(
            is_inherited_unsupported(&r),
            "must be UnsupportedMode, not InvalidName: {r:?}"
        );
    }
}

#[test]
fn empty_inherited_under_docker_or_auto_counts_as_unset() {
    let empty = Some(std::ffi::OsStr::new(""));
    // --docker and --auto never open the store when nothing is pinned.
    for (docker, auto) in [(true, false), (false, true)] {
        assert!(
            launch_profile_dir_with(None, empty, docker, false, auto, None, no_store)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn pointer_alone_under_docker_or_auto_is_still_skipped() {
    for (docker, auto) in [(true, false), (false, true)] {
        assert!(
            launch_profile_dir_with(None, None, docker, false, auto, None, no_store)
                .unwrap()
                .is_none()
        );
    }
    // Env-activated Docker: the pointer resolves, then is warned about and skipped.
    let (_d, s) = store_with(&["ptr"]);
    pointer::write(s.root(), &n("ptr")).unwrap();
    assert!(
        launch_profile_dir_with(None, None, false, true, false, None, || Ok(s))
            .unwrap()
            .is_none()
    );
}

#[test]
fn inherited_unsupported_mode_error_names_the_variable_and_hint() {
    let v = Some(std::ffi::OsStr::new("work"));
    let err = launch_profile_dir_with(None, v, false, false, true, None, no_store).unwrap_err();
    let msg = err.to_string();
    assert!(!msg.contains('\n'), "{msg}");
    assert!(msg.contains("AMPLIHACK_CLAUDE_PROFILE"), "{msg}");
    assert!(msg.contains("\"work\""), "{msg}");
    assert!(
        msg.contains("not applied with --docker, --auto or Docker"),
        "{msg}"
    );
    assert_eq!(
        err.hint(),
        "unset AMPLIHACK_CLAUDE_PROFILE and CLAUDE_CONFIG_DIR to run with --docker/--auto"
    );
}

#[test]
fn inherited_unsupported_mode_error_escapes_and_cuts_the_value() {
    let mut raw = String::from("\x1b[31m\nwork");
    raw.push_str(&"a".repeat(200));
    let v = Some(std::ffi::OsStr::new(&raw));
    for (docker, docker_env, auto) in UNSUPPORTED_MODES {
        let err =
            launch_profile_dir_with(None, v, docker, docker_env, auto, None, no_store).unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.bytes().any(|b| b < 0x20 || b == 0x7f),
            "raw control bytes in {msg:?}"
        );
        let escaped = escape_for_error(&raw);
        assert_eq!(msg.matches(escaped.as_str()).count(), 1, "{msg}");
        assert!(msg.len() < 200, "message is {} bytes", msg.len());
    }
}

// D5: only an empty value counts as unset; whitespace is not trimmed.
#[test]
fn whitespace_only_inherited_value_is_an_invalid_name() {
    let (_d, s) = store_with(&["ptr"]);
    pointer::write(s.root(), &n("ptr")).unwrap();
    for raw in [" ", "\t", " work "] {
        let err = resolve_launch_profile(&s, None, Some(raw)).unwrap_err();
        assert!(
            matches!(&err, ProfileError::Inherited { source, .. }
                if matches!(**source, ProfileError::InvalidName(_))),
            "{raw:?}: {err:?}"
        );
    }
}

#[test]
fn not_found_inherited_hint_is_unchanged_by_the_mode_hint() {
    let (_d, s) = store();
    let err = resolve_launch_profile(&s, None, Some("Missing")).unwrap_err();
    assert!(!err.hint().contains("--docker/--auto"), "{}", err.hint());
}

#[test]
fn explicit_flag_with_docker_or_auto_is_still_unsupported() {
    let inh = Some(std::ffi::OsStr::new("inh"));
    for (docker, docker_env, auto) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        assert!(matches!(
            launch_profile_dir_with(Some("w"), inh, docker, docker_env, auto, None, no_store),
            Err(ProfileError::UnsupportedMode(_))
        ));
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_inherited_value_is_an_inherited_error() {
    use std::os::unix::ffi::OsStrExt;
    let (_d, s) = store_with(&["ptr"]);
    pointer::write(s.root(), &n("ptr")).unwrap();
    let raw = std::ffi::OsStr::from_bytes(b"w\xffx");
    let err =
        launch_profile_dir_with(None, Some(raw), false, false, false, None, || Ok(s)).unwrap_err();
    assert!(matches!(err, ProfileError::Inherited { .. }), "{err:?}");
    assert!(err.to_string().contains("AMPLIHACK_CLAUDE_PROFILE"));
}

// --- Bug 3: launching the rotator's live profile ----------------------------
//
// Contract: `launch_profile_dir_with(flag, inherited, docker, docker_env,
// auto, home: Option<&Path>, open_store)`. When the resolved profile is the
// one `default_live::detect` names, it prints `live_launch_note(name)` once on
// stderr and returns `Ok(None)`: the child gets no `CLAUDE_CONFIG_DIR` and no
// profile pin, so Claude uses `~/.claude`, where that profile's login is.

#[cfg(unix)]
mod live_launch {
    use super::super::live_launch_note;
    use super::{launch_profile_dir_with, n, no_store, pointer, store_with};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn private(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn acct(uuid: &str, org: &str) -> String {
        serde_json::json!({"oauthAccount": {"accountUuid": uuid, "organizationUuid": org}})
            .to_string()
    }

    /// `team` is live: no login of its own, and its account is the one in
    /// `~/.claude.json`. `work` has its own login.
    fn env() -> (
        tempfile::TempDir,
        super::super::store::ProfileStore,
        PathBuf,
    ) {
        let (d, s) = store_with(&["team", "work"]);
        // `team`'s login is the one in ~/.claude (issue #1700: OAuth logins).
        fs::remove_file(s.root().join("team/.credentials.json")).unwrap();
        let home = d.path().join("home");
        private(&home.join(".claude/.credentials.json"), super::OAUTH_LOGIN);
        private(&home.join(".claude.json"), &acct("u-team", "o1"));
        private(&s.root().join("team/.claude.json"), &acct("u-team", "o1"));
        private(&s.root().join("work/.claude.json"), &acct("u-work", "o1"));
        private(&s.root().join("work/.credentials.json"), super::OAUTH_LOGIN);
        (d, s, home)
    }

    #[test]
    fn the_live_default_pointer_launches_against_the_default_dir() {
        let (_d, s, home) = env();
        pointer::write(s.root(), &n("team")).unwrap();
        let got = launch_profile_dir_with(None, None, false, false, false, Some(&home), || Ok(s));
        assert_eq!(got.unwrap(), None);
    }

    #[test]
    fn a_live_profile_flag_launches_against_the_default_dir() {
        let (_d, s, home) = env();
        let got =
            launch_profile_dir_with(Some("team"), None, false, false, false, Some(&home), || {
                Ok(s)
            });
        assert_eq!(got.unwrap(), None);
    }

    #[test]
    fn a_live_inherited_profile_launches_against_the_default_dir() {
        let (_d, s, home) = env();
        let got = launch_profile_dir_with(
            None,
            Some(std::ffi::OsStr::new("team")),
            false,
            false,
            false,
            Some(&home),
            || Ok(s),
        );
        assert_eq!(got.unwrap(), None);
    }

    #[test]
    fn a_default_that_is_not_live_still_sets_its_dir() {
        let (_d, s, home) = env();
        pointer::write(s.root(), &n("work")).unwrap();
        let want = s.resolve(&n("work")).unwrap();
        let got = launch_profile_dir_with(None, None, false, false, false, Some(&home), || Ok(s));
        assert_eq!(got.unwrap(), Some(want));
    }

    #[test]
    fn the_same_user_in_another_org_is_not_live_and_is_not_redirected() {
        let (_d, s, home) = env();
        private(
            &s.root().join("team/.claude.json"),
            &acct("u-team", "o-other"),
        );
        pointer::write(s.root(), &n("team")).unwrap();
        let got = launch_profile_dir_with(None, None, false, false, false, Some(&home), || Ok(s));
        // Not redirected, and `team` has no login of its own: the launch
        // guard (issue #1700) refuses it rather than bill the API key.
        assert!(
            matches!(got, Err(super::ProfileError::SubscriptionNoLogin(ref p)) if p == "team"),
            "{got:?}"
        );
    }

    #[test]
    fn without_home_nothing_is_redirected() {
        let (_d, s, _home) = env();
        pointer::write(s.root(), &n("team")).unwrap();
        let got = launch_profile_dir_with(None, None, false, false, false, None, || Ok(s));
        // Not redirected, and `team` has no login of its own: the launch
        // guard (issue #1700) refuses it rather than bill the API key.
        assert!(
            matches!(got, Err(super::ProfileError::SubscriptionNoLogin(ref p)) if p == "team"),
            "{got:?}"
        );
    }

    #[test]
    fn ambiguous_detection_is_not_redirected() {
        let (_d, s, home) = env();
        s.add(&n("twin")).unwrap();
        private(&s.root().join("twin/.claude.json"), &acct("u-team", "o1"));
        pointer::write(s.root(), &n("team")).unwrap();
        let got = launch_profile_dir_with(None, None, false, false, false, Some(&home), || Ok(s));
        // Not redirected, and `team` has no login of its own: the launch
        // guard (issue #1700) refuses it rather than bill the API key.
        assert!(
            matches!(got, Err(super::ProfileError::SubscriptionNoLogin(ref p)) if p == "team"),
            "{got:?}"
        );
    }

    #[test]
    fn docker_and_auto_are_unchanged() {
        let (_d, s, home) = env();
        pointer::write(s.root(), &n("team")).unwrap();
        for (docker, auto) in [(true, false), (false, true)] {
            assert_eq!(
                launch_profile_dir_with(None, None, docker, false, auto, Some(&home), no_store)
                    .unwrap(),
                None
            );
        }
        assert_eq!(
            launch_profile_dir_with(None, None, false, true, false, Some(&home), || Ok(s)).unwrap(),
            None,
            "env-activated Docker still skips the pointer"
        );
    }

    #[test]
    fn the_note_is_the_documented_line() {
        assert_eq!(
            live_launch_note("team"),
            "note: profile 'team' is the rotator's live login, which is in ~/.claude; \
             launching against ~/.claude."
        );
        assert!(!live_launch_note("team").contains('\n'), "exactly one line");
    }
}

// --- `profile switch --none` (D6-D8) ----------------------------------------
//
// Contract: `clear_default(&ProfileStore) -> Result<String, ProfileError>`
// removes `<root>/.default` and returns `CLEARED` when something was there
// (valid, corrupt or dangling), `NOT_SET` when nothing was. Issue #1691: on
// Unix it first removes the rotation pin under the rotate state lock, and
// when a pin was removed it adds a second line, `RELEASED`. A busy lock is
// `RotateRefused` and changes neither file.

mod switch_none {
    use super::super::{CLEARED, NOT_SET, ProfileCommands, clear_default};
    use super::{n, pointer, store_with};
    use clap::Parser;
    use std::fs;

    #[derive(Parser, Debug)]
    struct Cli {
        #[command(subcommand)]
        cmd: ProfileCommands,
    }

    fn parse(args: &[&str]) -> Result<ProfileCommands, clap::Error> {
        let mut v = vec!["profile"];
        v.extend_from_slice(args);
        Cli::try_parse_from(v).map(|c| c.cmd)
    }

    #[test]
    fn the_messages_are_the_documented_ones() {
        assert_eq!(CLEARED, "Default profile cleared.");
        assert_eq!(NOT_SET, "No default profile was set.");
    }

    #[test]
    fn none_clears_a_valid_default() {
        let (_d, s) = store_with(&["work"]);
        pointer::write(s.root(), &n("work")).unwrap();
        assert_eq!(clear_default(&s).unwrap(), CLEARED);
        assert!(!s.root().join(".default").exists());
        assert_eq!(pointer::read(s.root()).unwrap(), None);
        assert!(s.root().join("work").is_dir(), "the profile itself stays");
    }

    #[test]
    fn none_clears_a_corrupt_or_dangling_default() {
        for body in ["../outside", "missing\n"] {
            let (_d, s) = store_with(&["work"]);
            fs::write(s.root().join(".default"), body).unwrap();
            assert_eq!(clear_default(&s).unwrap(), CLEARED, "{body:?}");
            assert!(fs::symlink_metadata(s.root().join(".default")).is_err());
        }
    }

    #[test]
    fn none_without_a_default_is_a_quiet_success() {
        let (_d, s) = store_with(&["work"]);
        assert_eq!(clear_default(&s).unwrap(), NOT_SET);
        assert_eq!(clear_default(&s).unwrap(), NOT_SET, "idempotent");
    }

    #[cfg(unix)]
    #[test]
    fn none_refuses_a_symlinked_root() {
        let (d, s) = store_with(&["work"]);
        let link = d.path().join("link");
        std::os::unix::fs::symlink(s.root(), &link).unwrap();
        let linked = super::super::store::ProfileStore::new(link);
        pointer::write(s.root(), &n("work")).unwrap();
        assert!(clear_default(&linked).is_err());
        assert!(s.root().join(".default").exists());
    }

    #[cfg(unix)]
    const RELEASED: &str = "Rotation released; the rotator picks the best login on its next cycle.";

    /// Writes `<root>/.auto-switch.d/rotate/pin` as `profile switch` would.
    #[cfg(unix)]
    fn pin_at(root: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = root.join(".auto-switch.d").join("rotate");
        fs::create_dir_all(&dir).unwrap();
        for d in [root.join(".auto-switch.d"), dir.clone()] {
            fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let pin = dir.join("pin");
        fs::write(&pin, body).unwrap();
        fs::set_permissions(&pin, fs::Permissions::from_mode(0o600)).unwrap();
        pin
    }

    #[cfg(unix)]
    #[test]
    fn none_releases_a_pin_and_says_so() {
        let (_d, s) = store_with(&["work"]);
        pointer::write(s.root(), &n("work")).unwrap();
        let pin = pin_at(s.root(), "work\n");
        let out = clear_default(&s).unwrap();
        assert_eq!(out, format!("{CLEARED}\n{RELEASED}"));
        assert!(fs::symlink_metadata(&pin).is_err(), "the pin is removed");
        assert!(!s.root().join(".default").exists());
        assert_eq!(
            clear_default(&s).unwrap(),
            NOT_SET,
            "nothing left to release"
        );
    }

    #[cfg(unix)]
    #[test]
    fn none_releases_a_pin_even_without_a_default() {
        let (_d, s) = store_with(&["work"]);
        let pin = pin_at(s.root(), "garbage \x1b[0m");
        assert_eq!(clear_default(&s).unwrap(), format!("{NOT_SET}\n{RELEASED}"));
        assert!(fs::symlink_metadata(&pin).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn none_under_a_busy_rotate_lock_changes_nothing() {
        use super::super::auto_switch::kit::State;
        let (_d, s) = store_with(&["work"]);
        pointer::write(s.root(), &n("work")).unwrap();
        let pin = pin_at(s.root(), "work\n");
        let default_before = fs::read(s.root().join(".default")).unwrap();
        let state = State::new(s.root()).unwrap();
        let _held = state
            .try_lock(std::time::Duration::ZERO)
            .unwrap()
            .expect("the test holds the lock");
        match clear_default(&s) {
            Err(super::super::ProfileError::RotateRefused(m)) => {
                assert!(m.contains("lock busy"), "{m}")
            }
            other => panic!("expected RotateRefused, got {other:?}"),
        }
        assert_eq!(fs::read(&pin).unwrap(), b"work\n");
        assert_eq!(fs::read(s.root().join(".default")).unwrap(), default_before);
    }

    #[test]
    fn switch_parses_a_name_or_none() {
        match parse(&["switch", "--none"]).unwrap() {
            ProfileCommands::Switch { name, none } => {
                assert_eq!(name, None);
                assert!(none);
            }
            other => panic!("{other:?}"),
        }
        match parse(&["switch", "work"]).unwrap() {
            ProfileCommands::Switch { name, none } => {
                assert_eq!(name.as_deref(), Some("work"));
                assert!(!none);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn default_is_a_profile_name_not_an_alias() {
        // D6: `default` is a valid profile name.
        match parse(&["switch", "default"]).unwrap() {
            ProfileCommands::Switch { name, none } => {
                assert_eq!(name.as_deref(), Some("default"));
                assert!(!none);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_name_with_none_or_neither_is_a_usage_error() {
        let both = parse(&["switch", "work", "--none"]).unwrap_err();
        assert_eq!(both.kind(), clap::error::ErrorKind::ArgumentConflict);
        assert_eq!(both.exit_code(), 2);
        let neither = parse(&["switch"]).unwrap_err();
        assert_eq!(
            neither.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        assert_eq!(neither.exit_code(), 2);
    }
}
