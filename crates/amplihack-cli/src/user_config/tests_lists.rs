//! Behaviour of user-scope model-visible list configuration.

use super::edit::{set_value, unset_value};
use super::schema::{Scope, Value, ValueKind, from_toml, known_key, parse_cli};
use super::{LoadCtx, Origin, load};

#[test]
fn list_defaults_are_small_and_extensions_are_empty() {
    for (name, expected) in [
        ("lists.set", Value::Text("default".into())),
        ("lists.enabled", Value::List(Vec::new())),
    ] {
        let key = known_key(name).unwrap();
        assert_eq!(key.scope, Scope::UserOnly);
        assert_eq!(key.env, None);
        assert_eq!(key.default_value(), Some(expected));
    }
    assert_eq!(
        parse_cli("lists.set", "all").unwrap(),
        Value::Text("all".into())
    );
    for raw in ["", "ALL", "everything", "true"] {
        assert!(parse_cli("lists.set", raw).is_err());
    }
}

#[test]
fn command_line_values_for_extensions_are_rejected() {
    for raw in ["[]", r#"["skill:extra"]"#, "skill:extra"] {
        assert!(parse_cli("lists.enabled", raw).is_err(), "{raw}");
    }
}

#[test]
fn extensions_read_from_the_file_are_typed_deduplicated_and_do_not_repeat_defaults() {
    let read = |raw: &str| {
        from_toml(
            ValueKind::EntryList,
            &raw.parse::<toml_edit::Value>().unwrap(),
        )
    };
    assert_eq!(
        read(r#"["skill:extra", "agent:core/guide", "skill:extra", "skill:pdf", "agent:core/builder"]"#).unwrap(),
        Value::List(vec!["agent:core/guide".into(), "skill:extra".into()])
    );
    for raw in [
        r#""extra""#,
        "true",
        "[42]",
        r#"["extra"]"#,
        r#"["skill:group/extra"]"#,
        r#"["agent:../extra"]"#,
        r#"["agent:core/guide.md"]"#,
        r#"["amplihack:extra"]"#,
        r#"["skill:extra", 1]"#,
    ] {
        assert!(read(raw).is_err(), "{raw}");
    }
}

#[test]
fn writing_an_array_preserves_unrelated_content_and_comments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = "# preferences\n[lists]\n# chosen entries\nenabled = []\n\n[recipe]\nverbose = true # keep\n";
    std::fs::write(&path, original).unwrap();
    let key = known_key("lists.enabled").unwrap();
    let value = Value::List(vec!["skill:extra".into()]);
    set_value(&path, key, &value).unwrap();
    let changed = std::fs::read_to_string(&path).unwrap();
    assert!(changed.contains("# preferences\n[lists]\n# chosen entries\n"));
    assert!(changed.ends_with("\n[recipe]\nverbose = true # keep\n"));
    let document = changed.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(
        document["lists"]["enabled"][0].as_str(),
        Some("skill:extra")
    );
    set_value(&path, key, &Value::List(Vec::new())).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert!(unset_value(&path, key).unwrap());
    assert!(!unset_value(&path, key).unwrap());
}

#[test]
fn a_project_cannot_override_either_list_setting() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("user");
    let project = dir.path().join("project");
    std::fs::create_dir_all(home.join(".amplihack")).unwrap();
    std::fs::create_dir_all(project.join(".amplihack")).unwrap();
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let user_file = home.join(".amplihack/config.toml");
    std::fs::write(
        &user_file,
        "[lists]\nset = 'all'\nenabled = ['skill:extra']\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&user_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(
        project.join(".amplihack/config.toml"),
        "[lists]\nset = 'default'\nenabled = []\n",
    )
    .unwrap();
    let effective = load(&LoadCtx {
        home: Some(home),
        cwd: project,
        read_project: true,
        env: &|_| None,
        migrate: false,
    });
    assert_eq!(
        effective.entry("lists.set").value,
        Some(Value::Text("all".into()))
    );
    assert_eq!(
        effective.entry("lists.enabled").value,
        Some(Value::List(vec!["skill:extra".into()]))
    );
    for name in ["lists.set", "lists.enabled"] {
        assert_eq!(
            effective.entry(name).origin,
            Origin::User(user_file.clone())
        );
        assert!(
            effective
                .diagnostics
                .iter()
                .any(|d| d.key.as_deref() == Some(name))
        );
    }
}
