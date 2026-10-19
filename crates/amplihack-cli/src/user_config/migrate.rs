//! One-time migration of the legacy `~/.amplihack/config` JSON
//! (`node_options_consent`, `node_options_limit_mb`) into the user file.
//!
//! Order matters: the consent is written to `config.toml` first, and only then
//! are the two keys removed from the JSON file, so the consent always lives in
//! at least one file. Any failure leaves the legacy keys in place, and they are
//! read as the `legacy` layer until a later load completes the migration.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as Json;

use super::Diagnostic;
use super::edit::{parse_document, set_value, tighten_owned_dir, write_atomic};
use super::files::{legacy_file, migrated_file, user_file};
use super::safe::read_bounded;
use super::schema::{Value, key};

const CONSENT: &str = "node_options_consent";
const LIMIT: &str = "node_options_limit_mb";

/// The legacy file while it still holds a key amplihack reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegacyLayer {
    pub path: PathBuf,
    /// `node_options_consent`, when the file has it.
    pub consent: Option<bool>,
}

/// What the legacy check found.
#[derive(Debug, Default)]
pub(crate) struct LegacyOutcome {
    pub diagnostics: Vec<Diagnostic>,
    /// `Some` while legacy keys remain: the migration is disabled, waiting, or
    /// failed.
    pub layer: Option<LegacyLayer>,
}

/// Check `$HOME/.amplihack/config` and, when `write`, migrate it.
pub(crate) fn run(home: &Path, write: bool) -> LegacyOutcome {
    let path = legacy_file(home);
    let mut outcome = LegacyOutcome::default();
    let problem = |reason: String| Diagnostic::file(&path, reason);
    let text = match read_bounded(&path) {
        Ok(Some(text)) => text,
        Ok(None) => return outcome,
        Err(reason) => {
            outcome.diagnostics.push(problem(reason));
            return outcome;
        }
    };
    let object = match serde_json::from_str::<Json>(&text) {
        Ok(Json::Object(object)) => object,
        Ok(_) => {
            outcome
                .diagnostics
                .push(problem("is not a JSON object; left untouched".to_string()));
            return outcome;
        }
        Err(_) => {
            outcome
                .diagnostics
                .push(problem("is not valid JSON; left untouched".to_string()));
            return outcome;
        }
    };
    let consent = match object.get(CONSENT) {
        None => None,
        Some(Json::Bool(consent)) => Some(*consent),
        Some(_) => {
            outcome.diagnostics.push(Diagnostic::key(
                &path,
                CONSENT,
                "must be true or false; file left untouched".to_string(),
            ));
            return outcome;
        }
    };
    match object.get(LIMIT) {
        None => {}
        Some(limit) if limit.as_u64().is_some() => {}
        Some(_) => {
            outcome.diagnostics.push(Diagnostic::key(
                &path,
                LIMIT,
                "must be a non-negative integer; file left untouched".to_string(),
            ));
            return outcome;
        }
    }
    if consent.is_none() && !object.contains_key(LIMIT) {
        // Only other settings (git_identity, ...): not a config layer.
        return outcome;
    }
    let layer = LegacyLayer {
        path: path.clone(),
        consent,
    };
    if !write {
        outcome.layer = Some(layer);
        return outcome;
    }
    match migrate(home, &path, consent, object) {
        Ok(()) => {
            tracing::info!(
                from = %path.display(),
                to = %user_file(home).display(),
                "migrated the legacy NODE_OPTIONS consent to the user config"
            );
        }
        Err(reason) => {
            outcome.diagnostics.push(problem(format!(
                "migration to {} failed ({reason}); still reading it",
                super::safe::display_path(&user_file(home))
            )));
            outcome.layer = Some(layer);
        }
    }
    outcome
}

/// Steps 2 and 3 of the migration.
fn migrate(
    home: &Path,
    legacy: &Path,
    consent: Option<bool>,
    mut object: serde_json::Map<String, Json>,
) -> Result<(), String> {
    let user = user_file(home);
    let heap_consent = key("node.heap_consent").expect("node.heap_consent is in the schema");
    if let Some(consent) = consent {
        let text = read_bounded(&user)?.unwrap_or_default();
        let doc = parse_document(&text)?;
        let (table, name) = heap_consent.split();
        let already = doc
            .get(table)
            .and_then(toml_edit::Item::as_table_like)
            .is_some_and(|t| t.contains_key(name));
        if !already {
            set_value(&user, heap_consent, &Value::Bool(consent)).map_err(|e| e.to_string())?;
        }
        tighten_owned_dir(&home.join(".amplihack"));
    }
    if object.keys().all(|k| k == CONSENT || k == LIMIT) {
        let migrated = migrated_file(home);
        match fs::rename(legacy, &migrated) {
            Ok(()) => Ok(()),
            // Another amplihack process migrated it between our read and this
            // rename: the work is done, not failed.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && migrated.is_file() => Ok(()),
            Err(e) => Err(format!("cannot rename it: {e}")),
        }
    } else {
        object.remove(CONSENT);
        object.remove(LIMIT);
        let body = serde_json::to_string_pretty(&Json::Object(object))
            .map_err(|e| format!("cannot serialise it: {e}"))?
            + "\n";
        write_atomic(legacy, body.as_bytes()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(body: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".amplihack")).unwrap();
        fs::write(legacy_file(home.path()), body).unwrap();
        home
    }

    #[test]
    fn consent_moves_and_the_file_is_renamed() {
        let home = home_with(r#"{"node_options_consent": false, "node_options_limit_mb": 8192}"#);
        let outcome = run(home.path(), true);
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
        assert!(outcome.layer.is_none());
        assert!(migrated_file(home.path()).is_file());
        let body = fs::read_to_string(user_file(home.path())).unwrap();
        assert!(body.contains("heap_consent = false"), "{body}");
        assert!(!body.contains("max_old_space"), "{body}");
    }

    #[test]
    fn other_keys_are_kept_in_place() {
        let home = home_with(r#"{"node_options_consent": true, "git_identity": {"name": "A"}}"#);
        run(home.path(), true);
        let v: Json =
            serde_json::from_str(&fs::read_to_string(legacy_file(home.path())).unwrap()).unwrap();
        assert_eq!(v["git_identity"]["name"], "A");
        assert!(v.get(CONSENT).is_none());
    }

    #[test]
    fn an_existing_user_value_wins() {
        let home = home_with(r#"{"node_options_consent": true}"#);
        fs::write(user_file(home.path()), "[node]\nheap_consent = false\n").unwrap();
        run(home.path(), true);
        assert_eq!(
            fs::read_to_string(user_file(home.path())).unwrap(),
            "[node]\nheap_consent = false\n"
        );
    }

    #[test]
    fn malformed_files_are_reported_and_untouched() {
        for body in ["{nope", "[1]", r#"{"node_options_consent": "yes"}"#] {
            let home = home_with(body);
            let outcome = run(home.path(), true);
            assert_eq!(outcome.diagnostics.len(), 1, "{body}");
            assert!(outcome.layer.is_none());
            assert_eq!(fs::read_to_string(legacy_file(home.path())).unwrap(), body);
        }
    }

    #[test]
    fn without_write_the_layer_is_reported() {
        let home = home_with(r#"{"node_options_consent": true}"#);
        let outcome = run(home.path(), false);
        assert_eq!(outcome.layer.unwrap().consent, Some(true));
        assert!(legacy_file(home.path()).is_file());
    }

    #[test]
    fn a_file_without_node_keys_is_not_a_layer() {
        let home = home_with(r#"{"git_identity": {}}"#);
        let outcome = run(home.path(), true);
        assert!(outcome.layer.is_none() && outcome.diagnostics.is_empty());
        assert!(!user_file(home.path()).exists());
    }
}
