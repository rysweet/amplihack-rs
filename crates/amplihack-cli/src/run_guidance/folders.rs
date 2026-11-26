//! The folders `recipe guidance` reads (issue #3129).
//!
//! In this order and without duplicates: the folders
//! `AMPLIHACK_RUN_GUIDANCE_DIRS` lists, under the launcher's rules
//! ([`listed_dirs`]) but with missing and untrusted folders kept in the list
//! with their state, then the folder of the pull request named, resolved by
//! [`pr_dir_for`]. Only folders whose check passes are read.

use super::env::{DirCheck, check_guidance_dir, listed_dirs, pr_dir_for, warn_skipped};
use super::pr_key::{PrRef, pr_key, state_root};
use super::{Entry, GUIDANCE_DIRS_ENV, GuideError, is_new, read_entries};
use std::path::PathBuf;

/// One folder a reader reads, with what its last check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Folder {
    pub path: PathBuf,
    pub check: DirCheck,
}

/// The pull request named, resolved under the state root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrFolder {
    pub state_root: PathBuf,
    pub key: String,
    /// `<state_root>/auto-drive/<key>/guidance`.
    pub path: PathBuf,
}

/// The folders a reader reads, and the pull request's when one is named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Folders {
    pub list: Vec<Folder>,
    pub pr: Option<PrFolder>,
}

impl Folders {
    /// The folders `AMPLIHACK_RUN_GUIDANCE_DIRS` lists, then `pr`'s folder
    /// unless the list already holds it. Reads and creates nothing.
    ///
    /// # Errors
    ///
    /// [`GuideError::NotFound`] when a pull request is named and there is no
    /// usable state root.
    pub(crate) fn resolve(pr: Option<&PrRef>) -> Result<Self, GuideError> {
        let mut list: Vec<Folder> = std::env::var_os(GUIDANCE_DIRS_ENV)
            .map(|value| listed_dirs(&value))
            .unwrap_or_default()
            .into_iter()
            .map(|(path, check)| Folder { path, check })
            .collect();
        let Some(pr) = pr else {
            return Ok(Self { list, pr: None });
        };
        let state_root = state_root().map_err(|reason| {
            GuideError::NotFound(format!("cannot find the state folder: {reason}"))
        })?;
        let path = pr_dir_for(pr.number, &pr.owner, &pr.repo, &state_root);
        if !list.iter().any(|f| f.path == path) {
            list.push(Folder {
                path: path.clone(),
                check: check_guidance_dir(&path),
            });
        }
        Ok(Self {
            list,
            pr: Some(PrFolder {
                state_root,
                key: pr_key(&pr.owner, &pr.repo, pr.number),
                path,
            }),
        })
    }

    /// Says, once, on stderr, which folders are not read and why.
    pub(crate) fn warn_untrusted(&self) {
        for folder in &self.list {
            if let DirCheck::Untrusted(reason) = &folder.check {
                warn_skipped(&folder.path, reason);
            }
        }
    }

    /// The entries of the folders whose last check passed, oldest first,
    /// within the read limits of [`read_entries`].
    pub(crate) fn read(&self) -> Vec<Entry> {
        let trusted: Vec<PathBuf> = self
            .list
            .iter()
            .filter(|f| f.check == DirCheck::Trusted)
            .map(|f| f.path.clone())
            .collect();
        read_entries(&trusted)
    }

    /// Checks every folder again (one may have been created since), reads
    /// them, and returns the newest entry's name when it arrived after
    /// `since` ([`is_new`]).
    pub(crate) fn newest_new(&mut self, since: &str) -> Option<String> {
        for folder in &mut self.list {
            folder.check = check_guidance_dir(&folder.path);
        }
        self.read()
            .last()
            .map(|e| e.name.clone())
            .filter(|name| is_new(name, since))
    }
}
