//! Bundle packager — creates distributable archives from an agent directory.
//!
//! Ported from `amplihack/bundle_generator/packager.py` and `base_packager.py`.
//! Provides a [`Packager`] trait with a filesystem-backed [`FileSystemPackager`]
//! that produces `.tar.gz` and `.zip` bundles.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::BundleError;
use super::models::{PackageFormat, PackagedBundle};

/// Shorthand for packaging errors.
fn pkg_err(msg: impl std::fmt::Display) -> BundleError {
    BundleError::packaging(msg.to_string())
}

/// Result of a packaging operation.
#[derive(Debug, Clone)]
pub struct PackageResult {
    pub format: PackageFormat,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub checksum: String,
}

/// Options controlling packaging behaviour.
#[derive(Debug, Clone, Default)]
pub struct PackageOptions {
    /// Override the output directory (defaults to parent of source dir).
    pub output_dir: Option<PathBuf>,
    /// Extra metadata to embed in the manifest.
    pub metadata: HashMap<String, String>,
    /// If true, include hidden files/dirs.
    pub include_hidden: bool,
}

/// Abstraction over different packaging back-ends.
pub trait Packager {
    /// Package `source_dir` into the given `format`.
    fn package(
        &self,
        source_dir: &Path,
        format: PackageFormat,
        options: &PackageOptions,
    ) -> Result<PackageResult, BundleError>;
    /// List the contents of an existing package.
    fn list_contents(&self, package_path: &Path) -> Result<Vec<String>, BundleError>;
}

/// Packages a directory into `.tar.gz` or `.zip` archives on the local filesystem.
pub struct FileSystemPackager {
    work_dir: Option<PathBuf>,
}

fn dir_basename(p: &Path) -> &str {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("bundle")
}

impl FileSystemPackager {
    /// Create a new packager. If `work_dir` is `None`, the parent of the source directory is used.
    pub fn new(work_dir: Option<PathBuf>) -> Self {
        Self { work_dir }
    }

    fn create_tarball(
        &self,
        source: &Path,
        dest: &Path,
        hidden: bool,
    ) -> Result<PathBuf, BundleError> {
        let archive_path = dest.join(format!("{}.tar.gz", dir_basename(source)));
        let file =
            fs::File::create(&archive_path).map_err(|e| pkg_err(format!("create archive: {e}")))?;
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        Self::add_dir_recursive(
            &mut builder,
            source,
            Path::new(dir_basename(source)),
            hidden,
        )?;
        let enc = builder
            .into_inner()
            .map_err(|e| pkg_err(format!("tar finalise: {e}")))?;
        enc.finish()
            .map_err(|e| pkg_err(format!("gzip finalise: {e}")))?;
        Ok(archive_path)
    }

    fn add_dir_recursive<W: Write>(
        builder: &mut tar::Builder<W>,
        dir: &Path,
        prefix: &Path,
        hidden: bool,
    ) -> Result<(), BundleError> {
        for entry in fs::read_dir(dir).map_err(|e| pkg_err(format!("read dir: {e}")))? {
            let entry = entry.map_err(|e| pkg_err(format!("dir entry: {e}")))?;
            let name = entry.file_name();
            if !hidden && name.to_string_lossy().starts_with('.') {
                continue;
            }
            let full = entry.path();
            let rel = prefix.join(&name);
            let ft = entry
                .file_type()
                .map_err(|e| pkg_err(format!("ftype: {e}")))?;
            if ft.is_dir() {
                Self::add_dir_recursive(builder, &full, &rel, hidden)?;
            } else if ft.is_file() {
                builder
                    .append_path_with_name(&full, &rel)
                    .map_err(|e| pkg_err(format!("tar add {}: {e}", full.display())))?;
            }
        }
        Ok(())
    }

    fn create_zip(&self, source: &Path, dest: &Path, hidden: bool) -> Result<PathBuf, BundleError> {
        let archive_path = dest.join(format!("{}.zip", dir_basename(source)));
        let file =
            fs::File::create(&archive_path).map_err(|e| pkg_err(format!("create zip: {e}")))?;
        let mut zw = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        Self::add_dir_to_zip(&mut zw, source, dir_basename(source), hidden, opts)?;
        zw.finish()
            .map_err(|e| pkg_err(format!("zip finalise: {e}")))?;
        Ok(archive_path)
    }

    fn add_dir_to_zip<W: Write + io::Seek>(
        zw: &mut zip::ZipWriter<W>,
        dir: &Path,
        prefix: &str,
        hidden: bool,
        opts: zip::write::SimpleFileOptions,
    ) -> Result<(), BundleError> {
        for entry in fs::read_dir(dir).map_err(|e| pkg_err(format!("read dir: {e}")))? {
            let entry = entry.map_err(|e| pkg_err(format!("dir entry: {e}")))?;
            let name_os = entry.file_name();
            let name = name_os.to_string_lossy();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let full = entry.path();
            let rel = format!("{prefix}/{name}");
            let ft = entry
                .file_type()
                .map_err(|e| pkg_err(format!("ftype: {e}")))?;
            if ft.is_dir() {
                Self::add_dir_to_zip(zw, &full, &rel, hidden, opts)?;
            } else if ft.is_file() {
                zw.start_file(&rel, opts)
                    .map_err(|e| pkg_err(format!("zip start: {e}")))?;
                let data = fs::read(&full).map_err(|e| pkg_err(format!("read: {e}")))?;
                zw.write_all(&data)
                    .map_err(|e| pkg_err(format!("zip write: {e}")))?;
            }
        }
        Ok(())
    }

    fn output_dir(&self, source: &Path, options: &PackageOptions) -> PathBuf {
        options
            .output_dir
            .clone()
            .or_else(|| self.work_dir.clone())
            .unwrap_or_else(|| {
                source
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| ".".into())
            })
    }
}

/// Compute the SHA-256 hex digest of a file.
pub fn sha256_file(path: &Path) -> Result<String, BundleError> {
    let data = fs::read(path).map_err(|e| pkg_err(format!("read for checksum: {e}")))?;
    Ok(sha256_bytes(&data))
}

/// Compute the SHA-256 hex digest of arbitrary bytes.
pub fn sha256_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

impl Packager for FileSystemPackager {
    fn package(
        &self,
        source_dir: &Path,
        format: PackageFormat,
        options: &PackageOptions,
    ) -> Result<PackageResult, BundleError> {
        if !source_dir.is_dir() {
            return Err(pkg_err(format!(
                "not a directory: {}",
                source_dir.display()
            )));
        }
        let dest = self.output_dir(source_dir, options);
        fs::create_dir_all(&dest).map_err(|e| pkg_err(format!("mkdir: {e}")))?;

        let archive_path = match format {
            PackageFormat::TarGz | PackageFormat::Uvx => {
                self.create_tarball(source_dir, &dest, options.include_hidden)?
            }
            PackageFormat::Zip => self.create_zip(source_dir, &dest, options.include_hidden)?,
            PackageFormat::Directory => {
                let target = dest.join(dir_basename(source_dir));
                if target != source_dir {
                    copy_dir_recursive(source_dir, &target)?;
                }
                target
            }
        };

        let (size_bytes, checksum) = if archive_path.is_file() {
            let sz = fs::metadata(&archive_path)
                .map_err(|e| pkg_err(format!("stat: {e}")))?
                .len();
            (sz, sha256_file(&archive_path)?)
        } else {
            (dir_size(&archive_path), String::new())
        };
        Ok(PackageResult {
            format,
            path: archive_path,
            size_bytes,
            checksum,
        })
    }

    fn list_contents(&self, package_path: &Path) -> Result<Vec<String>, BundleError> {
        if package_path.is_dir() {
            return list_dir_contents(package_path, package_path);
        }
        let ext = package_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if ext == "gz" || ext == "tgz" {
            let file = fs::File::open(package_path).map_err(|e| pkg_err(format!("open: {e}")))?;
            let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(file));
            let mut names = Vec::new();
            for entry in ar
                .entries()
                .map_err(|e| pkg_err(format!("tar entries: {e}")))?
            {
                let entry = entry.map_err(|e| pkg_err(format!("tar entry: {e}")))?;
                if let Ok(p) = entry.path() {
                    names.push(p.to_string_lossy().into_owned());
                }
            }
            Ok(names)
        } else {
            Err(pkg_err(format!("unsupported archive format: {ext}")))
        }
    }
}

/// Convert a [`PackageResult`] into the public [`PackagedBundle`] model.
pub fn into_packaged_bundle(
    result: PackageResult,
    metadata: HashMap<String, String>,
) -> PackagedBundle {
    PackagedBundle {
        format: result.format,
        path: result.path,
        size_bytes: result.size_bytes,
        checksum: result.checksum,
        metadata,
    }
}

/// Recursively copy a directory tree.
pub(super) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), BundleError> {
    // Same-path guard.
    if let (Ok(src_canon), Ok(dst_canon)) = (src.canonicalize(), dst.canonicalize())
        && src_canon == dst_canon
    {
        return Err(pkg_err(format!(
            "source and destination are the same path: {}",
            src_canon.display()
        )));
    }

    fs::create_dir_all(dst).map_err(|e| pkg_err(format!("mkdir {}: {e}", dst.display())))?;
    for entry in fs::read_dir(src).map_err(|e| pkg_err(format!("read {}: {e}", src.display())))? {
        let entry = entry.map_err(|e| pkg_err(format!("entry: {e}")))?;
        let ft = entry
            .file_type()
            .map_err(|e| pkg_err(format!("ftype: {e}")))?;
        let file_name = entry.file_name();
        let target = dst.join(&file_name);
        if ft.is_dir() {
            if matches!(
                file_name.to_str(),
                Some("__pycache__" | ".pytest_cache" | "node_modules")
            ) {
                continue;
            }
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            if file_name
                .to_str()
                .map(|s| s.ends_with(".pyc") || s.ends_with(".pyo"))
                .unwrap_or(false)
            {
                continue;
            }
            fs::copy(entry.path(), &target).map_err(|e| pkg_err(format!("copy: {e}")))?;
        }
    }
    Ok(())
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    total += dir_size(&entry.path());
                } else if ft.is_file() {
                    total += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }
    }
    total
}

fn list_dir_contents(root: &Path, base: &Path) -> Result<Vec<String>, BundleError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| pkg_err(format!("readdir: {e}")))? {
        let entry = entry.map_err(|e| pkg_err(format!("entry: {e}")))?;
        let full = entry.path();
        let rel = full
            .strip_prefix(base)
            .unwrap_or(&full)
            .to_string_lossy()
            .into_owned();
        if full.is_dir() {
            out.extend(list_dir_contents(&full, base)?);
        } else {
            out.push(rel);
        }
    }
    Ok(out)
}
