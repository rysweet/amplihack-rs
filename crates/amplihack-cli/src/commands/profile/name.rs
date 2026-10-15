//! Pure validation of profile names. A name becomes a single directory
//! component, so it is checked against a strict allowlist.

use super::ProfileError;

const MAX_LEN: usize = 64;
const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileName(String);

impl ProfileName {
    pub fn parse(raw: &str) -> Result<Self, ProfileError> {
        let bad = || ProfileError::InvalidName(raw.to_string());
        let mut chars = raw.chars();
        let first = chars.next().ok_or_else(bad)?;
        if raw.len() > MAX_LEN
            || !first.is_ascii_alphanumeric()
            || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            || raw.contains("..")
            || raw.ends_with('.')
            || is_windows_reserved(raw)
        {
            return Err(bad());
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

fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return true;
    }
    ["COM", "LPT"].iter().any(|p| {
        stem.strip_prefix(p)
            .is_some_and(|d| d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9'))
    })
}
