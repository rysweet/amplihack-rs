//! Cross-platform terminal launcher for file tailing.
//!
//! Ports Python `amplihack/utils/terminal_launcher.py`:
//! - OS detection (linux/macos/windows)
//! - Launch terminal with `tail -f` command
//! - Multiple terminal emulator fallback on Linux

use std::path::Path;
use std::process::Command;

/// Detected operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsType {
    Linux,
    MacOs,
    Windows,
}

/// Detect the operating system.
pub fn detect_os() -> OsType {
    if cfg!(target_os = "macos") {
        OsType::MacOs
    } else if cfg!(target_os = "windows") {
        OsType::Windows
    } else {
        OsType::Linux
    }
}

/// Result of a terminal launch attempt.
#[derive(Debug)]
pub struct LaunchResult {
    pub success: bool,
    pub terminal: Option<String>,
}

/// Launch a terminal window tailing the specified file.
pub fn launch_tail_terminal(file_path: &Path) -> LaunchResult {
    let os = detect_os();
    let abs_path = std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());

    match os {
        OsType::MacOs => launch_macos(&abs_path),
        OsType::Linux => launch_linux(&abs_path),
        OsType::Windows => launch_windows(&abs_path),
    }
}

/// Sanitize a file path for safe shell interpolation.
///
/// Rejects paths containing shell metacharacters to prevent command injection.
fn validate_path_for_shell(file_path: &Path) -> Result<String, &'static str> {
    let s = file_path.to_string_lossy();
    let dangerous = [
        ';', '|', '&', '$', '`', '(', ')', '{', '}', '<', '>', '!', '\n', '\r',
    ];
    if s.chars().any(|c| dangerous.contains(&c)) {
        return Err("path contains shell metacharacters");
    }
    Ok(s.into_owned())
}

fn launch_macos(file_path: &Path) -> LaunchResult {
    let safe_path = match validate_path_for_shell(file_path) {
        Ok(p) => p,
        Err(_) => {
            return LaunchResult {
                success: false,
                terminal: None,
            };
        }
    };
    let script = format!(
        "tell app \"Terminal\" to do script \"tail -f '{}'\"",
        safe_path.replace('\'', "'\\''")
    );
    match Command::new("osascript")
        .args(["-e", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => LaunchResult {
            success: true,
            terminal: Some("Terminal.app".to_string()),
        },
        Err(_) => LaunchResult {
            success: false,
            terminal: None,
        },
    }
}

fn launch_linux(file_path: &Path) -> LaunchResult {
    let safe_path = match validate_path_for_shell(file_path) {
        Ok(p) => p,
        Err(_) => {
            return LaunchResult {
                success: false,
                terminal: None,
            };
        }
    };
    let quoted = safe_path.replace('\'', "'\\''");
    let tail_cmd = format!("tail -f '{quoted}'");

    // Try terminal emulators in preference order
    let terminals = [
        ("x-terminal-emulator", vec!["-e"]),
        ("gnome-terminal", vec!["--", "bash", "-c"]),
        ("konsole", vec!["-e"]),
        ("xfce4-terminal", vec!["-e"]),
        ("xterm", vec!["-e"]),
        ("mate-terminal", vec!["-e"]),
    ];

    for (term, args) in &terminals {
        if which_exists(term) {
            let mut cmd = Command::new(term);
            for arg in args {
                cmd.arg(arg);
            }
            if *term == "gnome-terminal" {
                cmd.arg(format!("{tail_cmd}; exec bash"));
            } else {
                cmd.arg(&tail_cmd);
            }
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::null());

            if cmd.spawn().is_ok() {
                return LaunchResult {
                    success: true,
                    terminal: Some(term.to_string()),
                };
            }
        }
    }

    LaunchResult {
        success: false,
        terminal: None,
    }
}

fn launch_windows(file_path: &Path) -> LaunchResult {
    let safe_path = match validate_path_for_shell(file_path) {
        Ok(p) => p,
        Err(_) => {
            return LaunchResult {
                success: false,
                terminal: None,
            };
        }
    };
    let ps_cmd = format!("Get-Content '{}' -Wait", safe_path.replace('\'', "''"));

    let terminals = [
        (
            "wt",
            vec![
                "-p",
                "Windows PowerShell",
                "--",
                "powershell",
                "-NoExit",
                "-Command",
            ],
        ),
        ("powershell", vec!["-NoExit", "-Command"]),
    ];

    for (term, args) in &terminals {
        if which_exists(term) {
            let mut cmd = Command::new(term);
            for arg in args {
                cmd.arg(arg);
            }
            cmd.arg(&ps_cmd);
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::null());

            if cmd.spawn().is_ok() {
                return LaunchResult {
                    success: true,
                    terminal: Some(term.to_string()),
                };
            }
        }
    }

    LaunchResult {
        success: false,
        terminal: None,
    }
}

/// Check if a command exists on PATH.
fn which_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_os_returns_linux_on_linux() {
        let os = detect_os();
        #[cfg(target_os = "linux")]
        assert_eq!(os, OsType::Linux);
        #[cfg(target_os = "macos")]
        assert_eq!(os, OsType::MacOs);
        #[cfg(target_os = "windows")]
        assert_eq!(os, OsType::Windows);
    }

    #[test]
    fn launch_result_debug() {
        let r = LaunchResult {
            success: false,
            terminal: None,
        };
        let s = format!("{r:?}");
        assert!(s.contains("false"));
    }

    #[test]
    fn which_exists_false_for_nonsense() {
        assert!(!which_exists("totally_nonexistent_binary_xyz123"));
    }

    #[test]
    fn which_exists_true_for_sh() {
        // sh should exist on any Unix
        #[cfg(unix)]
        assert!(which_exists("sh"));
    }

    #[test]
    fn validate_path_safe() {
        let p = Path::new("/tmp/my-log-file.log");
        assert!(validate_path_for_shell(p).is_ok());
    }

    #[test]
    fn validate_path_rejects_semicolon() {
        let p = Path::new("/tmp/file; rm -rf /");
        assert!(validate_path_for_shell(p).is_err());
    }

    #[test]
    fn validate_path_rejects_pipe() {
        let p = Path::new("/tmp/file | cat /etc/passwd");
        assert!(validate_path_for_shell(p).is_err());
    }

    #[test]
    fn validate_path_rejects_dollar() {
        let p = Path::new("/tmp/$(whoami)");
        assert!(validate_path_for_shell(p).is_err());
    }

    #[test]
    fn validate_path_rejects_backtick() {
        let p = Path::new("/tmp/`id`");
        assert!(validate_path_for_shell(p).is_err());
    }
}
