//! Immutable cleanup authorization, persisted before any backup unlink.
use super::*;
use std::{collections::BTreeMap, path::Component};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", deny_unknown_fields)]
enum Kind {
    Directory,
    File { sha256: String },
    Symlink { target: PathBuf },
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: PathBuf,
    kind: Kind,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    schema_version: u32,
    transaction: String,
    codex_home: PathBuf,
    original_digest: Option<String>,
    entries: Vec<Entry>,
}

fn entry_kind(path: &Path, meta: &fs::Metadata) -> Result<Kind> {
    let kind = if meta.file_type().is_symlink() {
        Kind::Symlink {
            target: fs::read_link(path)?,
        }
    } else if meta.is_file() {
        // Open without following a substituted final symlink.
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(path)?;
        ensure!(
            file.metadata()?.is_file(),
            "nonregular backup entry; record retained"
        );
        let mut hash = Sha256::new();
        let bytes = std::io::copy(&mut file, &mut hash)?;
        #[cfg(test)]
        super::cleanup_performance_tests::HASHED_BYTES.with(|n| n.set(n.get() + bytes));
        #[cfg(not(test))]
        let _ = bytes;
        Kind::File {
            sha256: format!("{:x}", hash.finalize()),
        }
    } else {
        ensure!(meta.is_dir(), "unsupported backup entry; record retained");
        Kind::Directory
    };
    Ok(kind)
}

fn scan(base: &Path) -> Result<Vec<Entry>> {
    #[cfg(test)]
    super::cleanup_performance_tests::SCANS.with(|n| n.set(n.get() + 1));
    fn walk(base: &Path, relative: &Path, entries: &mut Vec<Entry>) -> Result<()> {
        let path = base.join(relative);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound && relative.as_os_str().is_empty() =>
            {
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let kind = entry_kind(&path, &meta)?;
        let directory = kind == Kind::Directory;
        entries.push(Entry {
            path: relative.to_path_buf(),
            kind,
        });
        if directory {
            let mut children = fs::read_dir(&path)?.collect::<std::io::Result<Vec<_>>>()?;
            children.sort_by_key(|e| e.file_name());
            for child in children {
                walk(base, &relative.join(child.file_name()), entries)?;
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    walk(base, Path::new(""), &mut entries)?;
    Ok(entries)
}

/// Reject unsupported entry types before the framed digest reader opens them.
pub(super) fn original_digest(path: &Path) -> Result<String> {
    scan(path)?;
    digest(path)
}

fn inventory(pending: &Value, home: &Path) -> Result<Inventory> {
    let record: Inventory = serde_json::from_value(pending["backup_cleanup"].clone())
        .context("invalid backup cleanup inventory; record retained")?;
    ensure!(
        record.schema_version == 1
            && Some(record.transaction.as_str()) == pending["transaction"].as_str()
            && record.codex_home == home
            && record.original_digest.as_deref() == pending["ledger"]["package_digest"].as_str(),
        "backup cleanup scope or transaction changed; record retained"
    );
    ensure!(
        record
            .original_digest
            .as_ref()
            .is_none_or(|hash| resources::current_digest(hash)),
        "invalid backup cleanup original digest; record retained"
    );
    let mut paths = BTreeMap::new();
    for entry in &record.entries {
        ensure!(
            entry
                .path
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
                && !entry.path.as_os_str().as_encoded_bytes().contains(&0)
                && paths.insert(entry.path.clone(), &entry.kind).is_none(),
            "unsafe or duplicate backup cleanup path; record retained"
        );
        if let Kind::File { sha256 } = &entry.kind {
            ensure!(
                sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid backup cleanup file hash; record retained"
            );
        }
    }
    ensure!(
        if pending["had_package"] == true {
            paths.get(Path::new("")) == Some(&&Kind::Directory) && record.original_digest.is_some()
        } else {
            paths.is_empty()
        },
        "incomplete backup cleanup root; record retained"
    );
    for path in paths.keys().filter(|p| !p.as_os_str().is_empty()) {
        ensure!(
            paths.get(path.parent().context("backup entry parent missing")?)
                == Some(&&Kind::Directory),
            "incomplete backup cleanup directory inventory; record retained"
        );
    }
    Ok(record)
}

/// Missing recorded entries are allowed; all survivors must match exactly.
pub(super) fn validate(pending: &Value, root: &Path, home: &Path) -> Result<()> {
    let record = inventory(pending, home)?;
    let expected: BTreeMap<_, _> = record.entries.iter().map(|e| (&e.path, &e.kind)).collect();
    for entry in scan(&root.join("previous-package"))? {
        ensure!(
            expected.get(&entry.path) == Some(&&entry.kind),
            "foreign backup survivor changed or unrecorded; cleanup record retained"
        );
    }
    Ok(())
}

pub(super) fn finish(pending: &mut Value, root: &Path, home: &Path) -> Result<()> {
    let backup = root.join("previous-package");
    if pending.get("backup_cleanup").is_none() {
        let entries = scan(&backup)?;
        if pending["had_package"] == true {
            ensure!(
                !entries.is_empty()
                    && Some(digest(&backup)?.as_str())
                        == pending["ledger"]["package_digest"].as_str(),
                "backup lacks complete original ownership proof; reconcile manually; record retained"
            );
        } else {
            ensure!(entries.is_empty(), "unowned backup; record retained");
        }
        let record = Inventory {
            schema_version: 1,
            transaction: pending["transaction"]
                .as_str()
                .context("transaction missing")?
                .into(),
            codex_home: home.into(),
            original_digest: pending["ledger"]["package_digest"]
                .as_str()
                .map(str::to_owned),
            entries,
        };
        // Revalidate the complete snapshot before publishing immutable authorization.
        ensure!(
            scan(&backup)? == record.entries,
            "backup changed during inventory; record retained"
        );
        let original = pending.clone();
        pending["backup_cleanup"] = serde_json::to_value(record)?;
        recovery::preflight(&original, root, home)?;
        validate(pending, root, home)?;
        atomic_json(&root.join("pending.json"), pending, Some(original))?;
    }
    // A previous publication may have failed its directory barrier. Recheck it on retry.
    storage::sync_path(&root.join("pending.json"))?;
    storage::sync_path(root)?;
    validate(pending, root, home)?;
    let guard = cleanup_guard::Guard::capture(pending, root, home)?;
    let mut entries = inventory(pending, home)?.entries;
    entries.sort_by_key(|e| std::cmp::Reverse(e.path.components().count()));
    for entry in entries {
        guard.check(pending, root, home)?;
        // Check every ancestor before opening an entry: a directory replaced
        // by a symlink must never redirect a hash or unlink outside the backup.
        for ancestor in entry.path.ancestors().skip(1) {
            let meta = match fs::symlink_metadata(backup.join(ancestor)) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(e.into()),
            };
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "foreign backup ancestor changed; cleanup record retained"
            );
        }
        let path = backup.join(&entry.path);
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
            Ok(meta) => {
                ensure!(
                    entry_kind(&path, &meta)? == entry.kind,
                    "foreign backup survivor changed; cleanup record retained"
                );
            }
        }
        if entry.kind == Kind::Directory {
            fs::remove_dir(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
        storage::sync_path(path.parent().context("backup removal parent missing")?)?;
    }
    storage::sync_path(root)
        .context("Codex backup cleanup synchronization failed; record retained")?;
    recovery::preflight(pending, root, home)?;
    path_scope::check()?;
    fs::remove_file(root.join("pending.json"))?;
    storage::sync_path(root)
        .context("Codex committed cleanup synchronization failed; backup may already be removed")
}
