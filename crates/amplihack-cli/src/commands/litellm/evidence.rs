use super::types::RunSummaryV1;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) struct CommittedEvidence {
    pub path: PathBuf,
    pub sha256: String,
}

pub(crate) fn commit(
    directory: &Path,
    summary: &RunSummaryV1,
) -> Result<CommittedEvidence, String> {
    create_owner_only_directory(directory)?;

    let timestamp = summary.created_at.replace([':', '.'], "");
    let filename = format!("{timestamp}-{}-litellm-verify-live.jsonl", summary.run_id);
    let destination = directory.join(filename);
    let staging = directory.join(format!(".{}.tmp", summary.run_id));
    let mut body = serde_json::to_vec(summary).map_err(|_| "serialize evidence".to_string())?;
    body.push(b'\n');

    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staging)
            .map_err(|error| safe_io("create evidence staging file", error))?;
        set_owner_only_file(&staging)?;
        file.write_all(&body)
            .map_err(|error| safe_io("write evidence", error))?;
        file.sync_all()
            .map_err(|error| safe_io("fsync evidence", error))?;
        fs::hard_link(&staging, &destination)
            .map_err(|error| safe_io("publish evidence without replacement", error))?;
        fs::remove_file(&staging)
            .map_err(|error| safe_io("remove evidence staging link", error))?;
        let parent = OpenOptions::new()
            .read(true)
            .open(directory)
            .map_err(|error| safe_io("open evidence directory", error))?;
        parent
            .sync_all()
            .map_err(|error| safe_io("fsync evidence directory", error))?;
        Ok(CommittedEvidence {
            path: destination,
            sha256: hex(&Sha256::digest(&body)),
        })
    })();

    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

fn safe_io(action: &str, error: std::io::Error) -> String {
    format!("{action}: {}", error.kind())
}

#[cfg(unix)]
fn create_owner_only_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(path)
            .map_err(|error| safe_io("create evidence directory", error))?;
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|error| safe_io("inspect evidence directory", error))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(
            "evidence directory must be a non-symlink directory owned by the current user with mode 0700 or stricter"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_owner_only_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| safe_io("create evidence directory", error))
}

#[cfg(unix)]
fn set_owner_only_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| safe_io("secure evidence file", error))
}

#[cfg(not(unix))]
fn set_owner_only_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::commands::litellm::types::{RunStatus, RunSummaryV1};
    use std::os::unix::fs::PermissionsExt;

    fn summary() -> RunSummaryV1 {
        RunSummaryV1 {
            schema: "amplihack.litellm.run-summary",
            schema_version: 1,
            created_at: "2026-09-02T14:10:10.145Z".to_string(),
            run_id: "00000000-0000-4000-8000-000000000001".to_string(),
            execution_context: "trusted-host",
            repository_commit: "0".repeat(40),
            repository_context_sha256: "1".repeat(64),
            pr_number: 1445,
            status: RunStatus::Passed,
            exit_code: 0,
            clients: Vec::new(),
            negative_cases_passed: 0,
            negative_cases_failed: 0,
            evidence_path: None,
            evidence_sha256: None,
            credentials_read: false,
        }
    }

    #[test]
    fn commits_owner_only_evidence_without_replacement() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("private/evidence");
        let first = commit(&directory, &summary()).expect("first evidence commit");
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&first.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(commit(&directory, &summary()).is_err());
        assert_eq!(
            fs::read_dir(&directory)
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            1
        );
    }

    #[test]
    fn rejects_existing_shared_directory() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("shared");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(commit(&directory, &summary()).is_err());
        assert!(fs::read_dir(directory).unwrap().next().is_none());
    }
}
