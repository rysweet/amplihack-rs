#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use std::path::PathBuf;

    #[test]
    fn memory_tree_help_hides_kuzu_backend_alias() {
        let mut cmd = Cli::command();
        let memory = cmd
            .find_subcommand_mut("memory")
            .expect("memory command should exist");
        let tree = memory
            .find_subcommand_mut("tree")
            .expect("memory tree command should exist");
        let mut help = Vec::new();
        tree.write_long_help(&mut help).unwrap();
        let rendered = String::from_utf8(help).unwrap();

        assert!(rendered.contains("[possible values: graph-db, sqlite]"));
        assert!(!rendered.contains("graph-db, kuzu, sqlite"));
    }

    #[test]
    fn memory_export_help_hides_kuzu_format_alias() {
        let mut cmd = Cli::command();
        let memory = cmd
            .find_subcommand_mut("memory")
            .expect("memory command should exist");
        let export = memory
            .find_subcommand_mut("export")
            .expect("memory export command should exist");
        let mut help = Vec::new();
        export.write_long_help(&mut help).unwrap();
        let rendered = String::from_utf8(help).unwrap();

        assert!(rendered.contains("[possible values: json, raw-db]"));
        assert!(!rendered.contains("json, raw-db, kuzu"));
        assert!(!rendered.contains("compatibility alias: kuzu"));
    }

    #[test]
    fn memory_cli_still_accepts_kuzu_compat_values() {
        let cli = Cli::try_parse_from(["amplihack", "memory", "tree", "--backend", "kuzu"])
            .expect("legacy backend alias should still parse");
        match cli.command {
            Commands::Memory {
                command: MemoryCommands::Tree { backend, .. },
            } => assert_eq!(backend, "kuzu"),
            other => panic!("expected memory tree command, got {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "amplihack",
            "memory",
            "export",
            "--agent",
            "demo",
            "--output",
            "demo.json",
            "--format",
            "kuzu",
        ])
        .expect("legacy raw-db format alias should still parse");
        match cli.command {
            Commands::Memory {
                command: MemoryCommands::Export { format, .. },
            } => assert_eq!(format, "kuzu"),
            other => panic!("expected memory export command, got {other:?}"),
        }
    }

    #[test]
    fn code_graph_help_hides_kuzu_path_alias() {
        let mut cmd = Cli::command();
        let index = cmd
            .find_subcommand_mut("index-code")
            .expect("index-code command should exist");
        let mut help = Vec::new();
        index.write_long_help(&mut help).unwrap();
        let rendered = String::from_utf8(help).unwrap();
        assert!(rendered.contains("--db-path"));
        assert!(!rendered.contains("--kuzu-path"));

        let mut cmd = Cli::command();
        let query = cmd
            .find_subcommand_mut("query-code")
            .expect("query-code command should exist");
        let mut help = Vec::new();
        query.write_long_help(&mut help).unwrap();
        let rendered = String::from_utf8(help).unwrap();
        assert!(rendered.contains("--db-path"));
        assert!(!rendered.contains("--kuzu-path"));
    }

    #[test]
    fn code_graph_cli_still_accepts_kuzu_path_compat_flag() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "index-code",
            "graph.json",
            "--kuzu-path",
            "/tmp/legacy-graph-db",
        ])
        .expect("legacy kuzu-path alias should still parse for index-code");
        match cli.command {
            Commands::IndexCode {
                db_path,
                legacy_kuzu_path,
                ..
            } => {
                assert!(db_path.is_none());
                assert_eq!(
                    legacy_kuzu_path,
                    Some(PathBuf::from("/tmp/legacy-graph-db"))
                );
            }
            other => panic!("expected index-code command, got {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "amplihack",
            "query-code",
            "--kuzu-path",
            "/tmp/legacy-graph-db",
            "stats",
        ])
        .expect("legacy kuzu-path alias should still parse for query-code");
        match cli.command {
            Commands::QueryCode {
                db_path,
                legacy_kuzu_path,
                ..
            } => {
                assert!(db_path.is_none());
                assert_eq!(
                    legacy_kuzu_path,
                    Some(PathBuf::from("/tmp/legacy-graph-db"))
                );
            }
            other => panic!("expected query-code command, got {other:?}"),
        }
    }

    #[test]
    fn launch_cli_parses_common_sdk_flags() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "launch",
            "--no-reflection",
            "--docker",
            "--subprocess-safe",
            "--auto",
            "--max-turns",
            "12",
            "--ui",
            "--",
            "-p",
            "task",
        ])
        .expect("launch should parse common sdk flags");
        match cli.command {
            Commands::Launch {
                docker,
                no_reflection,
                subprocess_safe,
                auto,
                max_turns,
                ui,
                claude_args,
                ..
            } => {
                assert!(docker);
                assert!(no_reflection);
                assert!(subprocess_safe);
                assert!(auto);
                assert_eq!(max_turns, 12);
                assert!(ui);
                assert_eq!(claude_args, vec!["-p", "task"]);
            }
            other => panic!("expected launch command, got {other:?}"),
        }
    }

    #[test]
    fn launcher_surfaces_parse_docker_flag() {
        let cli = Cli::try_parse_from(["amplihack", "claude", "--docker"])
            .expect("claude should parse --docker");
        match cli.command {
            Commands::Claude { docker, .. } => assert!(docker),
            other => panic!("expected claude command, got {other:?}"),
        }

        let cli = Cli::try_parse_from(["amplihack", "copilot", "--docker", "--", "chat"])
            .expect("copilot should parse --docker");
        match cli.command {
            Commands::Copilot { docker, args, .. } => {
                assert!(docker);
                assert_eq!(args, vec!["chat"]);
            }
            other => panic!("expected copilot command, got {other:?}"),
        }

        let cli = Cli::try_parse_from(["amplihack", "amplifier", "--docker", "--", "-p", "ship"])
            .expect("amplifier should parse --docker");
        match cli.command {
            Commands::Amplifier { docker, args, .. } => {
                assert!(docker);
                assert_eq!(args, vec!["-p", "ship"]);
            }
            other => panic!("expected amplifier command, got {other:?}"),
        }

        let cli = Cli::try_parse_from(["amplihack", "launch", "--docker"])
            .expect("launch should parse --docker");
        match cli.command {
            Commands::Launch { docker, .. } => assert!(
                docker,
                "launch --docker should set docker=true on Launch variant"
            ),
            other => panic!("expected launch command, got {other:?}"),
        }

        let cli = Cli::try_parse_from(["amplihack", "codex", "--docker", "--", "-p", "work"])
            .expect("codex should parse --docker with extra args");
        match cli.command {
            Commands::Codex { docker, args, .. } => {
                assert!(
                    docker,
                    "codex --docker should set docker=true on Codex variant"
                );
                assert_eq!(
                    args,
                    vec!["-p", "work"],
                    "codex --docker should preserve extra args after --"
                );
            }
            other => panic!("expected codex command, got {other:?}"),
        }
    }

    #[test]
    fn copilot_cli_parses_common_sdk_flags() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "copilot",
            "--no-reflection",
            "--subprocess-safe",
            "--auto",
            "--max-turns",
            "9",
            "--ui",
            "--",
            "chat",
        ])
        .expect("copilot should parse common sdk flags");
        match cli.command {
            Commands::Copilot {
                no_reflection,
                subprocess_safe,
                auto,
                max_turns,
                ui,
                args,
                ..
            } => {
                assert!(no_reflection);
                assert!(subprocess_safe);
                assert!(auto);
                assert_eq!(max_turns, 9);
                assert!(ui);
                assert_eq!(args, vec!["chat"]);
            }
            other => panic!("expected copilot command, got {other:?}"),
        }
    }

    #[test]
    fn claude_cli_parses_append_flag() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "claude",
            "--append",
            "Continue with parity audit",
        ])
        .expect("claude should parse append flag");
        match cli.command {
            Commands::Claude { append, .. } => {
                assert_eq!(append.as_deref(), Some("Continue with parity audit"));
            }
            other => panic!("expected claude command, got {other:?}"),
        }
    }

    #[test]
    fn claude_cli_parses_checkout_repo_flag() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "claude",
            "--checkout-repo",
            "owner/repo",
            "--",
            "-p",
            "audit parity",
        ])
        .expect("claude should parse checkout-repo flag");
        match cli.command {
            Commands::Claude {
                checkout_repo,
                claude_args,
                ..
            } => {
                assert_eq!(checkout_repo.as_deref(), Some("owner/repo"));
                assert_eq!(claude_args, vec!["-p", "audit parity"]);
            }
            other => panic!("expected claude command, got {other:?}"),
        }
    }

    #[test]
    fn rustyclawd_cli_parses_auto_flags() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "RustyClawd",
            "--no-reflection",
            "--subprocess-safe",
            "--auto",
            "--max-turns",
            "7",
            "--ui",
            "--",
            "-p",
            "continue parity",
        ])
        .expect("RustyClawd should parse auto flags");
        match cli.command {
            Commands::RustyClawd {
                no_reflection,
                subprocess_safe,
                auto,
                max_turns,
                ui,
                args,
                ..
            } => {
                assert!(no_reflection);
                assert!(subprocess_safe);
                assert!(auto);
                assert_eq!(max_turns, 7);
                assert!(ui);
                assert_eq!(args, vec!["-p", "continue parity"]);
            }
            other => panic!("expected RustyClawd command, got {other:?}"),
        }
    }
}
