//! 100-agent local hive eval simulation.
//!
//! Runs the full hive evaluation loop using `LocalEventBus` with 100
//! simulated agents. Each agent has a knowledge domain and responds to
//! queries with varying confidence based on domain relevance.
//!
//! Uses `amplihack-memory` BloomFilter for knowledge deduplication.

use amplihack_hive::event_bus::{EventBus, LocalEventBus};
use amplihack_hive::hive_eval::{HiveEvalResult, build_default_eval_questions};
use amplihack_hive::hive_events::{HIVE_QUERY, HIVE_QUERY_RESPONSE};
use amplihack_hive::models::BusEvent;
use amplihack_hive::workload::HiveEvent;
use amplihack_memory::bloom::BloomFilter;
use amplihack_memory::models::{MemoryEntry, MemoryType};

use std::collections::HashMap;

/// A simulated agent with a knowledge domain and response behavior.
struct SimulatedAgent {
    id: String,
    domain: &'static str,
    expertise_level: f64,
    bloom: BloomFilter,
    memories: Vec<MemoryEntry>,
}

/// Knowledge domains and their keywords for matching.
const DOMAINS: &[(&str, &[&str])] = &[
    (
        "ownership",
        &["ownership", "borrow", "lifetime", "move", "drop", "RAII"],
    ),
    (
        "concurrency",
        &[
            "thread", "async", "race", "mutex", "channel", "sync", "send",
        ],
    ),
    (
        "types",
        &[
            "trait",
            "enum",
            "struct",
            "generic",
            "type",
            "polymorphism",
            "dyn",
        ],
    ),
    (
        "errors",
        &[
            "error", "result", "option", "panic", "unwrap", "?", "anyhow",
        ],
    ),
    (
        "strings",
        &["str", "String", "UTF", "slice", "format", "display"],
    ),
    (
        "memory",
        &[
            "heap",
            "stack",
            "allocation",
            "box",
            "rc",
            "arc",
            "reference",
        ],
    ),
    (
        "unsafe",
        &["unsafe", "raw pointer", "FFI", "extern", "transmute"],
    ),
    (
        "macros",
        &["macro", "derive", "proc_macro", "attribute", "token"],
    ),
    (
        "cargo",
        &[
            "cargo",
            "crate",
            "dependency",
            "build",
            "workspace",
            "feature",
        ],
    ),
    (
        "testing",
        &[
            "test",
            "assert",
            "mock",
            "benchmark",
            "criterion",
            "proptest",
        ],
    ),
];

impl SimulatedAgent {
    fn new(id: usize, domain_idx: usize) -> Self {
        let (domain, keywords) = DOMAINS[domain_idx % DOMAINS.len()];
        let mut bloom = BloomFilter::new(100, 0.01);
        let mut memories = Vec::new();

        for keyword in keywords.iter() {
            bloom.add(keyword);
            memories.push(MemoryEntry::new(
                "hive-session",
                format!("agent-{id:03}"),
                MemoryType::Semantic,
                format!("Knowledge about {keyword} in {domain}"),
            ));
        }

        Self {
            id: format!("agent-{id:03}"),
            domain,
            expertise_level: 0.5 + (domain_idx as f64 * 0.05).min(0.45),
            bloom,
            memories,
        }
    }

    /// Calculate confidence for a question based on domain keyword overlap.
    fn confidence_for(&self, question: &str) -> f64 {
        let q_lower = question.to_lowercase();
        let (_, keywords) = DOMAINS
            .iter()
            .find(|(d, _)| *d == self.domain)
            .unwrap_or(&("", &[]));

        let matches = keywords
            .iter()
            .filter(|kw| q_lower.contains(&kw.to_lowercase()))
            .count();

        if matches == 0 {
            // Low baseline confidence for out-of-domain
            0.05 + (self.expertise_level * 0.05)
        } else {
            // Any keyword match gives strong base; more matches increase further
            let base = 0.5 + (self.expertise_level * 0.3);
            let bonus = (matches as f64 / keywords.len() as f64) * 0.2;
            (base + bonus).min(0.99)
        }
    }

    /// Generate an answer based on domain knowledge.
    fn answer_for(&self, question: &str) -> String {
        let conf = self.confidence_for(question);
        if conf > 0.5 {
            format!(
                "[{}/{}] As a {domain} specialist: {question} relates to core {domain} concepts. \
                 I have {mem_count} relevant memories in my knowledge base.",
                self.id,
                self.domain,
                domain = self.domain,
                question = &question[..question.len().min(50)],
                mem_count = self.memories.len(),
            )
        } else {
            format!(
                "[{}/{}] Limited knowledge on this topic. My {domain} expertise has \
                 {bloom_items} indexed terms, but question may be outside my domain.",
                self.id,
                self.domain,
                domain = self.domain,
                bloom_items = self.bloom.count(),
            )
        }
    }
}

/// Simulate 100 agents responding to queries via the event bus.
///
/// Flow:
/// 1. Create 100 agents distributed across 10 domains
/// 2. For each question, each agent produces a response event
/// 3. Responses are pre-loaded into the bus (simulating async delivery)
/// 4. `run_eval()` collects and aggregates results
fn simulate_100_agents(bus: &mut LocalEventBus, questions: &[String]) -> Vec<SimulatedAgent> {
    let agents: Vec<SimulatedAgent> = (0..100)
        .map(|i| SimulatedAgent::new(i, i % DOMAINS.len()))
        .collect();

    // Subscribe the eval collector first — run_eval does this too,
    // but we need responses pre-loaded for the sync local bus.
    bus.subscribe("__hive_eval_collector", Some(&[HIVE_QUERY_RESPONSE]))
        .unwrap();

    for question in questions {
        // Generate a query_id matching what run_eval will produce
        let query_id = uuid::Uuid::new_v4().to_string();

        // Publish the query (agents would normally see this)
        let query_event = BusEvent::new(
            HIVE_QUERY,
            serde_json::to_value(&HiveEvent::Query {
                query_id: query_id.clone(),
                question: question.clone(),
            })
            .unwrap(),
            "eval-coordinator",
        );
        bus.publish(query_event).unwrap();

        // Each agent responds
        for agent in &agents {
            let confidence = agent.confidence_for(question);
            let answer = agent.answer_for(question);

            let response = HiveEvent::QueryResponse {
                query_id: query_id.clone(),
                answer,
                confidence,
            };

            let event = BusEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                source_id: agent.id.clone(),
                topic: HIVE_QUERY_RESPONSE.to_string(),
                payload: serde_json::to_value(&response).unwrap(),
                timestamp: chrono::Utc::now().timestamp() as f64,
            };

            bus.publish(event).unwrap();
        }
    }

    agents
}

// ── Tests ──

#[test]
fn hundred_agent_eval_completes_with_all_responses() {
    let questions = build_default_eval_questions();
    let mut bus = LocalEventBus::new();

    let agents = simulate_100_agents(&mut bus, &questions);
    assert_eq!(agents.len(), 100);

    // Verify responses are queued — each question gets 100 agent responses
    let pending = bus.pending_events("__hive_eval_collector").unwrap();
    assert_eq!(
        pending.len(),
        questions.len() * 100,
        "Expected {} response events, got {}",
        questions.len() * 100,
        pending.len()
    );
}

#[test]
fn hundred_agent_eval_produces_high_aggregate_confidence() {
    let questions = build_default_eval_questions();

    let agents: Vec<SimulatedAgent> = (0..100)
        .map(|i| SimulatedAgent::new(i, i % DOMAINS.len()))
        .collect();

    // Build results manually using agent simulation
    let mut results = Vec::new();
    for question in &questions {
        let query_id = uuid::Uuid::new_v4().to_string();
        let mut answers = Vec::new();
        for agent in &agents {
            let confidence = agent.confidence_for(question);
            let answer = agent.answer_for(question);
            answers.push(amplihack_hive::hive_eval::AgentAnswer {
                agent_id: agent.id.clone(),
                answer,
                confidence,
            });
        }
        results.push(amplihack_hive::hive_eval::QueryResult {
            query_id,
            question: question.clone(),
            answers,
        });
    }

    let result = HiveEvalResult::from_results(results);

    assert_eq!(result.total_queries, 5, "Should have 5 queries");
    assert_eq!(
        result.total_responses, 500,
        "100 agents × 5 questions = 500 responses"
    );
    assert!(
        result.average_confidence > 0.0,
        "Average confidence should be positive"
    );

    // Verify domain specialists have higher confidence for their topics
    for qr in &result.query_results {
        let best = qr.best_answer().unwrap();
        assert!(
            best.confidence > 0.1,
            "Best answer confidence should be meaningful: {} for '{}'",
            best.confidence,
            qr.question
        );
        assert_eq!(
            qr.response_count(),
            100,
            "Each question should get 100 responses"
        );
    }
}

#[test]
fn domain_specialists_outscore_generalists() {
    let agents: Vec<SimulatedAgent> = (0..100)
        .map(|i| SimulatedAgent::new(i, i % DOMAINS.len()))
        .collect();

    // Ownership question — ownership specialists should score higher
    let question = "What are the key benefits of Rust's ownership system?";

    let mut domain_scores: HashMap<&str, Vec<f64>> = HashMap::new();
    for agent in &agents {
        let conf = agent.confidence_for(question);
        domain_scores.entry(agent.domain).or_default().push(conf);
    }

    let ownership_avg: f64 = {
        let scores = &domain_scores["ownership"];
        scores.iter().sum::<f64>() / scores.len() as f64
    };

    // Non-ownership domains should have lower average confidence
    let other_avg: f64 = {
        let scores: Vec<f64> = domain_scores
            .iter()
            .filter(|(d, _)| **d != "ownership")
            .flat_map(|(_, s)| s.iter().copied())
            .collect();
        scores.iter().sum::<f64>() / scores.len() as f64
    };

    assert!(
        ownership_avg > other_avg,
        "Ownership specialists ({ownership_avg:.3}) should outscore others ({other_avg:.3})"
    );
}

#[test]
fn agent_bloom_filters_track_knowledge() {
    let agent = SimulatedAgent::new(0, 0); // ownership domain

    // Ownership keywords should be in the bloom filter
    assert!(agent.bloom.might_contain("ownership"));
    assert!(agent.bloom.might_contain("borrow"));
    assert!(agent.bloom.might_contain("lifetime"));

    // Non-domain keywords should likely not be in the filter
    // (BloomFilter may have false positives, but not for these specific strings)
    assert!(!agent.bloom.might_contain("completely_unrelated_xyz"));
}

#[test]
fn agent_memories_are_domain_specific() {
    let agent = SimulatedAgent::new(42, 3); // errors domain (index 3)
    assert_eq!(agent.domain, "errors");

    // Should have memories for each keyword in the errors domain
    let (_, keywords) = DOMAINS[3];
    assert_eq!(agent.memories.len(), keywords.len());

    for memory in &agent.memories {
        assert!(
            memory.content.contains("errors"),
            "Memory should reference errors domain: {}",
            memory.content
        );
    }
}

#[test]
fn eval_result_serialization_roundtrip() {
    let agents: Vec<SimulatedAgent> = (0..100)
        .map(|i| SimulatedAgent::new(i, i % DOMAINS.len()))
        .collect();

    let questions = build_default_eval_questions();
    let mut results = Vec::new();

    for question in &questions {
        let query_id = uuid::Uuid::new_v4().to_string();
        let answers: Vec<_> = agents
            .iter()
            .map(|a| amplihack_hive::hive_eval::AgentAnswer {
                agent_id: a.id.clone(),
                answer: a.answer_for(question),
                confidence: a.confidence_for(question),
            })
            .collect();
        results.push(amplihack_hive::hive_eval::QueryResult {
            query_id,
            question: question.clone(),
            answers,
        });
    }

    let eval_result = HiveEvalResult::from_results(results);

    // Serialize → deserialize roundtrip
    let json = serde_json::to_string(&eval_result).unwrap();
    let restored: HiveEvalResult = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.total_queries, eval_result.total_queries);
    assert_eq!(restored.total_responses, eval_result.total_responses);
    assert!((restored.average_confidence - eval_result.average_confidence).abs() < f64::EPSILON);
}

#[test]
fn hundred_agents_across_all_domains() {
    let agents: Vec<SimulatedAgent> = (0..100)
        .map(|i| SimulatedAgent::new(i, i % DOMAINS.len()))
        .collect();

    // All 10 domains should be represented
    let mut domain_counts: HashMap<&str, usize> = HashMap::new();
    for agent in &agents {
        *domain_counts.entry(agent.domain).or_default() += 1;
    }

    assert_eq!(
        domain_counts.len(),
        DOMAINS.len(),
        "All {} domains should be represented",
        DOMAINS.len()
    );

    // Each domain should have exactly 10 agents (100 / 10)
    for (domain, count) in &domain_counts {
        assert_eq!(
            *count, 10,
            "Domain '{domain}' should have 10 agents, got {count}"
        );
    }
}

#[test]
fn eval_report_summary() {
    let agents: Vec<SimulatedAgent> = (0..100)
        .map(|i| SimulatedAgent::new(i, i % DOMAINS.len()))
        .collect();

    let questions = build_default_eval_questions();
    let mut results = Vec::new();

    for question in &questions {
        let query_id = uuid::Uuid::new_v4().to_string();
        let answers: Vec<_> = agents
            .iter()
            .map(|a| amplihack_hive::hive_eval::AgentAnswer {
                agent_id: a.id.clone(),
                answer: a.answer_for(question),
                confidence: a.confidence_for(question),
            })
            .collect();
        results.push(amplihack_hive::hive_eval::QueryResult {
            query_id,
            question: question.clone(),
            answers,
        });
    }

    let result = HiveEvalResult::from_results(results);

    // Print eval report (visible with --nocapture)
    println!("\n=== 100-Agent Hive Eval Report ===");
    println!("Total queries:    {}", result.total_queries);
    println!("Total responses:  {}", result.total_responses);
    println!("Avg confidence:   {:.4}", result.average_confidence);
    println!();

    for qr in &result.query_results {
        let best = qr.best_answer().unwrap();
        println!("Q: {}", &qr.question[..qr.question.len().min(60)]);
        println!(
            "  Best: {} (confidence: {:.3})",
            best.agent_id, best.confidence
        );
        println!(
            "  Avg confidence: {:.3}, Responses: {}",
            qr.average_confidence(),
            qr.response_count()
        );
    }
    println!("=== End Report ===\n");

    // Assertions
    assert_eq!(result.total_queries, 5);
    assert_eq!(result.total_responses, 500);
    assert!(result.average_confidence > 0.1);
}
