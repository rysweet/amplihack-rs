//! The release bundle manifest embedded in a release binary (issue #1972).
//!
//! `release.yml` writes the manifest of the release's `amplifier-bundle/` over
//! the committed empty placeholder before `cargo build`, and attaches the same
//! bytes to the release as `amplifier-bundle-manifest.txt`. Install reads only
//! the embedded copy; nothing here downloads or reads the published asset.
//!
//! The check shows that a source matches the bundle this binary was released
//! with. It does not prove the release itself is authentic.

use super::bundle_manifest::{self, BundleManifest, ManifestDiff, ManifestParseError};
use super::framework_source::BinaryIdentity;
use super::paths::sanitize_text;
use std::path::Path;

/// The manifest `release.yml` copies over the placeholder. Empty in every
/// other build (dev, snapshot, `cargo install --git`).
const EMBEDDED_TEXT: &str = include_str!("../../../release/amplifier-bundle-manifest.txt");

/// The manifest of the bundle released with this binary's version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReleaseManifest {
    /// The binary's version, which is the release the manifest belongs to.
    pub(super) version: String,
    pub(super) files: BundleManifest,
}

/// The embedded manifest is active but does not parse (message M6). Not a
/// [`super::tag_fetch::SourceFetchFailed`], so self-heal exits instead of
/// keeping the installed bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MalformedReleaseManifest {
    pub(super) line: usize,
    pub(super) reason: &'static str,
}

impl std::fmt::Display for MalformedReleaseManifest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the release manifest embedded in this binary is malformed (line {}: {}); this is a \
             build defect, so nothing was staged. Reinstall amplihack from a published release.",
            self.line, self.reason
        )
    }
}

impl std::error::Error for MalformedReleaseManifest {}

impl From<ManifestParseError> for MalformedReleaseManifest {
    fn from(error: ManifestParseError) -> Self {
        Self {
            line: error.line,
            reason: error.reason,
        }
    }
}

/// Why a candidate does not match the release manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ManifestFinding {
    Differs(ManifestDiff),
    /// The candidate could not be hashed; the text is the I/O error.
    Unreadable(String),
}

impl ManifestFinding {
    /// The finding as text. `subject` is already sanitised; the error text and
    /// every listed path are sanitised here.
    pub(super) fn render(&self, subject: &str, version: &str) -> String {
        match self {
            Self::Differs(diff) => bundle_manifest::render_release_mismatch(subject, version, diff),
            Self::Unreadable(error) => format!(
                "{subject} could not be checked against the release v{version} manifest \
                 embedded in this binary: {}",
                sanitize_text(error)
            ),
        }
    }
}

impl ReleaseManifest {
    /// The manifest this binary checks against, or `None` when it carries none.
    pub(super) fn embedded(
        binary: &BinaryIdentity,
    ) -> Result<Option<Self>, MalformedReleaseManifest> {
        Self::from_text(EMBEDDED_TEXT, binary)
    }

    /// D2, D3: active only for non-empty text in a build that may fetch a
    /// release tag. An active manifest that does not parse, or lists nothing,
    /// is an error rather than "no manifest".
    pub(super) fn from_text(
        text: &str,
        binary: &BinaryIdentity,
    ) -> Result<Option<Self>, MalformedReleaseManifest> {
        if text.is_empty() {
            return Ok(None);
        }
        if !binary.may_fetch {
            tracing::debug!(
                "ignoring the embedded release manifest: build {} may not fetch a release tag",
                binary.version
            );
            return Ok(None);
        }
        let files = bundle_manifest::parse(text)?;
        if files.files.is_empty() {
            return Err(MalformedReleaseManifest {
                line: 1,
                reason: "the manifest lists no files",
            });
        }
        Ok(Some(Self {
            version: binary.version.clone(),
            files,
        }))
    }

    /// Compare `<root>/amplifier-bundle` with the manifest. `None` means it
    /// matches. A hashing error is a finding, so a local candidate fails closed.
    pub(super) fn check_candidate(&self, root: &Path) -> Option<ManifestFinding> {
        match bundle_manifest::of_source(&root.join("amplifier-bundle")) {
            Ok(actual) => self.check(&actual).map(ManifestFinding::Differs),
            Err(error) => Some(ManifestFinding::Unreadable(format!("{error:#}"))),
        }
    }

    /// D16: "missing" is listed but absent, "extra" present but not listed.
    pub(super) fn check(&self, actual: &BundleManifest) -> Option<ManifestDiff> {
        let diff = bundle_manifest::compare(&self.files, actual);
        (!diff.is_clean()).then_some(diff)
    }
}

#[cfg(test)]
pub(super) fn embedded_text() -> &'static str {
    EMBEDDED_TEXT
}
