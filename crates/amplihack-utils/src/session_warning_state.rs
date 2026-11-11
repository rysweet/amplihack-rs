//! Session lifecycle registration scoped to the canonical profile root and
//! kernel-reported hook parent. No conversation or credential data is stored.
use crate::swap_warning_history::{Session, WarningHistory};
use std::{io, path::Path};

pub fn register(root: &Path, session_id: &str) -> io::Result<(WarningHistory, Session)> {
    let root = root.canonicalize()?;
    let host = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(config) => {
            let config = std::path::PathBuf::from(config).canonicalize()?;
            if config.parent() == Some(root.as_path()) {
                let name = config
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| io::Error::other("invalid profile host"))?
                    .to_owned();
                format!("profile:{name}")
            } else {
                let home = std::env::var_os("HOME")
                    .ok_or_else(|| io::Error::other("HOME missing for session scope"))?;
                if config
                    != std::path::PathBuf::from(home)
                        .join(".claude")
                        .canonicalize()?
                {
                    return Err(io::Error::other(
                        "session configuration is outside the profile root and home",
                    ));
                }
                "home".into()
            }
        }
        None => "home".into(),
    };
    let mut history = WarningHistory::open(&root)?;
    let session = history.register_parent_process(session_id, &host)?;
    Ok((history, session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::EnvGuard;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn named_profile_home_cannot_share_the_default_home_scope() {
        let _lock = crate::test_serial::acquire();
        let tmp = tempfile::tempdir().unwrap();
        // Private whatever the umask: the warning root's parents are checked.
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = tmp.path().join("profiles");
        let home = tmp.path().join("user");
        let config = root.join("home");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let _env = EnvGuard::set([
            ("HOME", home.to_str().unwrap()),
            ("CLAUDE_CONFIG_DIR", config.to_str().unwrap()),
        ]);
        let (mut history, session) = register(&root, "session").unwrap();
        let intent = history.prepare(&serde_json::json!({"version":1,"transaction_id":"default-home-swap",
            "producer":"rotator","reason":"threshold","host":"home",
            "from_login":"work","to_login":"spare","from_org":"org-a","to_org":"org-b","affinity":"crossing"})).unwrap();
        history
            .complete(&intent, crate::swap_warning_history::Completion::Committed)
            .unwrap();
        assert!(history.pending(&session).unwrap().is_empty());
    }
}
