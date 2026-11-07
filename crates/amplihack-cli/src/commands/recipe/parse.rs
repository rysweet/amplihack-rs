use super::*;

pub(crate) fn parse_recipe_from_input(
    input: &str,
    working_dir: impl AsRef<Path>,
) -> Result<RecipeDoc> {
    parse_recipe_from_path(resolve_recipe_path(input, working_dir)?)
}

pub(crate) fn parse_recipe_from_path(path: impl AsRef<Path>) -> Result<RecipeDoc> {
    let validated = validate_path(path.as_ref(), false)?;
    let text = fs::read_to_string(&validated)
        .with_context(|| format!("Recipe file not found: {}", validated.display()))?;

    if text.len() > MAX_YAML_SIZE_BYTES {
        anyhow::bail!(
            "Recipe file too large ({} bytes). Maximum allowed: {} bytes",
            text.len(),
            MAX_YAML_SIZE_BYTES
        );
    }

    parse_recipe_text(&text)
}

pub(crate) fn parse_recipe_text(text: &str) -> Result<RecipeDoc> {
    if text.len() > MAX_YAML_SIZE_BYTES {
        anyhow::bail!(
            "YAML content too large ({} bytes). Maximum allowed: {} bytes",
            text.len(),
            MAX_YAML_SIZE_BYTES
        );
    }

    // YAML rejects JSON surrogate pairs. Decode the original JSON into the same
    // value type, which also rejects duplicate keys, before shared validation.
    let raw_value: Value = serde_yaml::from_str(text)
        .or_else(|yaml_error| serde_json::from_str::<Value>(text).map_err(|_| yaml_error))?;
    let raw_mapping = raw_value
        .as_mapping()
        .context("Recipe YAML must be a mapping at the top level")?;

    require_field(raw_mapping, "name", "Recipe must have a 'name' field")?;

    if let Some(name_val) = raw_mapping.get(Value::String("name".to_string()))
        && name_val.is_null()
    {
        anyhow::bail!("Recipe must have a 'name' field");
    }

    require_field(
        raw_mapping,
        "steps",
        "Recipe must have a 'steps' field with at least one step",
    )?;

    if let Some(steps_val) = raw_mapping.get(Value::String("steps".to_string()))
        && let Some(steps_seq) = steps_val.as_sequence()
    {
        for step_val in steps_seq {
            if let Some(step_map) = step_val.as_mapping() {
                let id_key = Value::String("id".to_string());
                let should_bail = match step_map.get(&id_key) {
                    None => true,
                    Some(v) if v.is_null() => true,
                    _ => false,
                };
                if should_bail {
                    anyhow::bail!("Every step must have a non-empty 'id' field");
                }
            }
        }
    }

    let recipe: RecipeDoc = serde_yaml::from_value(raw_value)?;
    if recipe.steps.is_empty() {
        anyhow::bail!("Recipe must have a 'steps' field with at least one step");
    }

    let mut seen_ids = BTreeSet::new();
    for step in &recipe.steps {
        if step.id.trim().is_empty() {
            anyhow::bail!("Every step must have a non-empty 'id' field");
        }
        if !seen_ids.insert(step.id.clone()) {
            anyhow::bail!("Duplicate step id: '{}'", step.id);
        }
        if let Some(step_type) = &step.step_type {
            match step_type.to_ascii_lowercase().as_str() {
                "bash" | "agent" | "recipe" => {}
                other => anyhow::bail!("'{other}' is not a valid StepType"),
            }
        }
    }

    Ok(recipe)
}

pub(crate) fn require_field(mapping: &serde_yaml::Mapping, key: &str, message: &str) -> Result<()> {
    if mapping.contains_key(Value::String(key.to_string())) {
        return Ok(());
    }
    anyhow::bail!(message.to_string())
}

pub(crate) fn validate_path(path: impl AsRef<Path>, must_exist: bool) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("failed to read current directory")?;
    let resolved = super::resolve::resolve_path_from(&cwd, path)?;

    if must_exist && !resolved.exists() {
        anyhow::bail!("Path does not exist: {}", resolved.display());
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The description forces JSON decoding even when the tested field is ASCII.
    fn fallback_recipe(fields: &str) -> String {
        format!(r#"{{"description":"\uD83D\uDE00",{fields}}}"#)
    }

    #[test]
    fn surrogate_pairs_and_utf8_decode_to_identical_prompt_text() {
        let expected = "Before: 😀, 𝄞 and 𐐷!\nAfter: café, \"quoted\".\tDone.";
        let utf8 = r#"{"name":"unicode","steps":[{"id":"review","type":"agent","prompt":"Before: 😀, 𝄞 and 𐐷!\nAfter: café, \"quoted\".\tDone."}]}"#;
        let escaped = r#"{"name":"unicode","steps":[{"id":"review","type":"agent","prompt":"Before: \uD83D\uDE00, \ud834\udd1e and \uD801\uDC37!\nAfter: café, \"quoted\".\tDone."}]}"#;
        let raw = parse_recipe_text(utf8).expect("raw UTF-8 recipe must parse");
        assert_eq!(raw.steps[0].prompt.as_deref(), Some(expected));
        let decoded = parse_recipe_text(escaped).expect("valid surrogate pairs must parse");
        assert_eq!(decoded.steps[0].prompt.as_deref(), Some(expected));
        assert_eq!(decoded.steps[0].prompt, raw.steps[0].prompt);
    }

    #[test]
    fn json_escapes_preserve_literal_backslash_sequences() {
        let expected = r"Keep \uD83D\uDE00, \uD800, \n and C:\recipes\unicode literally.";
        let prompt = serde_json::to_string(expected).unwrap();
        let text = fallback_recipe(&format!(
            r#""name":"literal","steps":[{{"id":"one","prompt":{prompt}}}]"#
        ));
        let recipe = parse_recipe_text(&text).expect("literal backslashes are valid text");
        assert_eq!(recipe.steps[0].prompt.as_deref(), Some(expected));
    }

    #[test]
    fn surrogate_pairs_decode_recursively_in_keys_and_values() {
        let escaped = r#"{"name":"\uD83D\uDE00","context":{"\uD834\uDD1E":["\uD801\uDC37"]},"extra":{"text":"\uD83D\uDE00"},"steps":[{"id":"\uD834\uDD1E","prompt":"\uD801\uDC37","custom":{"\uD83D\uDE00":"\uD834\uDD1E"}}]}"#;
        let utf8 = r#"{"name":"😀","context":{"𝄞":["𐐷"]},"extra":{"text":"😀"},"steps":[{"id":"𝄞","prompt":"𐐷","custom":{"😀":"𝄞"}}]}"#;
        let expected = parse_recipe_text(utf8).unwrap();
        let decoded = parse_recipe_text(escaped).expect("nested Unicode must decode");
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn json_fallback_rejects_duplicate_keys_at_every_depth() {
        let control = fallback_recipe(r#""name":"ok","steps":[{"id":"one"}]"#);
        parse_recipe_text(&control).expect("the fallback control must be accepted");
        for fields in [
            r#""name":"a","name":"b","steps":[{"id":"one"}]"#,
            r#""name":"a","na\u006de":"b","steps":[{"id":"one"}]"#,
            r#""name":"a","steps":[{"id":"one","prompt":"a","prompt":"b"}]"#,
            r#""name":"a","steps":[{"id":"one","prompt":"a","pro\u006dpt":"b"}]"#,
            r#""name":"a","context":{"key":1,"key":2},"steps":[{"id":"one"}]"#,
            r#""name":"a","context":{"key":1,"\u006bey":2},"steps":[{"id":"one"}]"#,
            r#""name":"a","custom":[{"nested":{"key":1,"key":2}}],"steps":[{"id":"one"}]"#,
            r#""name":"a","steps":[{"id":"one","custom":{"nested":[{"key":1,"key":2}]}}]"#,
            r#""name":"a","context":{"😀":1,"\uD83D\uDE00":2},"steps":[{"id":"one"}]"#,
        ] {
            assert!(
                parse_recipe_text(&fallback_recipe(fields)).is_err(),
                "duplicate keys must be rejected: {fields}"
            );
        }
    }

    #[test]
    fn typed_json_value_rejects_recursive_duplicate_keys() {
        // This checks the proposed decoder directly, independently of YAML rejection.
        let control = r#"{"key":[{"nested":{"a":1,"b":2}}]}"#;
        serde_json::from_str::<Value>(control).unwrap();
        for input in [
            r#"{"key":1,"key":2}"#,
            r#"{"key":1,"\u006bey":2}"#,
            r#"{"key":[{"nested":{"a":1,"a":2}}]}"#,
            r#"{"😀":1,"\uD83D\uDE00":2}"#,
        ] {
            assert!(serde_json::from_str::<Value>(input).is_err(), "{input}");
        }
    }

    #[test]
    fn lone_reversed_interrupted_and_malformed_surrogates_are_rejected() {
        for prompt in [
            r"\uD800",
            r"\uDC00",
            r"\uDE00\uD83D",
            r"\uD83Dtext\uDE00",
            r"\uD83D\u0041",
            r"\uD83D\uD83D",
            r"\uD83D\uDE0",
            r"\uD83D\uDE0G",
        ] {
            let text =
                format!(r#"{{"name":"invalid","steps":[{{"id":"one","prompt":"{prompt}"}}]}}"#);
            assert!(serde_json::from_str::<Value>(&text).is_err(), "{prompt}");
            assert!(parse_recipe_text(&text).is_err(), "{prompt}");
        }
    }

    #[test]
    fn fallback_preserves_schema_and_step_validation() {
        for (fields, diagnostic) in [
            (
                r#""steps":[{"id":"one"}]"#,
                "Recipe must have a 'name' field",
            ),
            (
                r#""name":null,"steps":[{"id":"one"}]"#,
                "Recipe must have a 'name' field",
            ),
            (r#""name":{},"steps":[{"id":"one"}]"#, "invalid type"),
            (r#""name":"a""#, "Recipe must have a 'steps' field"),
            (r#""name":"a","steps":[]"#, "at least one step"),
            (r#""name":"a","steps":{}"#, "invalid type"),
            (r#""name":"a","steps":[{}]"#, "non-empty 'id' field"),
            (
                r#""name":"a","steps":[{"id":null}]"#,
                "non-empty 'id' field",
            ),
            (
                r#""name":"a","steps":[{"id":"  "}]"#,
                "non-empty 'id' field",
            ),
            (
                r#""name":"a","steps":[{"id":"one","prompt":[]}]"#,
                "invalid type",
            ),
            (
                r#""name":"a","steps":[{"id":"one","parse_json":"yes"}]"#,
                "invalid type",
            ),
            (
                r#""name":"a","steps":[{"id":"one"},{"id":"one"}]"#,
                "Duplicate step id: 'one'",
            ),
            (
                r#""name":"a","steps":[{"id":"one","type":"python"}]"#,
                "'python' is not a valid StepType",
            ),
        ] {
            let error = parse_recipe_text(&fallback_recipe(fields)).unwrap_err();
            assert!(error.to_string().contains(diagnostic), "{fields}: {error}");
        }
    }

    #[test]
    fn fallback_preserves_inference_supported_types_and_additional_fields() {
        let text = fallback_recipe(
            r#""name":"a","unknown":{"text":"keep"},"context":{"key":[1,true]},"steps":[{"id":"bash","command":"echo hi","retired":"keep"},{"id":"agent","agent":"fake","prompt":"text"},{"id":"recipe","recipe":"child"},{"id":"explicit","type":"BASH"},{"id":"explicit-agent","type":"agent"},{"id":"explicit-recipe","type":"recipe"}]"#,
        );
        let recipe = parse_recipe_text(&text).unwrap();
        for (step, expected) in recipe
            .steps
            .iter()
            .zip(["bash", "agent", "recipe", "bash", "agent", "recipe"])
        {
            assert_eq!(super::super::show_validate::infer_step_type(step), expected);
        }
        assert_eq!(recipe.extra["unknown"]["text"].as_str(), Some("keep"));
        assert_eq!(recipe.steps[0].extra["retired"].as_str(), Some("keep"));
        assert_eq!(recipe.context["key"][0].as_i64(), Some(1));
        assert_eq!(recipe.context["key"][1].as_bool(), Some(true));
    }

    #[test]
    fn yaml_features_and_literal_text_remain_unchanged() {
        let yaml = r#"
name: yaml
context:
  original: &value [one, 2, true]
  alias: *value
steps:
  - id: one
    command: echo hi
    prompt: |
      Keep \uD83D\uDE00 and café.
      Second line.
"#;
        let recipe = parse_recipe_text(yaml).unwrap();
        assert_eq!(recipe.context["original"], recipe.context["alias"]);
        assert_eq!(
            recipe.steps[0].prompt.as_deref(),
            Some("Keep \\uD83D\\uDE00 and café.\nSecond line.\n")
        );
        for yaml in [
            "name: a\nname: b\nsteps: [{id: one}]",
            "name: a\ncontext: {key: 1, key: 2}\nsteps: [{id: one}]",
        ] {
            assert!(parse_recipe_text(yaml).is_err());
        }
    }

    #[test]
    fn unsuccessful_decoding_keeps_yaml_diagnostics_and_rejects_trailing_content() {
        for text in [
            "name: [unfinished",
            r#"{"name":"a","steps":[{"id":"one","prompt":"\uD800"}]}"#,
            r#"{"name":"a","steps":[{"id":"one","prompt":"\uD83D\uDE00"}]} {}"#,
            r#"{"name":"a","steps":[{"id":"one","prompt":"\uD83D\uDE00"}]} garbage"#,
        ] {
            let original = serde_yaml::from_str::<Value>(text).unwrap_err();
            let actual = parse_recipe_text(text).unwrap_err();
            assert_eq!(actual.to_string(), original.to_string());
        }
    }

    #[test]
    fn input_byte_limit_applies_before_decoding() {
        let mut text = fallback_recipe(r#""name":"a","steps":[{"id":"one"}]"#);
        text.extend(std::iter::repeat_n(' ', MAX_YAML_SIZE_BYTES - text.len()));
        parse_recipe_text(&text).expect("the exact original byte limit is permitted");
        text.push(' ');
        let error = parse_recipe_text(&text).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("YAML content too large (1000001 bytes)")
        );

        let text = format!(
            r#"{{"name":"a","steps":[{{"id":"one","prompt":"{}"}}]}}"#,
            r"\uD83D\uDE00".repeat(MAX_YAML_SIZE_BYTES / 12)
        );
        assert!(text.len() > MAX_YAML_SIZE_BYTES);
        assert!(
            parse_recipe_text(&text)
                .unwrap_err()
                .to_string()
                .contains("YAML content too large")
        );
    }

    #[test]
    fn file_parser_preserves_unicode_and_original_file_byte_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("unicode.yaml");
        let mut text =
            fallback_recipe(r#""name":"a","steps":[{"id":"one","prompt":"\uD834\uDD1E"}]"#);
        fs::write(&path, &text).unwrap();
        let recipe = parse_recipe_from_path(&path).unwrap();
        assert_eq!(recipe.steps[0].prompt.as_deref(), Some("𝄞"));
        text.extend(std::iter::repeat_n(
            ' ',
            MAX_YAML_SIZE_BYTES + 1 - text.len(),
        ));
        fs::write(&path, text).unwrap();
        let error = parse_recipe_from_path(&path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Recipe file too large (1000001 bytes)")
        );
    }
}
