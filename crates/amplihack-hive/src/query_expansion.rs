//! Query expansion for improved hive mind retrieval.
//!
//! Expands user queries into semantically richer variants using a local
//! synonym map. In the Python implementation an LLM fallback exists; here
//! we provide the dependency-free local expansion which is always available.
//!
//! - Single responsibility: expand queries, search with expanded queries.
//! - No persistent state: each call is independent.

use std::collections::HashSet;
use std::sync::LazyLock;

/// Maximum number of expanded queries (including original).
pub const MAX_EXPANSIONS: usize = 4;

/// Default expansion model name (for compatibility with Python config).
pub const EXPANSION_MODEL: &str = "claude-haiku-4-5-20251001";

// ---------------------------------------------------------------------------
// Synonym map
// ---------------------------------------------------------------------------

struct SynonymEntry {
    word: &'static str,
    synonyms: &'static [&'static str],
}

static SYNONYM_TABLE: LazyLock<Vec<SynonymEntry>> = LazyLock::new(|| {
    vec![
        SynonymEntry {
            word: "error",
            synonyms: &["exception", "failure", "bug"],
        },
        SynonymEntry {
            word: "fix",
            synonyms: &["repair", "resolve", "patch"],
        },
        SynonymEntry {
            word: "performance",
            synonyms: &["speed", "latency", "throughput"],
        },
        SynonymEntry {
            word: "memory",
            synonyms: &["storage", "cache", "buffer"],
        },
        SynonymEntry {
            word: "test",
            synonyms: &["verify", "validate", "check"],
        },
        SynonymEntry {
            word: "deploy",
            synonyms: &["release", "ship", "publish"],
        },
        SynonymEntry {
            word: "config",
            synonyms: &["configuration", "settings", "parameters"],
        },
        SynonymEntry {
            word: "auth",
            synonyms: &["authentication", "authorization", "login"],
        },
        SynonymEntry {
            word: "api",
            synonyms: &["endpoint", "interface", "service"],
        },
        SynonymEntry {
            word: "database",
            synonyms: &["db", "storage", "datastore"],
        },
    ]
});

// ---------------------------------------------------------------------------
// Local synonym expansion
// ---------------------------------------------------------------------------

fn local_expand(query: &str) -> Vec<String> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    let mut expansions = vec![query.to_string()];

    for word in &words {
        if let Some(entry) = SYNONYM_TABLE.iter().find(|e| e.word == word.as_str()) {
            for &syn in entry.synonyms.iter().take(2) {
                let expanded = query.replace(word.as_str(), syn);
                if expanded != query && !expansions.contains(&expanded) {
                    expansions.push(expanded);
                    if expansions.len() >= MAX_EXPANSIONS {
                        return expansions;
                    }
                }
            }
        }
    }
    expansions
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Expand a query into semantically richer search variants.
///
/// Uses a built-in synonym map to generate alternative phrasings that
/// capture different aspects of the user's intent.
///
/// Returns a list of query strings, original first, then expansions.
pub fn expand_query(query: &str, max_expansions: usize) -> Vec<String> {
    let query = query.trim();
    if query.is_empty() {
        return if query.is_empty() {
            vec![]
        } else {
            vec![query.to_string()]
        };
    }
    let mut result = local_expand(query);
    result.truncate(max_expansions);
    result
}

/// Search with expanded queries and merge results.
///
/// Expands the query, calls `search_fn` for each variant, and merges
/// results by deduplicating on the string representation.
///
/// `search_fn` receives `(query, limit)` and returns a `Vec<String>` of results.
pub fn search_expanded(
    query: &str,
    search_fn: impl Fn(&str, usize) -> Vec<String>,
    limit: usize,
) -> Vec<String> {
    let expanded = expand_query(query, MAX_EXPANSIONS);
    let mut seen: HashSet<String> = HashSet::new();
    let mut results = Vec::new();

    for variant in &expanded {
        if let Ok(facts) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| search_fn(variant, limit)))
        {
            for fact in facts {
                if seen.insert(fact.clone()) {
                    results.push(fact);
                }
            }
        }
    }
    results.truncate(limit);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_empty_query() {
        assert!(expand_query("", 4).is_empty());
        assert!(expand_query("   ", 4).is_empty());
    }

    #[test]
    fn expand_no_synonyms() {
        let result = expand_query("quantum physics", 4);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "quantum physics");
    }

    #[test]
    fn expand_with_synonym() {
        let result = expand_query("fix error", 4);
        assert!(result.len() > 1);
        assert_eq!(result[0], "fix error");
        // Should contain synonym expansions
        let has_expansion = result.iter().any(|q| q != "fix error");
        assert!(has_expansion);
    }

    #[test]
    fn expand_respects_max() {
        let result = expand_query("fix error test config", 2);
        assert!(result.len() <= 2);
    }

    #[test]
    fn expand_includes_original_first() {
        let result = expand_query("deploy api", 4);
        assert_eq!(result[0], "deploy api");
    }

    #[test]
    fn expand_no_duplicates() {
        let result = expand_query("test error", 10);
        let unique: HashSet<_> = result.iter().collect();
        assert_eq!(unique.len(), result.len());
    }

    #[test]
    fn search_expanded_deduplicates() {
        let search_fn = |_query: &str, _limit: usize| -> Vec<String> {
            vec!["result-a".to_string(), "result-b".to_string()]
        };
        let results = search_expanded("error handling", search_fn, 10);
        let unique: HashSet<_> = results.iter().collect();
        assert_eq!(unique.len(), results.len());
    }

    #[test]
    fn search_expanded_respects_limit() {
        let search_fn = |_query: &str, _limit: usize| -> Vec<String> {
            (0..50).map(|i| format!("r-{i}")).collect()
        };
        let results = search_expanded("test", search_fn, 5);
        assert!(results.len() <= 5);
    }

    #[test]
    fn search_expanded_merges_across_variants() {
        let call_count = std::sync::atomic::AtomicUsize::new(0);
        let search_fn = |_query: &str, _limit: usize| -> Vec<String> {
            let n = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            vec![format!("result-{n}")]
        };
        let results = search_expanded("error handling", search_fn, 20);
        // Should have results from multiple query variants
        assert!(!results.is_empty());
    }

    #[test]
    fn synonym_map_coverage() {
        // Verify all synonym entries produce expansions
        let test_words = [
            "error",
            "fix",
            "performance",
            "memory",
            "test",
            "deploy",
            "config",
            "auth",
            "api",
            "database",
        ];
        for word in test_words {
            let result = expand_query(word, 4);
            assert!(result.len() > 1, "no expansion for '{word}'");
        }
    }

    #[test]
    fn expansion_model_constant() {
        #[allow(clippy::const_is_empty)]
        {
            assert!(!EXPANSION_MODEL.is_empty());
        }
    }
}
