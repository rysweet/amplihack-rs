//! PyO3 bindings for the amplihack-memory crate.
//!
//! Exposes the core memory API as a Python module (`amplihack_memory_rs`),
//! providing a drop-in replacement for the Python `amplihack.memory` library
//! with native Rust performance.
//!
//! Enable via: `cargo build --features pyo3-bindings`

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::backend::InMemoryBackend;
use crate::config::MemoryConfig;
use crate::facade::{MemoryFacade, RecallOptions, StoreOptions};
use crate::models::{MemoryEntry, MemoryType};

// ── Helper: convert MemoryType to/from Python string ──

fn parse_memory_type(s: &str) -> PyResult<MemoryType> {
    match s {
        "episodic" => Ok(MemoryType::Episodic),
        "semantic" => Ok(MemoryType::Semantic),
        "procedural" => Ok(MemoryType::Procedural),
        "prospective" => Ok(MemoryType::Prospective),
        "working" => Ok(MemoryType::Working),
        "strategic" => Ok(MemoryType::Strategic),
        "code_context" => Ok(MemoryType::CodeContext),
        "project_structure" => Ok(MemoryType::ProjectStructure),
        "user_preference" => Ok(MemoryType::UserPreference),
        "error_pattern" => Ok(MemoryType::ErrorPattern),
        "conversation" => Ok(MemoryType::Conversation),
        "task" => Ok(MemoryType::Task),
        _ => Err(PyRuntimeError::new_err(format!("Unknown memory type: {s}"))),
    }
}

fn entry_to_py_dict<'py>(py: Python<'py>, entry: &MemoryEntry) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("id", &entry.id)?;
    dict.set_item("session_id", &entry.session_id)?;
    dict.set_item("agent_id", &entry.agent_id)?;
    dict.set_item("memory_type", entry.memory_type.as_str())?;
    dict.set_item("title", &entry.title)?;
    dict.set_item("content", &entry.content)?;
    dict.set_item("created_at", entry.created_at)?;
    dict.set_item("accessed_at", entry.accessed_at)?;
    dict.set_item("importance", entry.importance)?;
    dict.set_item("tags", entry.tags.iter().collect::<Vec<_>>())?;
    Ok(dict)
}

// ── Python-visible classes ──

/// Python wrapper around `MemoryFacade`.
#[pyclass(name = "Memory")]
struct PyMemory {
    inner: MemoryFacade,
}

#[pymethods]
impl PyMemory {
    /// Create a new in-memory store (for testing or ephemeral usage).
    #[new]
    fn new() -> PyResult<Self> {
        let config = MemoryConfig::for_testing();
        let backend = Box::new(InMemoryBackend::new());
        let facade = MemoryFacade::new(backend, config);
        Ok(Self { inner: facade })
    }

    /// Store a memory. Returns the entry ID.
    ///
    /// Args:
    ///     content: Text content to store
    ///     memory_type: One of "semantic", "episodic", "procedural", "working", "strategic", etc.
    ///     session_id: Session identifier (default: "default")
    ///     importance: Optional importance score 0.0-1.0
    #[pyo3(signature = (content, memory_type="semantic", session_id="default", importance=None))]
    fn remember(
        &mut self,
        content: &str,
        memory_type: &str,
        session_id: &str,
        importance: Option<f64>,
    ) -> PyResult<String> {
        let mt = parse_memory_type(memory_type)?;
        let opts = StoreOptions {
            memory_type: mt,
            session_id: session_id.to_string(),
            importance,
            ..Default::default()
        };
        self.inner
            .store_memory(content, opts)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    /// Alias for `remember()`.
    #[pyo3(signature = (content, memory_type="semantic", session_id="default", importance=None))]
    fn store(
        &mut self,
        content: &str,
        memory_type: &str,
        session_id: &str,
        importance: Option<f64>,
    ) -> PyResult<String> {
        self.remember(content, memory_type, session_id, importance)
    }

    /// Recall memories matching a query.
    ///
    /// Returns a list of dicts with memory entry fields.
    #[pyo3(signature = (query, session_id=None, memory_types=None, limit=20, token_budget=4000))]
    fn recall<'py>(
        &self,
        py: Python<'py>,
        query: &str,
        session_id: Option<String>,
        memory_types: Option<Vec<String>>,
        limit: usize,
        token_budget: usize,
    ) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let types = match memory_types {
            Some(ref names) => {
                let mut v = Vec::new();
                for name in names {
                    v.push(parse_memory_type(name)?);
                }
                v
            }
            None => Vec::new(),
        };

        let opts = RecallOptions {
            session_id,
            memory_types: types,
            token_budget,
            limit,
        };

        let entries = self
            .inner
            .recall(query, opts)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;

        entries.iter().map(|e| entry_to_py_dict(py, e)).collect()
    }

    /// Delete a memory by ID.
    fn forget(&mut self, entry_id: &str) -> PyResult<bool> {
        self.inner
            .forget(entry_id)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))
    }

    /// Get basic statistics.
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let s = self
            .inner
            .stats()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let dict = PyDict::new(py);
        for (k, v) in &s {
            dict.set_item(k, v.to_string())?;
        }
        Ok(dict)
    }

    /// Get the backend name.
    fn backend_name(&self) -> &str {
        self.inner.backend_name()
    }
}

/// Python-visible bloom filter.
#[pyclass(name = "BloomFilter")]
struct PyBloomFilter {
    inner: crate::bloom::BloomFilter,
}

#[pymethods]
impl PyBloomFilter {
    #[new]
    #[pyo3(signature = (expected_items=1000, fp_rate=0.01))]
    fn new(expected_items: usize, fp_rate: f64) -> Self {
        Self {
            inner: crate::bloom::BloomFilter::new(expected_items, fp_rate),
        }
    }

    fn add(&mut self, item: &str) {
        self.inner.add(item);
    }

    fn might_contain(&self, item: &str) -> bool {
        self.inner.might_contain(item)
    }

    fn count(&self) -> usize {
        self.inner.count()
    }

    fn size_bytes(&self) -> usize {
        self.inner.size_bytes()
    }
}

/// Register the `amplihack_memory_rs` Python module.
#[pymodule]
pub fn amplihack_memory_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyMemory>()?;
    m.add_class::<PyBloomFilter>()?;
    Ok(())
}
