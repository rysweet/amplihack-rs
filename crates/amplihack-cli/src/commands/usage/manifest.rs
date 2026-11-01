//! Manifest identity and ancestry validation. Errors never echo input values.
use super::models::*;
use anyhow::{Result, bail};
use std::collections::BTreeSet;

pub fn validate(m: &Manifest) -> Result<()> {
    if m.version != 1 {
        bail!("unsupported usage manifest version");
    }
    if m.sessions.is_empty() {
        bail!("usage manifest needs at least one session");
    }
    if m.workstream.as_deref().is_some_and(|s| !safe_label(s))
        || m.branch.as_deref().is_some_and(|s| !safe_path_label(s))
        || m.repository.as_deref().is_some_and(|s| {
            let parts: Vec<_> = s.split('/').collect();
            parts.len() != 2 || parts.iter().any(|s| !safe_label(s))
        })
    {
        bail!("invalid manifest identity label");
    }
    let mut ids = BTreeSet::new();
    for s in &m.sessions {
        if !safe_label(&s.id)
            || !ids.insert(&s.id)
            || s.run_id.as_deref().is_some_and(|s| !safe_label(s))
            || s.parent.as_deref().is_some_and(|s| !safe_label(s))
        {
            bail!("invalid or duplicate session identity");
        }
        let mut chain = BTreeSet::new();
        let mut current = Some(s.id.as_str());
        while let Some(id) = current {
            if !chain.insert(id) {
                bail!("cyclic session ancestry");
            }
            current = m
                .sessions
                .iter()
                .find(|s| s.id == id)
                .and_then(|s| s.parent.as_deref());
        }
    }
    if m.prs.contains(&0) || m.prs.iter().collect::<BTreeSet<_>>().len() != m.prs.len() {
        bail!("PR identities must be unique positive numbers");
    }
    Ok(())
}
fn safe_path_label(s: &str) -> bool {
    s.split('/').all(safe_label) && s.len() <= 240
}
