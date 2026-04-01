//! Environment variable forwarding, API key validation, and value sanitization.

use std::collections::BTreeMap;

pub(super) fn validate_api_key(key: &str, value: &str) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;

    static SK_RE: OnceLock<Regex> = OnceLock::new();
    static GH_RE: OnceLock<Regex> = OnceLock::new();

    let result = match key {
        "ANTHROPIC_API_KEY" | "OPENAI_API_KEY" => {
            let re = SK_RE.get_or_init(|| Regex::new(r"^sk-[a-zA-Z0-9\-_]+$").unwrap());
            re.is_match(value)
        }
        "GITHUB_TOKEN" | "GH_TOKEN" => {
            let re = GH_RE.get_or_init(|| {
                Regex::new(r"^(ghp_|ghs_|gho_|ghu_|github_pat_).+$|^[0-9a-fA-F]{40}$").unwrap()
            });
            re.is_match(value)
        }
        _ => true, // No format requirement for other keys
    };
    if !result {
        eprintln!("Warning: {key} has an unexpected format, skipping.");
    }
    result
}

pub(crate) fn forwarded_env_vars<I, K, V>(env_vars: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<std::ffi::OsString>,
    V: Into<std::ffi::OsString>,
{
    let mut forwarded = BTreeMap::new();
    for (key, value) in env_vars {
        let key = key.into();
        let value = value.into();
        let key = key.to_string_lossy();
        let value = value.to_string_lossy();
        let should_forward = (matches!(
            key.as_ref(),
            "ANTHROPIC_API_KEY" | "OPENAI_API_KEY" | "GITHUB_TOKEN" | "GH_TOKEN" | "TERM"
        ) || (key.starts_with("AMPLIHACK_")
            && key != "AMPLIHACK_USE_DOCKER"))
            && validate_api_key(&key, &value);
        if should_forward {
            forwarded.insert(key.into_owned(), sanitize_env_value(&value));
        }
    }
    forwarded.insert("AMPLIHACK_IN_DOCKER".to_string(), "1".to_string());
    forwarded
}

fn sanitize_env_value(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        .collect()
}
