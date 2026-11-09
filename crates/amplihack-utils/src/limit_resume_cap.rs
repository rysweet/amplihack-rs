//! The resume cap shared by Claude's limit-resume hook and
//! `amplihack profile codex-resume` (issue #2005), so the two cannot drift.

/// At most this many resumes of one session in 24 hours.
pub const MAX_RESUMES_PER_SESSION: u32 = 3;
