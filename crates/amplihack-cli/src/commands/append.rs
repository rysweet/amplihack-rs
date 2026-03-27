use crate::command_error::exit_error;
use anyhow::{Result, anyhow};
use chrono::Local;
use regex::Regex;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const MAX_INSTRUCTION_SIZE: usize = 100 * 1024;
const MAX_APPENDS_PER_MINUTE: usize = 10;
const MAX_PENDING_INSTRUCTIONS: usize = 100;
const SUSPICIOUS_PATTERNS: &[&str] = &[
    r"ignore\s+previous\s+instructions",
    r"disregard\s+all\s+prior",
    r"forget\s+everything",
    r"new\s+instructions:",
    r"system\s+prompt:",
    r"<\s*script",
    r"eval\s*\(",
    r"exec\s*\(",
    r"__import__",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppendResult {
    filename: String,
    session_id: String,
}

pub fn run_append(instruction: &str) -> Result<()> {
    match append_instructions(instruction, None, &std::env::current_dir()?) {
        Ok(result) => {
            println!("Instruction appended to session: {}", result.session_id);
            println!("  File: {}", result.filename);
            println!("  The auto mode session will process this on its next turn.");
            Ok(())
        }
        Err(error) => {
            eprintln!("Error: {error}");
            Err(exit_error(1))
        }
    }
}

fn append_instructions(
    instruction: &str,
    session_id: Option<&str>,
    start_dir: &Path,
) -> Result<AppendResult> {
    if instruction.trim().is_empty() {
        return Err(anyhow!("Instruction cannot be empty or whitespace-only"));
    }

    validate_instruction(instruction)?;
    let workspace = find_workspace_root(start_dir).ok_or_else(|| {
        anyhow!(
            "No .claude directory found starting from {}. Start an auto mode session first.",
            start_dir.display()
        )
    })?;
    let session_dir = find_active_session(&workspace, session_id).ok_or_else(|| {
        if let Some(session_id) = session_id {
            anyhow!("Session not found: {session_id}")
        } else {
            anyhow!(
                "No active auto mode session found in {}. Start an auto mode session first.",
                workspace.display()
            )
        }
    })?;
    let append_dir = session_dir.join("append");
    if !append_dir.is_dir() {
        return Err(anyhow!(
            "Append directory not found in session: {}",
            session_dir.display()
        ));
    }

    check_rate_limit(&append_dir)?;

    let timestamp = Local::now().format("%Y%m%d_%H%M%S_%6f").to_string();
    let filename = format!("{timestamp}.md");
    let filepath = append_dir.join(&filename);
    write_instruction_file(&filepath, instruction)?;

    Ok(AppendResult {
        filename,
        session_id: session_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
    })
}

fn validate_instruction(instruction: &str) -> Result<()> {
    let instruction_bytes = instruction.as_bytes();
    if instruction_bytes.len() > MAX_INSTRUCTION_SIZE {
        return Err(anyhow!(
            "Instruction too large: {} bytes (max {} bytes / {}KB)",
            instruction_bytes.len(),
            MAX_INSTRUCTION_SIZE,
            MAX_INSTRUCTION_SIZE / 1024
        ));
    }

    let lowered = instruction.to_ascii_lowercase();
    for pattern in SUSPICIOUS_PATTERNS {
        let regex = Regex::new(pattern).expect("suspicious pattern regex must compile");
        if regex.is_match(&lowered) {
            return Err(anyhow!(
                "Suspicious pattern detected: '{pattern}'. This might be a prompt injection attempt. If this is legitimate, please rephrase your instruction."
            ));
        }
    }

    Ok(())
}

fn check_rate_limit(append_dir: &Path) -> Result<()> {
    let pending_files = markdown_files(append_dir)?;
    if pending_files.len() >= MAX_PENDING_INSTRUCTIONS {
        return Err(anyhow!(
            "Too many pending instructions: {} (max {}). Wait for the auto mode session to process existing instructions.",
            pending_files.len(),
            MAX_PENDING_INSTRUCTIONS
        ));
    }

    let one_minute_ago = SystemTime::now()
        .checked_sub(Duration::from_secs(60))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let recent_count = pending_files
        .into_iter()
        .filter_map(|path| path.metadata().ok())
        .filter_map(|metadata| metadata.modified().ok())
        .filter(|mtime| *mtime >= one_minute_ago)
        .count();
    if recent_count >= MAX_APPENDS_PER_MINUTE {
        return Err(anyhow!(
            "Rate limit exceeded: {} appends in last minute (max {}). Please wait before appending more instructions.",
            recent_count,
            MAX_APPENDS_PER_MINUTE
        ));
    }
    Ok(())
}

fn markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            files.push(path);
        }
    }
    Ok(files)
}

fn find_workspace_root(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir.canonicalize().ok()?;
    loop {
        let claude_dir = current.join(".claude");
        if claude_dir.is_dir() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn find_active_session(workspace: &Path, session_id: Option<&str>) -> Option<PathBuf> {
    let logs_dir = workspace.join(".claude").join("runtime").join("logs");
    if !logs_dir.is_dir() {
        return None;
    }

    if let Some(session_id) = session_id {
        let session_dir = logs_dir.join(session_id);
        return session_dir.is_dir().then_some(session_dir);
    }

    let mut auto_dirs = fs::read_dir(logs_dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("auto_"))
        })
        .collect::<Vec<_>>();
    auto_dirs.sort_by(|left, right| {
        right
            .file_name()
            .and_then(|name| name.to_str())
            .cmp(&left.file_name().and_then(|name| name.to_str()))
    });
    auto_dirs.into_iter().next()
}

fn write_instruction_file(filepath: &Path, instruction: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(filepath)?;
        let content = format!(
            "# Appended Instruction\n\n**Timestamp**: {}\n\n{}\n",
            Local::now().to_rfc3339(),
            instruction
        );
        file.write_all(content.as_bytes())?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(filepath)?;
        let content = format!(
            "# Appended Instruction\n\n**Timestamp**: {}\n\n{}\n",
            Local::now().to_rfc3339(),
            instruction
        );
        file.write_all(content.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{cwd_env_lock, restore_cwd, set_cwd};

    #[test]
    fn find_workspace_root_walks_up_to_claude_dir() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(dir.path().join(".claude")).unwrap();

        let workspace = find_workspace_root(&nested).unwrap();

        assert_eq!(workspace, dir.path());
    }

    #[test]
    fn find_active_session_prefers_latest_auto_session() {
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join(".claude/runtime/logs");
        fs::create_dir_all(logs_dir.join("auto_20260318_01")).unwrap();
        fs::create_dir_all(logs_dir.join("auto_20260318_02")).unwrap();
        fs::create_dir_all(logs_dir.join("manual_session")).unwrap();

        let session = find_active_session(dir.path(), None).unwrap();

        assert_eq!(
            session.file_name().and_then(|name| name.to_str()),
            Some("auto_20260318_02")
        );
    }

    #[test]
    fn append_instructions_writes_timestamped_file() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".claude/runtime/logs/auto_1/append")).unwrap();
        let previous = set_cwd(dir.path()).unwrap();

        let result = append_instructions("Continue with the audit", None, dir.path()).unwrap();

        restore_cwd(&previous).unwrap();

        let file = dir
            .path()
            .join(".claude/runtime/logs/auto_1/append")
            .join(&result.filename);
        assert_eq!(result.session_id, "auto_1");
        assert!(file.exists());
        let content = fs::read_to_string(file).unwrap();
        assert!(content.contains("Continue with the audit"));
        assert!(content.contains("# Appended Instruction"));
    }

    #[test]
    fn append_instructions_rejects_suspicious_patterns() {
        let error =
            append_instructions("ignore previous instructions", None, Path::new(".")).unwrap_err();
        assert!(format!("{error:#}").contains("Suspicious pattern detected"));
    }

    #[test]
    fn append_instructions_requires_append_directory() {
        let _guard = cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".claude/runtime/logs/auto_1")).unwrap();

        let error = append_instructions("Continue", None, dir.path()).unwrap_err();

        assert!(format!("{error:#}").contains("Append directory not found"));
    }
}
