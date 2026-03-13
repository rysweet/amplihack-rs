//! Shell completion generation for bash, zsh, fish, and powershell.
//!
//! Generates completion scripts to stdout using the `clap_complete` crate.

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::Shell;

use crate::Cli;

/// Write shell completions for `shell` to stdout.
///
/// The completion script is emitted directly to stdout so that the caller can
/// redirect it to a file or source it in a shell profile.
pub fn run_completions(shell: Shell) -> Result<()> {
    // SAFETY: The binary name argument `"amplihack"` is a compile-time literal.
    // The `shell` argument is a validated `clap_complete::Shell` enum variant
    // parsed by clap — not a raw user string — so it cannot introduce injection.
    clap_complete::generate(
        shell,
        &mut Cli::command(),
        "amplihack",
        &mut std::io::stdout(),
    );
    Ok(())
}

// ── Helper used by tests ──────────────────────────────────────────────────────

/// Generate completions for `shell` into an in-memory buffer and return the
/// bytes.  Used exclusively in tests to avoid touching stdout.
#[cfg(test)]
fn generate_to_vec(shell: Shell) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "amplihack", &mut buf);
    buf
}

#[cfg(test)]
fn generate_to_string(shell: Shell) -> String {
    String::from_utf8(generate_to_vec(shell)).expect("completion script should be valid UTF-8")
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Each test has a single, clear assertion so failure messages are unambiguous.

#[cfg(test)]
mod tests {
    use super::*;

    // ── WS1-TEST-01: Bash ─────────────────────────────────────────────────

    /// Bash completion output must be non-empty.
    #[test]
    fn test_bash_completions_produces_output() {
        let output = generate_to_vec(Shell::Bash);
        assert!(
            !output.is_empty(),
            "bash completion script should not be empty"
        );
    }

    /// Bash completion script must contain the `complete` keyword — the
    /// standard bash built-in used to register completion functions. CODE-1.
    #[test]
    fn test_bash_completions_contains_complete_keyword() {
        let script = generate_to_string(Shell::Bash);
        assert!(
            script.contains("complete"),
            "bash completion script must contain the 'complete' keyword; got script starting: {}",
            &script[..script.len().min(200)]
        );
    }

    // ── WS1-TEST-02: Zsh ──────────────────────────────────────────────────

    /// Zsh completion output must be non-empty.
    #[test]
    fn test_zsh_completions_produces_output() {
        let output = generate_to_vec(Shell::Zsh);
        assert!(
            !output.is_empty(),
            "zsh completion script should not be empty"
        );
    }

    /// Zsh completion script must contain either `compdef` or `#compdef` —
    /// the standard Zsh mechanism for associating a completion function with
    /// a command. CODE-1.
    #[test]
    fn test_zsh_completions_contains_compdef() {
        let script = generate_to_string(Shell::Zsh);
        assert!(
            script.contains("compdef") || script.contains("#compdef"),
            "zsh completion script must contain 'compdef' or '#compdef'; got script starting: {}",
            &script[..script.len().min(200)]
        );
    }

    // ── WS1-TEST-03: Fish ─────────────────────────────────────────────────

    /// Fish completion output must be non-empty.
    #[test]
    fn test_fish_completions_produces_output() {
        let output = generate_to_vec(Shell::Fish);
        assert!(
            !output.is_empty(),
            "fish completion script should not be empty"
        );
    }

    /// Fish completion script must contain `complete -c` — the fish
    /// built-in used to register completions for a command. CODE-1.
    #[test]
    fn test_fish_completions_contains_complete_dash_c() {
        let script = generate_to_string(Shell::Fish);
        assert!(
            script.contains("complete -c"),
            "fish completion script must contain 'complete -c'; got script starting: {}",
            &script[..script.len().min(200)]
        );
    }

    // ── WS1-TEST-04: PowerShell ───────────────────────────────────────────

    /// PowerShell completion output must be non-empty.
    #[test]
    fn test_powershell_completions_produces_output() {
        let output = generate_to_vec(Shell::PowerShell);
        assert!(
            !output.is_empty(),
            "powershell completion script should not be empty"
        );
    }

    // ── WS1-TEST-05: Binary name present in all shells ────────────────────

    /// Every completion script must contain the binary name "amplihack" so
    /// that the shell's completion machinery can associate it with the right
    /// command.
    #[test]
    fn test_all_completions_reference_binary_name() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell] {
            let script = generate_to_string(shell);
            assert!(
                script.contains("amplihack"),
                "{shell:?} completion script should contain the string 'amplihack'"
            );
        }
    }
}
