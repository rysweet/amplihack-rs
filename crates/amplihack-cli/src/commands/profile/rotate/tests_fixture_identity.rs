//! Explicit synthetic credential identity for legacy rotation fixtures.
use super::super::identify::Account;
use std::path::Path;

pub(in crate::commands::profile::rotate) fn read(dir: &Path) -> Result<Account, &'static str> {
    if !dir.starts_with(std::env::temp_dir()) {
        return Err("not a task fixture");
    }
    let raw = std::fs::read(dir.join(".credentials.json")).map_err(|_| "fake login missing")?;
    let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| "fake login invalid")?;
    Account::from_block(
        v.get("task1753FakeIdentity")
            .ok_or("fake identity missing")?,
    )
    .ok_or("fake identity invalid")
}

/// Seed fake moving identity from the fixture's initial, synthetic account.
/// Later session rewrites never affect the login's evidence.
pub(in crate::commands::profile::rotate) fn body(path: &Path) -> String {
    assert!(path.starts_with(std::env::temp_dir()));
    let dir = path.parent().unwrap();
    let session = if dir.ends_with(".claude") {
        dir.parent().unwrap()
    } else {
        dir
    };
    let name = if dir.ends_with(".claude") {
        "work"
    } else {
        dir.file_name().unwrap().to_str().unwrap()
    };
    let account = Account::of_dir(session).map(|a| a.block()).unwrap_or_else(||
        serde_json::json!({"accountUuid": format!("u-{name}"), "emailAddress": format!("{name}@example.com")}));
    serde_json::json!({"claudeAiOauth": {"accessToken": "SENTINEL"}, "task1753FakeIdentity": account}).to_string()
}
