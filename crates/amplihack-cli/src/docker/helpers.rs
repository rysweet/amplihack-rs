//! Environment variable forwarding, API key validation, and value sanitization.

use std::collections::BTreeMap;

pub(super) fn validate_api_key(key: &str, value: &str) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;

    static SK_RE: OnceLock<Regex> = OnceLock::new();
    static GH_RE: OnceLock<Regex> = OnceLock::new();

    let result = match key {
        "ANTHROPIC_API_KEY" | "OPENAI_API_KEY" => {
            let re =
                SK_RE.get_or_init(|| Regex::new(r"^sk-[a-zA-Z0-9\-_]+$").expect("static SK regex"));
            re.is_match(value)
        }
        "GITHUB_TOKEN" | "GH_TOKEN" => {
            let re = GH_RE.get_or_init(|| {
                Regex::new(r"^(ghp_|ghs_|gho_|ghu_|github_pat_).+$|^[0-9a-fA-F]{40}$")
                    .expect("static GH token regex")
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
    let mut gateway_requested = false;
    let mut provider_credentials = Vec::new();
    for (key, value) in env_vars {
        let key = key.into().to_string_lossy().into_owned();
        let value = value.into().to_string_lossy().into_owned();

        if matches!(
            key.as_str(),
            amplihack_utils::litellm_proxy::ENDPOINT_ENV
                | amplihack_utils::litellm_proxy::API_KEY_ENV
                | amplihack_utils::litellm_proxy::MODEL_ENV
        ) {
            gateway_requested = true;
            provider_credentials.clear();
        }

        if matches!(
            key.as_str(),
            "ANTHROPIC_API_KEY" | "OPENAI_API_KEY" | "GITHUB_TOKEN" | "GH_TOKEN"
        ) {
            if !gateway_requested {
                provider_credentials.push((key, value));
            }
            continue;
        }

        let should_forward = (key == "TERM"
            || (key.starts_with("AMPLIHACK_")
                && key != "AMPLIHACK_USE_DOCKER"
                && (!is_secret_env_key(&key)
                    || key == amplihack_utils::litellm_proxy::API_KEY_ENV)))
            && validate_api_key(&key, &value);
        if should_forward {
            forwarded.insert(key, sanitize_env_value(&value));
        }
    }
    if !gateway_requested {
        for (key, value) in provider_credentials {
            if validate_api_key(&key, &value) {
                forwarded.insert(key, sanitize_env_value(&value));
            }
        }
    }
    forwarded.insert("AMPLIHACK_IN_DOCKER".to_string(), "1".to_string());
    forwarded
}

pub(super) fn is_secret_env_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    ["API_KEY", "TOKEN", "AUTHORIZATION", "PASSWORD", "SECRET"]
        .iter()
        .any(|marker| key.contains(marker))
}

fn sanitize_env_value(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        .collect()
}
