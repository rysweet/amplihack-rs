use super::name::ProfileName;
use super::store::ProfileStore;
use super::{
    ProfileError, pointer, render_list, render_show, resolve_launch_profile, validate_launch_flags,
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
    assert_eq!(render_list(&s).unwrap(), "  alpha\n* beta\n");
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
    assert!(resolve_launch_profile(&s, None).unwrap().is_none());
    pointer::write(s.root(), &n("ptr")).unwrap();
    let (name, dir) = resolve_launch_profile(&s, None).unwrap().unwrap();
    assert_eq!(name, n("ptr"));
    assert!(dir.ends_with("ptr"));
    let (name, _) = resolve_launch_profile(&s, Some("flag")).unwrap().unwrap();
    assert_eq!(name, n("flag"));
}

#[test]
fn explicit_flag_ignores_corrupt_or_dangling_pointer() {
    let (_d, s) = store();
    s.add(&n("flag")).unwrap();
    fs::write(s.root().join(".default"), "gone").unwrap();
    assert!(resolve_launch_profile(&s, Some("flag")).is_ok());
    assert!(matches!(
        resolve_launch_profile(&s, None),
        Err(ProfileError::PointerDangling(_))
    ));
}

#[test]
fn unknown_or_invalid_flag_value_errors() {
    let (_d, s) = store();
    assert!(matches!(
        resolve_launch_profile(&s, Some("nope")),
        Err(ProfileError::NotFound(_))
    ));
    assert!(matches!(
        resolve_launch_profile(&s, Some("../x")),
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
