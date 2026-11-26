use super::*;
use amplihack_cli::Cli;
use std::{fs, sync::Mutex};

static ENV: Mutex<()> = Mutex::new(());
struct Environment {
    previous: [Option<OsString>; 2],
    codex: tempfile::TempDir,
}
impl Environment {
    fn new(home: &std::path::Path) -> Self {
        // Select native targets outside HOME to verify reset is metadata-only.
        let codex = tempfile::tempdir().unwrap();
        fs::write(codex.path().join("config.toml"), b"# native config\n").unwrap();
        fs::write(codex.path().join("hooks.json"), b"{\"native\":true}\n").unwrap();
        let previous = [std::env::var_os("HOME"), std::env::var_os("CODEX_HOME")];
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("CODEX_HOME", codex.path());
        }
        Self { previous, codex }
    }
    fn assert_targets_untouched(&self) {
        assert_eq!(
            fs::read(self.codex.path().join("config.toml")).unwrap(),
            b"# native config\n"
        );
        assert_eq!(
            fs::read(self.codex.path().join("hooks.json")).unwrap(),
            b"{\"native\":true}\n"
        );
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (key, previous) in ["HOME", "CODEX_HOME"].into_iter().zip(&mut self.previous) {
            unsafe {
                match previous.take() {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

#[test]
fn reset_finishes_before_accepted_declined_or_unavailable_startup_update() {
    let _lock = ENV.lock().unwrap();
    for available in [true, false] {
        for accept in [true, false] {
            let home = tempfile::tempdir().unwrap();
            let environment = Environment::new(home.path());
            let root = home.path().join(".amplihack/codex");
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("pending.json"), b"unparsed journal").unwrap();
            let mut args = ["amplihack", "codex", "--reset-recovery", "--", "exec", "-"]
                .map(OsString::from)
                .to_vec();
            let mut cli = Cli::try_parse_from(&args).unwrap();
            let mut order = Vec::new();
            let outcome = prepare_with(&mut cli.command, &mut args, |forwarded| {
                // This injected updater represents checks ENABLED, an available
                // release, user acceptance and the post-update installation.
                assert!(!root.join("pending.json").exists());
                let receipt: serde_json::Value =
                    serde_json::from_slice(&fs::read(root.join("recovery-archive.json")).unwrap())
                        .unwrap();
                assert_eq!(receipt["complete"], true);
                assert_eq!(
                    fs::read(root.join(receipt["destination"].as_str().unwrap())).unwrap(),
                    b"unparsed journal"
                );
                order.push("reset-complete");
                order.push("update-check");
                assert!(!forwarded.iter().any(|arg| arg == "--reset-recovery"));
                if available && accept {
                    order.push("update-install");
                    fs::write(home.path().join("installed-assets"), b"updated").unwrap();
                    update::StartupUpdateOutcome::ExitSuccess
                } else {
                    update::StartupUpdateOutcome::Continue
                }
            })
            .unwrap();
            assert!(matches!(
                cli.command,
                Commands::Codex {
                    reset_recovery: false,
                    ..
                }
            ));
            assert_eq!(&order[..2], ["reset-complete", "update-check"]);
            assert_eq!(
                home.path().join("installed-assets").exists(),
                available && accept
            );
            assert_eq!(
                matches!(outcome, update::StartupUpdateOutcome::ExitSuccess),
                available && accept
            );
            environment.assert_targets_untouched();
            // Re-execution receives the consumed argv and cannot reset again.
            let replay = Cli::try_parse_from(&args).unwrap();
            assert!(matches!(
                replay.command,
                Commands::Codex {
                    reset_recovery: false,
                    ..
                }
            ));
        }
    }
}

#[test]
fn reset_failure_stops_before_updater_and_leaves_arguments_unconsumed() {
    let _lock = ENV.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let environment = Environment::new(home.path());
    let root = home.path().join(".amplihack/codex");
    fs::create_dir_all(root.join("pending.json")).unwrap();
    let mut args = ["amplihack", "codex", "--reset-recovery"]
        .map(OsString::from)
        .to_vec();
    let before = args.clone();
    let mut cli = Cli::try_parse_from(&args).unwrap();
    assert!(
        prepare_with(&mut cli.command, &mut args, |_| panic!(
            "update must not run"
        ))
        .is_err()
    );
    environment.assert_targets_untouched();
    assert_eq!(args, before);
    assert!(root.join("pending.json").is_dir());
    assert!(matches!(
        cli.command,
        Commands::Codex {
            reset_recovery: true,
            ..
        }
    ));
}

#[test]
fn ordinary_startup_keeps_journal_and_arguments_for_updater() {
    let _lock = ENV.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let environment = Environment::new(home.path());
    let root = home.path().join(".amplihack/codex");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("pending.json"), b"current journal").unwrap();
    let mut args = ["amplihack", "codex", "--", "exec", "-"]
        .map(OsString::from)
        .to_vec();
    let before = args.clone();
    let mut cli = Cli::try_parse_from(&args).unwrap();
    prepare_with(&mut cli.command, &mut args, |forwarded| {
        assert_eq!(forwarded, before);
        assert_eq!(
            fs::read(root.join("pending.json")).unwrap(),
            b"current journal"
        );
        update::StartupUpdateOutcome::Continue
    })
    .unwrap();
    environment.assert_targets_untouched();
}

#[test]
fn forwarding_preserves_literal_native_reset_with_or_without_separator() {
    let _lock = ENV.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let environment = Environment::new(home.path());
    for native in [
        vec!["--", "--reset-recovery", "literal"],
        vec!["exec", "--reset-recovery"],
    ] {
        let mut args = ["amplihack", "codex", "--reset-recovery"]
            .map(OsString::from)
            .to_vec();
        args.extend(native.iter().map(OsString::from));
        let mut cli = Cli::try_parse_from(&args).unwrap();
        prepare_with(&mut cli.command, &mut args, |forwarded| {
            assert_eq!(
                forwarded
                    .iter()
                    .filter(|arg| *arg == "--reset-recovery")
                    .count(),
                1
            );
            update::StartupUpdateOutcome::Continue
        })
        .unwrap();
        let replay = Cli::try_parse_from(&args).unwrap();
        assert!(matches!(
            replay.command,
            Commands::Codex {
                reset_recovery: false,
                ..
            }
        ));
    }
    environment.assert_targets_untouched();
}
