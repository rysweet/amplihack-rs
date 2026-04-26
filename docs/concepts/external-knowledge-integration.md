# External Knowledge Integration

Complete implementation of external knowledge integration for the Neo4j memory
system, allowing the framework to fetch, cache, and link external documentation
sources to code and memories.

!!! note "Rust Port"
    In amplihack-rs, external knowledge integration is accessed through the
    `amplihack memory` subcommands. The Python API examples below show the
    upstream interface for reference.

## Overview

The External Knowledge Integration system provides:

- **Fetching & Caching**: Retrieve external documentation with HTTP caching and TTL
- **Graph Storage**: Store documentation in Neo4j with relationships to code and memories
- **Version Tracking**: Support multiple versions (e.g., Python 3.10 vs 3.12 docs)
- **Credibility Scoring**: Trust scores for different sources
- **Query & Retrieval**: Search and retrieve relevant documentation

## Architecture

### Core Components

```
Upstream Python layout (reference only):

src/amplihack/memory/neo4j/
├── external_knowledge.py      # Main implementation
│   ├── KnowledgeSource         # Enum: PYTHON_DOCS, MS_LEARN, GITHUB, etc.
│   ├── ExternalDoc             # Document dataclass
│   ├── APIReference            # API reference dataclass
│   └── ExternalKnowledgeManager # Main manager class
└── __init__.py                 # Exports external knowledge components
```

### Graph Schema

```cypher
# Nodes
(:ExternalDoc {
    url: STRING (UNIQUE),
    title: STRING,
    content: TEXT,
    source: STRING,
    version: STRING,
    trust_score: FLOAT,
    metadata: JSON,
    fetched_at: DATETIME,
    ttl_hours: INT
})

(:APIReference {
    id: STRING (UNIQUE),
    name: STRING,
    signature: STRING,
    doc_url: STRING,
    description: TEXT,
    examples: JSON,
    source: STRING,
    version: STRING
})

# Relationships
(:ExternalDoc)-[:EXPLAINS]->(:CodeFile)
(:ExternalDoc)-[:DOCUMENTS]->(:Function)
(:Memory)-[:SOURCED_FROM]->(:ExternalDoc)
```

## Usage

### Python API (upstream reference)

#### Basic Document Fetching

```python
# Upstream Python API (reference only)
from amplihack.memory.neo4j import (
    Neo4jConnector,
    ExternalKnowledgeManager,
    KnowledgeSource,
)

with Neo4jConnector() as conn:
    manager = ExternalKnowledgeManager(conn)
    manager.initialize_knowledge_schema()

    doc = manager.fetch_api_docs(
        url="https://docs.python.org/3/library/json.html",
        source=KnowledgeSource.PYTHON_DOCS,
        version="3.10",
        trust_score=0.95,
    )

    if doc:
        manager.cache_external_doc(doc)
```

#### Linking to Code

```python
# Upstream Python API (reference only)
# Link documentation to code file
manager.link_to_code(
    doc_url="https://docs.python.org/3/library/json.html",
    code_path="src/data_processing.rs",
    relationship_type="EXPLAINS",
    metadata={"reason": "Uses json module extensively"},
)

# Link documentation to function
manager.link_to_function(
    doc_url="https://docs.python.org/3/library/json.html",
    function_id="parse_json_data:1.0",
)

# Link memory to documentation source
manager.link_to_memory(
    doc_url="https://docs.python.org/3/library/json.html",
    memory_id="mem_12345",
    relationship_type="SOURCED_FROM",
)
```

#### Querying Knowledge

```python
# Upstream Python API (reference only)
results = manager.query_external_knowledge(
    query_text="json parsing",
    source=KnowledgeSource.PYTHON_DOCS,
    version="3.10",
    min_trust_score=0.9,
    limit=10,
)

for doc in results:
    print(f"{doc['title']} ({doc['url']})")
    print(f"  Trust: {doc['trust_score']}, Version: {doc['version']}")

# Get documentation for code file
docs = manager.get_code_documentation("src/data_processing.rs")

# Get documentation for function
docs = manager.get_function_documentation("parse_json_data:1.0")
```

#### API References

```python
# Upstream Python API (reference only)
from amplihack.memory.neo4j import APIReference, KnowledgeSource

api_ref = APIReference(
    name="serde_json::from_str",
    signature="pub fn from_str<'a, T>(s: &'a str) -> Result<T>",
    doc_url="https://docs.rs/serde_json/latest/serde_json/fn.from_str.html",
    description="Deserialize JSON string to Rust type",
    examples=[
        'let v: Value = serde_json::from_str(r#"{"key": "value"}"#)?;',
    ],
    source=KnowledgeSource.DOCS_RS,
    version="1.0",
)

manager.store_api_reference(api_ref)
```

#### Maintenance

```python
# Upstream Python API (reference only)
stats = manager.get_knowledge_stats()
print(f"Total docs: {stats['total_docs']}")
print(f"Average trust: {stats['avg_trust_score']:.2f}")
print(f"Total links: {stats['total_links']}")

# Cleanup expired documents
removed = manager.cleanup_expired_docs()
print(f"Removed {removed} expired documents")
```

### CLI Usage

```bash
# Import external documentation (upstream Python CLI, reference only)
# python scripts/import_external_knowledge.py python --version 3.10
# python scripts/import_external_knowledge.py python --version latest \
#     --pages library/asyncio.html library/pathlib.html
```

## Knowledge Sources

The system supports the following source types:

| Source | Trust Score | Description |
|---|---|---|
| `PYTHON_DOCS` | 0.95 | Official Python documentation |
| `DOCS_RS` | 0.95 | Official Rust crate documentation |
| `MS_LEARN` | 0.90 | Microsoft Learn articles |
| `GITHUB` | 0.80 | GitHub repositories and READMEs |
| `STACK_OVERFLOW` | 0.70 | Community Q&A |
| `BLOG` | 0.60 | Technical blog posts |

Trust scores are configurable per-source and affect query ranking.

## Version Tracking

External documentation supports version tracking:

```python
# Upstream Python API (reference only)
# Import Rust 1.75 docs
manager.fetch_api_docs(url="...", version="1.75", ...)

# Import Rust 1.80 docs
manager.fetch_api_docs(url="...", version="1.80", ...)

# Query specific version
results = manager.query_external_knowledge(
    query_text="async traits",
    version="1.80",
)
```

## TTL and Caching

Documents have a configurable TTL (Time-To-Live):

- Default: 168 hours (7 days)
- Official docs: 720 hours (30 days)
- Blog posts: 24 hours (1 day)

Expired documents are automatically refreshed on next access or removed during
cleanup.

## Related Documentation

- [Documentation Knowledge Graph](documentation-knowledge-graph.md) — internal doc graph
- [Doc Graph Quick Reference](../reference/doc-graph-quick-reference.md) — cheat sheet
- [Agent Memory Architecture](agent-memory-architecture.md) — overall memory system

---

**Status**: Upstream implementation complete ✓ | Rust port in progress
