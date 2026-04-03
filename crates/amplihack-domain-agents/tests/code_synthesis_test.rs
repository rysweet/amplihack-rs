use amplihack_domain_agents::{
    CodeAnalysis, CodeSpec, CodeSynthesisConfig, CodeSynthesizer, GeneratedCode,
};

// ── Construction & accessors (PASS) ─────────────────────────────────────────

#[test]
fn new_with_config_stores_config() {
    let cfg = CodeSynthesisConfig {
        language: "python".to_string(),
        style: "functional".to_string(),
        max_complexity: 15,
    };
    let synth = CodeSynthesizer::new(cfg.clone());
    let got = synth.config();
    assert_eq!(got.language, "python");
    assert_eq!(got.style, "functional");
    assert_eq!(got.max_complexity, 15);
}

#[test]
fn with_defaults_uses_default_config() {
    let synth = CodeSynthesizer::with_defaults();
    let cfg = synth.config();
    assert_eq!(cfg.language, "rust");
    assert_eq!(cfg.style, "idiomatic");
    assert_eq!(cfg.max_complexity, 10);
}

#[test]
fn config_accessor_returns_config() {
    let cfg = CodeSynthesisConfig {
        language: "go".to_string(),
        style: "verbose".to_string(),
        max_complexity: 3,
    };
    let synth = CodeSynthesizer::new(cfg);
    let got = synth.config();
    assert_eq!(got.language, "go");
    assert_eq!(got.style, "verbose");
    assert_eq!(got.max_complexity, 3);
}

// ── generate (todo → should_panic) ──────────────────────────────────────────

#[test]
#[should_panic]
fn generate_from_spec() {
    let synth = CodeSynthesizer::with_defaults();
    let spec = CodeSpec {
        description: "A function that adds two numbers".to_string(),
        language: "rust".to_string(),
        constraints: vec!["must be generic".to_string()],
    };
    let _ = synth.generate(&spec);
}

#[test]
#[should_panic]
fn generate_complex_spec() {
    let synth = CodeSynthesizer::with_defaults();
    let spec = CodeSpec {
        description: "A concurrent hash map with lock striping".to_string(),
        language: "rust".to_string(),
        constraints: vec![
            "thread-safe".to_string(),
            "no unsafe".to_string(),
            "must implement Iterator".to_string(),
        ],
    };
    let _ = synth.generate(&spec);
}

// ── refactor (todo → should_panic) ──────────────────────────────────────────

#[test]
#[should_panic]
fn refactor_basic_code() {
    let synth = CodeSynthesizer::with_defaults();
    let _ = synth.refactor("fn add(a: i32, b: i32) -> i32 { return a + b; }");
}

#[test]
#[should_panic]
fn refactor_empty_code() {
    let synth = CodeSynthesizer::with_defaults();
    let _ = synth.refactor("");
}

// ── analyze (todo → should_panic) ───────────────────────────────────────────

#[test]
#[should_panic]
fn analyze_basic_code() {
    let synth = CodeSynthesizer::with_defaults();
    let _ = synth.analyze("fn hello() { println!(\"hi\"); }");
}

#[test]
#[should_panic]
fn analyze_complex_code() {
    let synth = CodeSynthesizer::with_defaults();
    let code = r#"
        fn process(items: &[Item]) -> Result<Vec<Output>, Error> {
            items.iter()
                .filter(|i| i.is_valid())
                .map(|i| transform(i))
                .collect()
        }
    "#;
    let _ = synth.analyze(code);
}

// ── serde roundtrip (PASS) ──────────────────────────────────────────────────

#[test]
fn code_spec_serde_roundtrip() {
    let spec = CodeSpec {
        description: "Sort a vector".to_string(),
        language: "rust".to_string(),
        constraints: vec!["stable sort".to_string(), "in-place".to_string()],
    };
    let json = serde_json::to_string(&spec).expect("serialize");
    let back: CodeSpec = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(spec, back);
}

#[test]
fn generated_code_serde_roundtrip() {
    let gc = GeneratedCode {
        code: "fn add(a: i32, b: i32) -> i32 { a + b }".to_string(),
        language: "rust".to_string(),
        explanation: "Simple addition function".to_string(),
    };
    let json = serde_json::to_string(&gc).expect("serialize");
    let back: GeneratedCode = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(gc, back);
}

#[test]
fn code_analysis_serde_roundtrip() {
    let analysis = CodeAnalysis {
        complexity: 5,
        issues: vec!["unused variable".to_string()],
        suggestions: vec!["use _prefix for unused vars".to_string()],
    };
    let json = serde_json::to_string(&analysis).expect("serialize");
    let back: CodeAnalysis = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(analysis, back);
}
