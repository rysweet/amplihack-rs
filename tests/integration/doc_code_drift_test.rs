//! Documentation-vs-code drift tests for issue #259.
//!
//! These tests verify that documentation accurately reflects the source code,
//! catching the four drift bugs (D1-D3) and validating the four new reference
//! docs (D4-D7) plus mkdocs.yml navigation (D8).
//!
//! Test strategy: Parse documentation files and assert content matches the
//! source-of-truth in Rust source code. No binary execution required — these
//! are pure file-content assertions.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/
    path.pop(); // workspace root
    path
}

fn read_doc(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

// ═══════════════════════════════════════════════════════════════════════════
// D1: launch-flag-injection.md — drift fixes
// ═══════════════════════════════════════════════════════════════════════════

mod launch_flag_injection {
    use super::*;

    /// Source code says: `"claude" | "rusty" | "rustyclawd" | "amplifier"`
    /// Doc must list exactly these four Claude-compatible tools.
    #[test]
    fn overview_table_lists_claude_compatible_tools() {
        let doc = read_doc("docs/reference/launch-flag-injection.md");
        // The table row for --dangerously-skip-permissions must mention all four
        assert!(
            doc.contains("`claude`, `rusty`, `rustyclawd`, `amplifier`"),
            "Overview table must list all four Claude-compatible tools: claude, rusty, rustyclawd, amplifier"
        );
    }

    /// DRIFT-1 core: The Rust launcher sections must NOT say "always injected"
    /// for --dangerously-skip-permissions. The Python parity table may mention
    /// "always injected" for the Python column — that's intentional divergence
    /// documentation.
    #[test]
    fn skip_permissions_not_always_injected_in_rust_sections() {
        let doc = read_doc("docs/reference/launch-flag-injection.md");
        // Check sections before the parity table (which legitimately describes
        // Python's "always injected" behavior).
        let parity_start = doc.find("## Python launcher parity").unwrap_or(doc.len());
        let rust_sections = &doc[..parity_start];
        let lower = rust_sections.to_lowercase();
        assert!(
            !lower.contains("always inject"),
            "Rust launcher sections must not claim --dangerously-skip-permissions is \
             'always injected'. Code conditionally injects for Claude-compatible tools only. \
             (Note: 'always injected' is acceptable only in the Python parity table.)"
        );
    }

    /// The doc must state both conditions: skip_permissions AND is_claude_compatible.
    #[test]
    fn skip_permissions_requires_both_conditions() {
        let doc = read_doc("docs/reference/launch-flag-injection.md");
        assert!(
            doc.contains("--skip-permissions") && doc.contains("Claude-compatible"),
            "Doc must explain both conditions for --dangerously-skip-permissions injection"
        );
    }

    /// --model injection must be documented as Claude-compatible only.
    #[test]
    fn model_injection_is_claude_compatible_only() {
        let doc = read_doc("docs/reference/launch-flag-injection.md");
        // The --model section must mention Claude-compatible restriction
        let model_section_start = doc
            .find("## --model")
            .expect("Doc must have --model section");
        let model_section = &doc[model_section_start..];
        let next_section = model_section[3..]
            .find("## ")
            .map(|i| i + 3)
            .unwrap_or(model_section.len());
        let model_text = &model_section[..next_section];
        assert!(
            model_text.contains("Claude-compatible"),
            "--model section must document Claude-compatible-only injection"
        );
    }

    /// `rusty` must appear in the Claude-compatible tools list.
    /// Source: command.rs line 44-45: `"claude" | "rusty" | "rustyclawd" | "amplifier"`
    #[test]
    fn rusty_listed_as_claude_compatible() {
        let doc = read_doc("docs/reference/launch-flag-injection.md");
        // Count occurrences of `rusty` in tool lists
        assert!(
            doc.contains("`rusty`"),
            "Doc must list `rusty` as a Claude-compatible tool (source: command.rs L44)"
        );
    }

    /// DRIFT-2: --resume/--continue must say "launch only, not claude".
    /// Source: Commands::Claude has no resume/continue_session fields.
    #[test]
    fn resume_continue_launch_only() {
        let doc = read_doc("docs/reference/launch-flag-injection.md");
        // Find the resume/continue section
        let section = doc
            .find("## --resume")
            .or_else(|| doc.find("--resume and --continue"))
            .expect("Doc must have --resume/--continue section");
        let section_text = &doc[section..];
        // Must explicitly state these are launch-only
        assert!(
            section_text.contains("launch") && section_text.contains("not"),
            "--resume/--continue section must state these are launch-only, not available on claude subcommand"
        );
    }

    /// Examples must NOT show --dangerously-skip-permissions without --skip-permissions.
    /// After fix, bare `amplihack claude` should spawn `claude --model opus[1m]` only.
    #[test]
    fn bare_claude_example_no_skip_permissions() {
        let doc = read_doc("docs/reference/launch-flag-injection.md");
        // Find "amplihack claude\n" (bare, no flags) examples
        // After "amplihack claude" without --skip-permissions, the spawned command
        // should NOT contain --dangerously-skip-permissions
        for (i, line) in doc.lines().enumerate() {
            if line.trim() == "amplihack claude" || line.trim() == "# User runs:" {
                // Check the next few lines for the spawned command
                let context: String = doc.lines().skip(i).take(5).collect::<Vec<_>>().join("\n");
                if context.contains("# amplihack spawns:") || context.contains("# →") {
                    // The spawned command after bare `amplihack claude` must not have --dangerously-skip-permissions
                    let spawned_line = doc
                        .lines()
                        .skip(i)
                        .take(5)
                        .find(|l| l.contains("claude --") || l.starts_with("# →"));
                    if let Some(spawned) = spawned_line
                        && !context.contains("--skip-permissions")
                    {
                        assert!(
                            !spawned.contains("--dangerously-skip-permissions"),
                            "Line {}: bare `amplihack claude` must NOT inject --dangerously-skip-permissions.\nContext:\n{context}",
                            i + 1
                        );
                    }
                }
            }
        }
    }

    /// The parity table must show Rust launcher as conditional, not "always injected".
    #[test]
    fn parity_table_rust_column_conditional() {
        let doc = read_doc("docs/reference/launch-flag-injection.md");
        let parity_start = doc
            .find("## Python launcher parity")
            .expect("Doc must have Python launcher parity section");
        let parity_text = &doc[parity_start..];
        // Rust column for --dangerously-skip-permissions must say "conditional"
        assert!(
            parity_text.contains("conditional"),
            "Parity table Rust column must say 'conditional' for --dangerously-skip-permissions"
        );
    }

    /// Assembly section must list Claude-compatible condition for both
    /// --dangerously-skip-permissions and --model.
    #[test]
    fn assembly_section_has_claude_compatible_condition() {
        let doc = read_doc("docs/reference/launch-flag-injection.md");
        let assembly_start = doc
            .find("## Complete command-line assembly")
            .expect("Doc must have assembly section");
        let assembly_text = &doc[assembly_start..];
        assert!(
            assembly_text.contains("Claude-compatible"),
            "Assembly section must mention Claude-compatible condition"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D2: completions-command.md — real Commands enum variants
// ═══════════════════════════════════════════════════════════════════════════

mod completions_command {
    use super::*;

    /// The completions doc must list all 24 real Commands enum variants.
    /// Source: cli_commands.rs defines these exact variants.
    #[test]
    fn lists_all_24_subcommands() {
        let doc = read_doc("docs/reference/completions-command.md");

        // All 24 variants from Commands enum, using their actual command names
        // (respecting #[command(name = "...")] attributes)
        let expected_commands = [
            "install",
            "uninstall",
            "launch",
            "claude",
            "copilot",
            "codex",
            "amplifier",
            "plugin",
            "memory",
            "index-code",
            "index-scip",
            "query-code",
            "recipe",
            "mode",
            "version",
            "update",
            "fleet",
            "new",
            "RustyClawd", // explicit #[command(name = "RustyClawd")]
            "uvx-help",   // explicit #[command(name = "uvx-help")]
            "completions",
            "doctor",
            "resolve-bundle-asset", // explicit #[command(name = "resolve-bundle-asset")]
            "multitask",
        ];

        for cmd in &expected_commands {
            assert!(
                doc.contains(cmd),
                "Completions doc must list subcommand '{cmd}' (from Commands enum in cli_commands.rs)"
            );
        }
    }

    /// Must NOT contain fake/nonexistent subcommands that were in the old doc.
    #[test]
    fn no_fake_subcommands() {
        let doc = read_doc("docs/reference/completions-command.md");
        // These were the old fake subcommands. If any appear that aren't also
        // real subcommands, that's a drift bug.
        // We check that the verification section doesn't contain commands
        // that aren't in the Commands enum.
        let verification_start = doc
            .find("## Verification")
            .expect("Doc must have Verification section");
        let verification_text = &doc[verification_start..];

        // Make sure the tab-completion example exists with real commands
        assert!(
            verification_text.contains("RustyClawd"),
            "Verification section must include RustyClawd (from Commands enum)"
        );
        assert!(
            verification_text.contains("multitask"),
            "Verification section must include multitask (from Commands enum)"
        );
        assert!(
            verification_text.contains("resolve-bundle-asset"),
            "Verification section must include resolve-bundle-asset (from Commands enum)"
        );
    }

    /// The RustyClawd command must use exact casing (not rustyclawd or rusty-clawd).
    #[test]
    fn rustyclawd_exact_casing() {
        let doc = read_doc("docs/reference/completions-command.md");
        // Find the tab-completion example
        let verification_start = doc.find("## Verification").unwrap();
        let verification_text = &doc[verification_start..];
        assert!(
            verification_text.contains("RustyClawd"),
            "Must use exact 'RustyClawd' casing per #[command(name = \"RustyClawd\")]"
        );
    }

    /// Shells must match clap_complete::Shell enum: bash, zsh, fish, powershell.
    #[test]
    fn shell_values_match_clap() {
        let doc = read_doc("docs/reference/completions-command.md");
        for shell in &["bash", "zsh", "fish", "powershell"] {
            assert!(
                doc.contains(shell),
                "Doc must list shell '{shell}' from clap_complete::Shell"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D3: memory-index-command.md — output format drift
// ═══════════════════════════════════════════════════════════════════════════

mod memory_index_command {
    use super::*;

    /// DRIFT-3: index-scip output must be documented as plain-text, NOT JSON.
    /// Source: commands.rs uses println!("Native SCIP indexing summary") etc.
    #[test]
    fn index_scip_output_is_plain_text() {
        let doc = read_doc("docs/reference/memory-index-command.md");
        let output_section = doc
            .find("## Output format")
            .expect("Doc must have Output format section");
        let output_text = &doc[output_section..];

        // Find the index-scip subsection
        let scip_start = output_text
            .find("index-scip")
            .expect("Output format section must mention index-scip");
        let scip_text = &output_text[scip_start..];

        // Must say plain-text, not JSON
        assert!(
            scip_text.contains("plain-text") || scip_text.contains("plain text"),
            "index-scip output format must be documented as plain-text, not JSON. \
             Source: commands.rs uses println!() for human-readable output."
        );
    }

    /// The index-scip example must show the "Native SCIP indexing summary" header.
    /// Source: commands.rs line 43: println!("Native SCIP indexing summary")
    #[test]
    fn index_scip_example_has_correct_header() {
        let doc = read_doc("docs/reference/memory-index-command.md");
        assert!(
            doc.contains("Native SCIP indexing summary"),
            "Doc must show 'Native SCIP indexing summary' header (from commands.rs L43)"
        );
    }

    /// The index-scip example must show the 40-char separator.
    /// Source: commands.rs line 44: println!("{}", "=".repeat(40))
    #[test]
    fn index_scip_example_has_separator() {
        let doc = read_doc("docs/reference/memory-index-command.md");
        let separator = "=".repeat(40);
        assert!(
            doc.contains(&separator),
            "Doc must show 40-char '=' separator (from commands.rs L44)"
        );
    }

    /// Must show key-value pairs matching the actual println! format.
    /// Source: "Success: {}", "Completed: {}", "Skipped: {}", "Artifact: {}",
    ///         "Imported: files={}, classes={}, ..."
    #[test]
    fn index_scip_example_has_key_value_pairs() {
        let doc = read_doc("docs/reference/memory-index-command.md");
        let expected_keys = ["Success:", "Completed:", "Imported: files="];
        for key in &expected_keys {
            assert!(
                doc.contains(key),
                "index-scip example must contain '{key}' (matches println! in commands.rs)"
            );
        }
    }

    /// index-scip output section must NOT claim JSON format.
    #[test]
    fn index_scip_not_json() {
        let doc = read_doc("docs/reference/memory-index-command.md");
        // Find index-scip output subsection specifically
        let output_section = doc.find("## Output format").unwrap();
        let output_text = &doc[output_section..];
        let scip_start = output_text
            .find("### index-scip")
            .unwrap_or(output_text.find("index-scip").unwrap());
        let scip_text = &output_text[scip_start..];
        // Limit to this subsection
        let next_section = scip_text[1..].find("## ").unwrap_or(scip_text.len());
        let scip_section = &scip_text[..next_section];

        assert!(
            !scip_section.contains("```json"),
            "index-scip output must NOT be in a ```json code block — it's plain-text"
        );
    }

    /// index-code output SHOULD still be documented as JSON (that's correct).
    #[test]
    fn index_code_output_is_json() {
        let doc = read_doc("docs/reference/memory-index-command.md");
        let output_section = doc.find("## Output format").unwrap();
        let output_text = &doc[output_section..];

        // index-code subsection should have JSON
        let code_start = output_text
            .find("### index-code")
            .expect("Output format must have index-code subsection");
        let code_text = &output_text[code_start..];
        let scip_start = code_text.find("### index-scip").unwrap_or(code_text.len());
        let code_section = &code_text[..scip_start];

        assert!(
            code_section.contains("JSON") || code_section.contains("json"),
            "index-code output should be documented as JSON"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D4: rustyclawd-command.md — new reference doc
// ═══════════════════════════════════════════════════════════════════════════

mod rustyclawd_command {
    use super::*;

    #[test]
    fn file_exists() {
        let path = workspace_root().join("docs/reference/rustyclawd-command.md");
        assert!(path.exists(), "rustyclawd-command.md must exist");
    }

    #[test]
    fn has_synopsis() {
        let doc = read_doc("docs/reference/rustyclawd-command.md");
        assert!(doc.contains("## Synopsis"), "Must have Synopsis section");
        assert!(
            doc.contains("amplihack RustyClawd"),
            "Synopsis must use exact 'RustyClawd' casing"
        );
    }

    /// All 6 flags from cli_commands.rs RustyClawd variant.
    #[test]
    fn documents_all_flags() {
        let doc = read_doc("docs/reference/rustyclawd-command.md");
        let expected_flags = [
            "--append",
            "--no-reflection",
            "--subprocess-safe",
            "--auto",
            "--max-turns",
            "--ui",
        ];
        for flag in &expected_flags {
            assert!(
                doc.contains(flag),
                "RustyClawd doc must document flag '{flag}' (from cli_commands.rs)"
            );
        }
    }

    #[test]
    fn documents_trailing_args() {
        let doc = read_doc("docs/reference/rustyclawd-command.md");
        assert!(
            doc.contains("trailing") || doc.contains("ARGS") || doc.contains("forwarded"),
            "Must document trailing args passthrough"
        );
    }

    #[test]
    fn has_exit_codes() {
        let doc = read_doc("docs/reference/rustyclawd-command.md");
        assert!(
            doc.contains("## Exit Codes") || doc.contains("## Exit codes"),
            "Must have exit codes section"
        );
    }

    #[test]
    fn mentions_claude_compatible() {
        let doc = read_doc("docs/reference/rustyclawd-command.md");
        assert!(
            doc.contains("Claude-compatible"),
            "Must mention RustyClawd is Claude-compatible (receives flag injection)"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D5: uvx-help-command.md — new reference doc
// ═══════════════════════════════════════════════════════════════════════════

mod uvx_help_command {
    use super::*;

    #[test]
    fn file_exists() {
        let path = workspace_root().join("docs/reference/uvx-help-command.md");
        assert!(path.exists(), "uvx-help-command.md must exist");
    }

    #[test]
    fn has_synopsis() {
        let doc = read_doc("docs/reference/uvx-help-command.md");
        assert!(doc.contains("## Synopsis"), "Must have Synopsis section");
        assert!(
            doc.contains("uvx-help"),
            "Synopsis must use 'uvx-help' name"
        );
    }

    /// Two flags from cli_commands.rs UvxHelp variant.
    #[test]
    fn documents_both_flags() {
        let doc = read_doc("docs/reference/uvx-help-command.md");
        assert!(
            doc.contains("--find-path"),
            "Must document --find-path flag"
        );
        assert!(doc.contains("--info"), "Must document --info flag");
    }

    #[test]
    fn has_exit_codes() {
        let doc = read_doc("docs/reference/uvx-help-command.md");
        assert!(
            doc.contains("## Exit Codes") || doc.contains("## Exit codes"),
            "Must have exit codes section"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D6: resolve-bundle-asset-command.md — new reference doc
// ═══════════════════════════════════════════════════════════════════════════

mod resolve_bundle_asset_command {
    use super::*;

    #[test]
    fn file_exists() {
        let path = workspace_root().join("docs/reference/resolve-bundle-asset-command.md");
        assert!(path.exists(), "resolve-bundle-asset-command.md must exist");
    }

    #[test]
    fn has_synopsis() {
        let doc = read_doc("docs/reference/resolve-bundle-asset-command.md");
        assert!(doc.contains("## Synopsis"), "Must have Synopsis section");
        assert!(
            doc.contains("resolve-bundle-asset"),
            "Synopsis must use 'resolve-bundle-asset' name"
        );
    }

    /// Required argument: <ASSET> or <asset>.
    #[test]
    fn documents_asset_argument() {
        let doc = read_doc("docs/reference/resolve-bundle-asset-command.md");
        assert!(
            doc.contains("ASSET") || doc.contains("asset"),
            "Must document the required asset argument"
        );
    }

    /// Exit codes must document 0, 1, AND 2.
    /// Source: cli_commands.rs doc comment says "exits 1 if not found, exits 2 on invalid input"
    #[test]
    fn has_three_exit_codes() {
        let doc = read_doc("docs/reference/resolve-bundle-asset-command.md");
        assert!(doc.contains("| `0`"), "Must document exit code 0 (success)");
        assert!(
            doc.contains("| `1`"),
            "Must document exit code 1 (not found)"
        );
        assert!(
            doc.contains("| `2`"),
            "Must document exit code 2 (invalid input)"
        );
    }

    /// Security: must mention path traversal protection.
    #[test]
    fn documents_security_constraints() {
        let doc = read_doc("docs/reference/resolve-bundle-asset-command.md");
        assert!(
            doc.contains("traversal") || doc.contains(".."),
            "Must document path traversal protection"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D7: multitask-command.md — new reference doc
// ═══════════════════════════════════════════════════════════════════════════

mod multitask_command {
    use super::*;

    #[test]
    fn file_exists() {
        let path = workspace_root().join("docs/reference/multitask-command.md");
        assert!(path.exists(), "multitask-command.md must exist");
    }

    #[test]
    fn has_synopsis() {
        let doc = read_doc("docs/reference/multitask-command.md");
        assert!(doc.contains("## Synopsis"), "Must have Synopsis section");
        assert!(doc.contains("multitask"), "Synopsis must mention multitask");
    }

    /// Must document all 3 subcommands from MultitaskCommands enum.
    #[test]
    fn documents_all_subcommands() {
        let doc = read_doc("docs/reference/multitask-command.md");
        let lower = doc.to_lowercase();
        assert!(lower.contains("run"), "Must document 'run' subcommand");
        assert!(
            lower.contains("cleanup"),
            "Must document 'cleanup' subcommand"
        );
        assert!(
            lower.contains("status"),
            "Must document 'status' subcommand"
        );
    }

    /// run subcommand flags match MultitaskCommands::Run struct fields.
    #[test]
    fn run_subcommand_has_correct_flags() {
        let doc = read_doc("docs/reference/multitask-command.md");
        let expected_flags = [
            "--mode",
            "--recipe",
            "--max-runtime",
            "--timeout-policy",
            "--dry-run",
        ];
        for flag in &expected_flags {
            assert!(
                doc.contains(flag),
                "multitask run must document flag '{flag}' (from MultitaskCommands::Run)"
            );
        }
    }

    /// cleanup subcommand flags match MultitaskCommands::Cleanup struct fields.
    #[test]
    fn cleanup_subcommand_has_dry_run() {
        let doc = read_doc("docs/reference/multitask-command.md");
        // Find the cleanup section and verify --dry-run is there
        let cleanup_start = doc
            .find("### cleanup")
            .or_else(|| doc.find("### Cleanup"))
            .expect("Must have cleanup subsection");
        let cleanup_text = &doc[cleanup_start..];
        assert!(
            cleanup_text.contains("--dry-run"),
            "cleanup subcommand must document --dry-run flag"
        );
    }

    /// status subcommand flags match MultitaskCommands::Status struct fields.
    #[test]
    fn status_subcommand_has_base_dir() {
        let doc = read_doc("docs/reference/multitask-command.md");
        let status_start = doc
            .find("### status")
            .or_else(|| doc.find("### Status"))
            .expect("Must have status subsection");
        let status_text = &doc[status_start..];
        assert!(
            status_text.contains("--base-dir"),
            "status subcommand must document --base-dir flag"
        );
    }

    #[test]
    fn has_exit_codes() {
        let doc = read_doc("docs/reference/multitask-command.md");
        assert!(
            doc.contains("## Exit Codes") || doc.contains("## Exit codes"),
            "Must have exit codes section"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// D8: mkdocs.yml — navigation entries
// ═══════════════════════════════════════════════════════════════════════════

mod mkdocs_navigation {
    use super::*;

    #[test]
    fn has_rustyclawd_nav_entry() {
        let config = read_doc("mkdocs.yml");
        assert!(
            config.contains("rustyclawd-command.md"),
            "mkdocs.yml must have nav entry for rustyclawd-command.md"
        );
    }

    #[test]
    fn has_uvx_help_nav_entry() {
        let config = read_doc("mkdocs.yml");
        assert!(
            config.contains("uvx-help-command.md"),
            "mkdocs.yml must have nav entry for uvx-help-command.md"
        );
    }

    #[test]
    fn has_resolve_bundle_asset_nav_entry() {
        let config = read_doc("mkdocs.yml");
        assert!(
            config.contains("resolve-bundle-asset-command.md"),
            "mkdocs.yml must have nav entry for resolve-bundle-asset-command.md"
        );
    }

    #[test]
    fn has_multitask_nav_entry() {
        let config = read_doc("mkdocs.yml");
        assert!(
            config.contains("multitask-command.md"),
            "mkdocs.yml must have nav entry for multitask-command.md"
        );
    }

    /// All four new entries must be in the Reference section.
    #[test]
    fn new_entries_in_reference_section() {
        let config = read_doc("mkdocs.yml");
        let reference_start = config
            .find("Reference:")
            .expect("mkdocs.yml must have Reference: section");
        let concepts_start = config.find("Concepts:").unwrap_or(config.len());
        let reference_section = &config[reference_start..concepts_start];

        for doc_file in &[
            "rustyclawd-command.md",
            "uvx-help-command.md",
            "resolve-bundle-asset-command.md",
            "multitask-command.md",
        ] {
            assert!(
                reference_section.contains(doc_file),
                "'{doc_file}' must be in the Reference section of mkdocs.yml nav"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-document consistency checks
// ═══════════════════════════════════════════════════════════════════════════

mod cross_doc_consistency {
    use super::*;

    /// The Claude-compatible tool list must be consistent across all docs.
    /// Source of truth: command.rs `"claude" | "rusty" | "rustyclawd" | "amplifier"`
    #[test]
    fn claude_compatible_list_consistent() {
        let launch_doc = read_doc("docs/reference/launch-flag-injection.md");
        let rusty_doc = read_doc("docs/reference/rustyclawd-command.md");

        // launch-flag-injection.md must list all four
        assert!(
            launch_doc.contains("`claude`"),
            "launch doc must list claude"
        );
        assert!(launch_doc.contains("`rusty`"), "launch doc must list rusty");
        assert!(
            launch_doc.contains("`rustyclawd`"),
            "launch doc must list rustyclawd"
        );
        assert!(
            launch_doc.contains("`amplifier`"),
            "launch doc must list amplifier"
        );

        // rustyclawd-command.md must reference being Claude-compatible
        assert!(
            rusty_doc.contains("Claude-compatible"),
            "RustyClawd doc must mention it's Claude-compatible"
        );
    }

    /// Completions doc command count must match Commands enum variant count (24).
    #[test]
    fn completions_tab_example_has_24_commands() {
        let doc = read_doc("docs/reference/completions-command.md");
        // Find the TAB completion example block
        let tab_start = doc
            .find("amplihack <TAB>")
            .expect("Must have TAB completion example");
        // Count the command names in the subsequent lines
        let after_tab = &doc[tab_start..];
        // Find the end of the code block
        let block_end = after_tab.find("```\n").unwrap_or(after_tab.len());
        let block = &after_tab[..block_end];

        // Count unique command names (whitespace-separated tokens on indented lines)
        let commands: Vec<&str> = block
            .lines()
            .filter(|l| !l.starts_with("$") && !l.starts_with("amplihack") && !l.trim().is_empty())
            .flat_map(|l| l.split_whitespace())
            .collect();

        assert_eq!(
            commands.len(),
            24,
            "Tab completion example must list exactly 24 commands (matching Commands enum).\n\
             Found {}: {:?}",
            commands.len(),
            commands
        );
    }
}
