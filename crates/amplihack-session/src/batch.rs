//! Batched file operations.

use crate::config::{Result, SessionError};
use crate::file_utils::{safe_copy_file, safe_move_file, safe_write_file};
use std::path::{Component, Path, PathBuf};

/// A single queued operation in a [`BatchFileOperations`].
#[derive(Debug, Clone)]
pub enum BatchOp {
    Write { path: PathBuf, content: String },
    Copy { src: PathBuf, dst: PathBuf },
    Move { src: PathBuf, dst: PathBuf },
}

/// Builder/queue for executing multiple file operations together.
///
/// **Security:** absolute paths and `..` components are rejected with
/// [`crate::SessionError::PathEscape`] before being queued. All paths are
/// resolved relative to `base_dir` at execution time.
#[derive(Debug)]
pub struct BatchFileOperations {
    base_dir: PathBuf,
    ops: Vec<BatchOp>,
    verify_all: bool,
}

fn check_within_base(base: &Path, p: &Path) -> Result<PathBuf> {
    if p.is_absolute() {
        return Err(SessionError::PathEscape(p.to_path_buf()));
    }
    for c in p.components() {
        match c {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SessionError::PathEscape(p.to_path_buf()));
            }
            _ => {}
        }
    }
    Ok(base.join(p))
}

impl BatchFileOperations {
    /// Construct a new batch confined to `base_dir`.
    pub fn new(base_dir: impl Into<PathBuf>, verify_all: bool) -> Self {
        Self {
            base_dir: base_dir.into(),
            ops: Vec::new(),
            verify_all,
        }
    }

    /// Queue a write operation.
    pub fn add_write(
        &mut self,
        path: impl Into<PathBuf>,
        content: impl Into<String>,
    ) -> Result<()> {
        let rel = path.into();
        let abs = check_within_base(&self.base_dir, &rel)?;
        self.ops.push(BatchOp::Write {
            path: abs,
            content: content.into(),
        });
        Ok(())
    }

    /// Queue a copy operation.
    pub fn add_copy(&mut self, src: impl Into<PathBuf>, dst: impl Into<PathBuf>) -> Result<()> {
        let src_rel = src.into();
        let dst_rel = dst.into();
        let src_abs = check_within_base(&self.base_dir, &src_rel)?;
        let dst_abs = check_within_base(&self.base_dir, &dst_rel)?;
        self.ops.push(BatchOp::Copy {
            src: src_abs,
            dst: dst_abs,
        });
        Ok(())
    }

    /// Queue a move operation.
    pub fn add_move(&mut self, src: impl Into<PathBuf>, dst: impl Into<PathBuf>) -> Result<()> {
        let src_rel = src.into();
        let dst_rel = dst.into();
        let src_abs = check_within_base(&self.base_dir, &src_rel)?;
        let dst_abs = check_within_base(&self.base_dir, &dst_rel)?;
        self.ops.push(BatchOp::Move {
            src: src_abs,
            dst: dst_abs,
        });
        Ok(())
    }

    /// Execute all queued operations; returns one Result per operation.
    pub fn execute(&mut self) -> Vec<Result<()>> {
        let verify = self.verify_all;
        let ops = std::mem::take(&mut self.ops);
        ops.into_iter()
            .map(|op| match op {
                BatchOp::Write { path, content } => {
                    safe_write_file(&path, &content, true, false, verify)
                }
                BatchOp::Copy { src, dst } => safe_copy_file(&src, &dst, verify),
                BatchOp::Move { src, dst } => safe_move_file(&src, &dst, verify),
            })
            .collect()
    }

    /// Number of queued (un-executed) operations.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}
