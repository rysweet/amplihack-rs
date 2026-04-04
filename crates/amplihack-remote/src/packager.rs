//! Context packaging with secret scanning.
//!
//! Creates secure tar.gz archives of project context for remote
//! execution, including a git bundle and `.claude` configuration.
//! Scans for hardcoded secrets before packaging.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, warn};

use crate::error::{ErrorContext, RemoteError};

/// A secret detected in source files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMatch {
    pub file_path: String,
    pub line_number: usize,
    /// First 100 chars of the offending line.
    pub line_content: String,
    pub pattern_name: String,
}

/// Secret detection patterns.
static SECRET_PATTERNS: &[(&str, &str)] = &[
    (
        "anthropic_key",
        r#"ANTHROPIC_API_KEY\s*=\s*["']sk-ant-[^"']+["']"#,
    ),
    ("openai_key", r#"OPENAI_API_KEY\s*=\s*["']sk-[^"']+["']"#),
    ("anthropic_key_generic", r"sk-ant-[a-zA-Z0-9\-_]{20,}"),
    ("openai_key_generic", r"sk-[a-zA-Z0-9\-_]{20,}"),
    ("github_pat", r"ghp_[a-zA-Z0-9]{36}"),
    ("azure_key", r#"AZURE_[A-Z_]*KEY\s*=\s*["'][^"']+["']"#),
    ("aws_key", r#"AWS_[A-Z_]*KEY\s*=\s*["'][^"']+["']"#),
    (
        "api_key_generic",
        r#"api[_\-]?key\s*[=:]\s*["'][^"']{20,}["']"#,
    ),
    ("password", r#"password\s*[=:]\s*["'][^"']+["']"#),
    ("token", r#"token\s*[=:]\s*["'][^"']{20,}["']"#),
    ("bearer_token", r"[Bb]earer\s+[a-zA-Z0-9\-_\.]{20,}"),
];

/// File patterns to always exclude from scanning/archiving.
const EXCLUDED_PATTERNS: &[&str] = &[
    ".env*",
    "*credentials*",
    "*secret*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    ".ssh/*",
    ".aws/*",
    ".azure/*",
    ".config/gh/*",
    "node_modules/*",
    "__pycache__/*",
    ".venv/*",
    "venv/*",
    "*.pyc",
    ".git/*",
    ".DS_Store",
];

/// Packages project context for remote execution.
pub struct ContextPackager {
    repo_path: PathBuf,
    max_size_bytes: u64,
    skip_secret_scan: bool,
    temp_dir: Option<PathBuf>,
}

impl ContextPackager {
    pub fn new(repo_path: &Path, max_size_mb: u64, skip_secret_scan: bool) -> Self {
        Self {
            repo_path: repo_path
                .canonicalize()
                .unwrap_or_else(|_| repo_path.to_path_buf()),
            max_size_bytes: max_size_mb * 1024 * 1024,
            skip_secret_scan,
            temp_dir: None,
        }
    }

    /// Scan tracked files for hardcoded secrets.
    pub async fn scan_secrets(&self) -> Result<Vec<SecretMatch>, RemoteError> {
        let output = Command::new("git")
            .args(["ls-files"])
            .current_dir(&self.repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| {
                RemoteError::packaging_ctx(
                    format!("Failed to list git files: {e}"),
                    ErrorContext::new().insert("repo_path", self.repo_path.display().to_string()),
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RemoteError::packaging_ctx(
                format!("git ls-files failed: {stderr}"),
                ErrorContext::new().insert("repo_path", self.repo_path.display().to_string()),
            ));
        }

        let file_list = String::from_utf8_lossy(&output.stdout);
        let compiled: Vec<(&str, Regex)> = SECRET_PATTERNS
            .iter()
            .filter_map(|(name, pat)| Regex::new(pat).ok().map(|r| (*name, r)))
            .collect();

        let mut matches = Vec::new();

        for rel_path in file_list.lines() {
            if rel_path.is_empty() {
                continue;
            }
            if is_excluded(rel_path) {
                continue;
            }

            let full = self.repo_path.join(rel_path);
            if !full.is_file() {
                continue;
            }
            if is_binary(&full) {
                continue;
            }

            let content = match std::fs::read_to_string(&full) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        path = %rel_path,
                        error = %e,
                        "could not read file for secret scan"
                    );
                    continue;
                }
            };

            for (line_num, line) in content.lines().enumerate().map(|(i, l)| (i + 1, l)) {
                for (pat_name, re) in &compiled {
                    if re.is_match(line) {
                        let truncated: String = line.chars().take(100).collect();
                        matches.push(SecretMatch {
                            file_path: rel_path.to_string(),
                            line_number: line_num,
                            line_content: truncated,
                            pattern_name: pat_name.to_string(),
                        });
                    }
                }
            }
        }

        Ok(matches)
    }

    /// Create a git bundle containing all branches.
    pub async fn create_bundle(&mut self) -> Result<PathBuf, RemoteError> {
        let temp_dir = self.ensure_temp_dir()?;
        let bundle_path = temp_dir.join("repo.bundle");

        let output = Command::new("git")
            .args([
                "bundle",
                "create",
                bundle_path.to_str().unwrap_or("repo.bundle"),
                "--all",
            ])
            .current_dir(&self.repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| RemoteError::packaging(format!("Failed to create git bundle: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RemoteError::packaging_ctx(
                format!("git bundle create failed: {stderr}"),
                ErrorContext::new().insert("repo_path", self.repo_path.display().to_string()),
            ));
        }

        if !bundle_path.exists() {
            return Err(RemoteError::packaging("Git bundle file not created"));
        }

        // Verify bundle
        let verify = Command::new("git")
            .args([
                "bundle",
                "verify",
                bundle_path.to_str().unwrap_or("repo.bundle"),
            ])
            .current_dir(&self.repo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| RemoteError::packaging(format!("bundle verify failed: {e}")))?;

        if !verify.status.success() {
            let stderr = String::from_utf8_lossy(&verify.stderr);
            return Err(RemoteError::packaging(format!(
                "Git bundle verification failed: {stderr}"
            )));
        }

        Ok(bundle_path)
    }

    /// Create the full context archive (`context.tar.gz`).
    ///
    /// Steps: scan secrets → create bundle → tar.gz bundle + .claude →
    /// verify size.
    pub async fn package(&mut self) -> Result<PathBuf, RemoteError> {
        // 1. Secret scan
        if !self.skip_secret_scan {
            let secrets = self.scan_secrets().await?;
            if !secrets.is_empty() {
                let details: Vec<String> = secrets
                    .iter()
                    .take(10)
                    .map(|s| {
                        format!(
                            "  - {}:{} ({}): {}",
                            s.file_path, s.line_number, s.pattern_name, s.line_content,
                        )
                    })
                    .collect();
                let mut msg = format!(
                    "Detected {} potential secret(s):\n{}",
                    secrets.len(),
                    details.join("\n"),
                );
                if secrets.len() > 10 {
                    msg.push_str(&format!("\n  ... and {} more", secrets.len() - 10));
                }
                return Err(RemoteError::packaging(msg));
            }
        } else {
            debug!("secret scan skipped");
        }

        // 2. Git bundle (placed into temp_dir)
        let _bundle_path = self.create_bundle().await?;

        // 3. Verify .claude directory exists
        let claude_dir = self.repo_path.join(".claude");
        if !claude_dir.is_dir() {
            return Err(RemoteError::packaging_ctx(
                ".claude directory not found in repository",
                ErrorContext::new().insert("repo_path", self.repo_path.display().to_string()),
            ));
        }

        // 4. Build tar.gz via `tar` command
        let temp_dir = self.ensure_temp_dir()?;
        let archive_path = temp_dir.join("context.tar.gz");

        let status = Command::new("tar")
            .args([
                "czf",
                archive_path.to_str().unwrap_or("context.tar.gz"),
                "-C",
                temp_dir.to_str().unwrap_or("."),
                "repo.bundle",
                "-C",
                self.repo_path.to_str().unwrap_or("."),
                ".claude",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .await
            .map_err(|e| RemoteError::packaging(format!("tar command failed: {e}")))?;

        if !status.success() {
            return Err(RemoteError::packaging("Failed to create tar archive"));
        }

        // 5. Verify size
        let meta = std::fs::metadata(&archive_path)
            .map_err(|e| RemoteError::packaging(format!("Cannot stat archive: {e}")))?;

        if meta.len() > self.max_size_bytes {
            let size_mb = meta.len() as f64 / 1024.0 / 1024.0;
            let limit_mb = self.max_size_bytes as f64 / 1024.0 / 1024.0;
            return Err(RemoteError::packaging(format!(
                "Archive size ({size_mb:.1} MB) exceeds limit \
                 ({limit_mb:.1} MB)"
            )));
        }

        debug!(
            size_bytes = meta.len(),
            path = %archive_path.display(),
            "context archive created"
        );

        Ok(archive_path)
    }

    /// Remove temporary files.
    pub fn cleanup(&mut self) {
        if let Some(ref dir) = self.temp_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
        self.temp_dir = None;
    }

    fn ensure_temp_dir(&mut self) -> Result<PathBuf, RemoteError> {
        if self.temp_dir.is_none() {
            let dir = tempfile::tempdir()
                .map_err(|e| RemoteError::packaging(format!("Failed to create temp dir: {e}")))?
                .keep();
            self.temp_dir = Some(dir);
        }
        Ok(self.temp_dir.clone().unwrap())
    }
}

impl Drop for ContextPackager {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn is_excluded(path: &str) -> bool {
    for pattern in EXCLUDED_PATTERNS {
        if glob_match(pattern, path) {
            return true;
        }
    }
    false
}

/// Simple glob matching (supports `*` prefix/suffix and `*x*`).
fn glob_match(pattern: &str, path: &str) -> bool {
    if pattern == path {
        return true;
    }
    let stripped = pattern.trim_matches('*');
    if pattern.starts_with('*') && pattern.ends_with('*') && !stripped.is_empty() {
        // "*secret*" style — substring match
        return path.contains(stripped);
    }
    // "*.ext" style
    if let Some(suffix) = pattern.strip_prefix('*')
        && path.ends_with(suffix)
    {
        return true;
    }
    // "dir/*" style
    if let Some(prefix) = pattern.strip_suffix('*')
        && path.starts_with(prefix)
    {
        return true;
    }
    false
}

fn is_binary(path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => {
            let check_len = bytes.len().min(1024);
            bytes[..check_len].contains(&0u8)
        }
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_patterns_match() {
        assert!(is_excluded(".env"));
        assert!(is_excluded(".env.local"));
        assert!(is_excluded("node_modules/foo"));
        assert!(is_excluded(".git/objects"));
        assert!(is_excluded("secret.txt"));
        assert!(!is_excluded("src/main.rs"));
    }

    #[test]
    fn binary_detection() {
        let dir = tempfile::tempdir().unwrap();

        let text_file = dir.path().join("text.txt");
        std::fs::write(&text_file, b"hello world").unwrap();
        assert!(!is_binary(&text_file));

        let bin_file = dir.path().join("binary.bin");
        std::fs::write(&bin_file, b"hello\x00world").unwrap();
        assert!(is_binary(&bin_file));
    }

    #[test]
    fn glob_match_works() {
        assert!(glob_match("*.pyc", "foo.pyc"));
        assert!(glob_match(".env*", ".env.local"));
        assert!(glob_match(".git/*", ".git/objects"));
        assert!(!glob_match("*.pyc", "foo.py"));
    }

    #[test]
    fn secret_patterns_compile() {
        for (name, pat) in SECRET_PATTERNS {
            assert!(Regex::new(pat).is_ok(), "pattern {name} failed to compile");
        }
    }

    #[test]
    fn secret_match_serialization() {
        let m = SecretMatch {
            file_path: "src/main.rs".into(),
            line_number: 42,
            line_content: "let key = sk-ant-...".into(),
            pattern_name: "anthropic_key_generic".into(),
        };
        let json = serde_json::to_string(&m).unwrap();
        let m2: SecretMatch = serde_json::from_str(&json).unwrap();
        assert_eq!(m2.file_path, "src/main.rs");
    }
}
