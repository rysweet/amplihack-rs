use amplihack_domain_agents::{DomainAgentType, IntentRouter, RoutingDecision};

// ── Construction & accessors (PASS) ─────────────────────────────────────────

#[test]
fn new_with_threshold() {
    let router = IntentRouter::new(0.8);
    assert!((router.confidence_threshold() - 0.8).abs() < f64::EPSILON);
}

#[test]
fn with_defaults_uses_half() {
    let router = IntentRouter::with_defaults();
    assert!((router.confidence_threshold() - 0.5).abs() < f64::EPSILON);
}

#[test]
fn confidence_threshold_accessor() {
    let router = IntentRouter::new(0.75);
    assert!((router.confidence_threshold() - 0.75).abs() < f64::EPSILON);
}

// ── supported_types (REAL implementation → PASS) ────────────────────────────

#[test]
fn supported_types_returns_all_five() {
    let router = IntentRouter::with_defaults();
    let types = router.supported_types();
    assert_eq!(types.len(), 5);
    assert!(types.contains(&DomainAgentType::Teaching));
    assert!(types.contains(&DomainAgentType::Security));
    assert!(types.contains(&DomainAgentType::CodeSynthesis));
    assert!(types.contains(&DomainAgentType::Learning));
    assert!(types.contains(&DomainAgentType::Research));
}

// ── route (todo → should_panic) ─────────────────────────────────────────────

#[test]
#[should_panic]
fn route_basic_input() {
    let router = IntentRouter::with_defaults();
    let _ = router.route("teach me about Rust lifetimes");
}

#[test]
#[should_panic]
fn route_empty_input() {
    let router = IntentRouter::with_defaults();
    let _ = router.route("");
}

// ── route_with_context (todo → should_panic) ────────────────────────────────

#[test]
#[should_panic]
fn route_with_context_basic() {
    let router = IntentRouter::with_defaults();
    let _ = router.route_with_context("scan for vulnerabilities", "security audit project");
}

#[test]
#[should_panic]
fn route_with_context_empty() {
    let router = IntentRouter::with_defaults();
    let _ = router.route_with_context("", "");
}

// ── serde roundtrip (PASS) ──────────────────────────────────────────────────

#[test]
fn routing_decision_serde_roundtrip() {
    let rd = RoutingDecision {
        agent_type: DomainAgentType::Security,
        confidence: 0.92,
        reasoning: "Input mentions vulnerability scanning".to_string(),
    };
    let json = serde_json::to_string(&rd).expect("serialize");
    let back: RoutingDecision = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(rd, back);
}

#[test]
fn domain_agent_type_display() {
    assert_eq!(format!("{}", DomainAgentType::Teaching), "teaching");
    assert_eq!(format!("{}", DomainAgentType::Security), "security");
    assert_eq!(format!("{}", DomainAgentType::CodeSynthesis), "code_synthesis");
    assert_eq!(format!("{}", DomainAgentType::Learning), "learning");
    assert_eq!(format!("{}", DomainAgentType::Research), "research");
}

#[test]
fn domain_agent_type_serde_roundtrip() {
    let variants = vec![
        DomainAgentType::Teaching,
        DomainAgentType::Security,
        DomainAgentType::CodeSynthesis,
        DomainAgentType::Learning,
        DomainAgentType::Research,
    ];
    for variant in variants {
        let json = serde_json::to_string(&variant).expect("serialize");
        let back: DomainAgentType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(variant, back);
    }
}
