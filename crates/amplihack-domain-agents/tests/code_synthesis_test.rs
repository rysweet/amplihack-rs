use amplihack_domain_agents::{
    CodeAnalysis, CodeSpec, CodeSynthesisConfig, CodeSynthesizer, DomainError, GeneratedCode,
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

// ── generate (honest CodeSynthesis error — issue #874) ──────────────────────

#[test]
fn generate_from_spec() {
    let synth = CodeSynthesizer::with_defaults();
    let spec = CodeSpec {
        description: "A function that adds two numbers".to_string(),
        language: "rust".to_string(),
        constraints: vec!["must be generic".to_string()],
    };
    let err = synth.generate(&spec).unwrap_err();
    match err {
        DomainError::CodeSynthesis(msg) => {
            assert!(msg.contains("rust"), "should name the language: {msg}");
            assert!(
                !msg.contains("adds two numbers"),
                "must not echo description: {msg}"
            );
            assert!(
                !msg.contains("must be generic"),
                "must not echo constraints: {msg}"
            );
        }
        other => panic!("expected CodeSynthesis error, got {other:?}"),
    }
}

#[test]
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
    let err = synth.generate(&spec).unwrap_err();
    assert!(
        matches!(err, DomainError::CodeSynthesis(_)),
        "expected CodeSynthesis error, got {err:?}"
    );
}

#[test]
fn generate_empty_spec_is_invalid_input() {
    let synth = CodeSynthesizer::with_defaults();
    let spec = CodeSpec {
        description: "   ".to_string(),
        language: "rust".to_string(),
        constraints: vec![],
    };
    let err = synth.generate(&spec).unwrap_err();
    assert!(
        matches!(err, DomainError::InvalidInput(_)),
        "expected InvalidInput error, got {err:?}"
    );
}

// ── refactor (honest errors — issue #874) ───────────────────────────────────

#[test]
fn refactor_basic_code() {
    let synth = CodeSynthesizer::with_defaults();
    let err = synth
        .refactor("fn add(a: i32, b: i32) -> i32 { return a + b; }")
        .unwrap_err();
    assert!(
        matches!(err, DomainError::CodeSynthesis(_)),
        "expected CodeSynthesis error, got {err:?}"
    );
    assert!(
        !err.to_string().contains("fn add"),
        "must not echo the code body: {err}"
    );
}

#[test]
fn refactor_empty_code() {
    let synth = CodeSynthesizer::with_defaults();
    let err = synth.refactor("").unwrap_err();
    assert!(
        matches!(err, DomainError::InvalidInput(_)),
        "expected InvalidInput error, got {err:?}"
    );
}

// ── analyze (real heuristic — unchanged) ────────────────────────────────────

#[test]
fn analyze_basic_code() {
    let synth = CodeSynthesizer::with_defaults();
    let analysis = synth.analyze("fn hello() { println!(\"hi\"); }").unwrap();
    assert!(analysis.complexity > 0);
}

#[test]
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
    let analysis = synth.analyze(code).unwrap();
    assert!(analysis.complexity > 0);
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
