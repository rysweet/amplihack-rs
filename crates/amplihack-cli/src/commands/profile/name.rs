//! Profile names. A name becomes a single directory component, so it is
//! checked against a strict allowlist: `amplihack_utils::rotator_status::
//! valid_live_name`, the rule the hooks apply to names read from disk.

use super::ProfileError;
use amplihack_utils::rotator_status::valid_live_name;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileName(String);

impl ProfileName {
    pub fn parse(raw: &str) -> Result<Self, ProfileError> {
        if !valid_live_name(raw) {
            return Err(ProfileError::InvalidName(raw.to_string()));
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn eq_fold(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl std::fmt::Display for ProfileName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
