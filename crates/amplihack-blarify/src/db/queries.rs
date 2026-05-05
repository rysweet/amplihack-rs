//! Cypher query constants for graph database operations.
//!
//! Mirrors the Python `repositories/graph_db_manager/queries.py`.

/// Get the codebase skeleton structure.
pub const CODEBASE_SKELETON_QUERY: &str = r#"
MATCH (n:NODE {entityId: $entity_id, repoId: $repo_id})
WHERE n.layer = 'code'
OPTIONAL MATCH (parent:NODE)-[:CONTAINS]->(n)
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path, parent.node_id AS parent_id
ORDER BY n.path
"#;

/// Get detailed information about a specific node.
pub const NODE_DETAILS_QUERY: &str = r#"
MATCH (n:NODE {node_id: $node_id, entityId: $entity_id, repoId: $repo_id})
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path, n.start_line AS start_line, n.end_line AS end_line,
       coalesce(n.text, '') AS content
"#;

/// Get all relationships for a node.
pub const NODE_RELATIONSHIPS_QUERY: &str = r#"
MATCH (n:NODE {node_id: $node_id, entityId: $entity_id, repoId: $repo_id})
OPTIONAL MATCH (n)-[r]->(target:NODE)
OPTIONAL MATCH (source:NODE)-[r2]->(n)
RETURN type(r) AS outbound_type, target.node_id AS target_id, target.name AS target_name,
       type(r2) AS inbound_type, source.node_id AS source_id, source.name AS source_name
"#;

/// Get code nodes by a list of IDs.
pub const CODE_NODES_BY_IDS_QUERY: &str = r#"
UNWIND $node_ids AS nid
MATCH (n:NODE {node_id: nid, entityId: $entity_id, repoId: $repo_id})
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path, n.start_line AS start_line, n.end_line AS end_line,
       coalesce(n.text, '') AS content
"#;

/// Get all leaf nodes (functions with no calls, files with no definitions).
pub const ALL_LEAF_NODES_QUERY: &str = r#"
MATCH (n:NODE {entityId: $entity_id, repoId: $repo_id})
WHERE n.layer = 'code'
  AND (
    ('FUNCTION' IN labels(n) AND NOT (n)-[:CALLS]->(:NODE))
    OR ('FILE' IN labels(n)
        AND NOT (n)-[:FUNCTION_DEFINITION|CLASS_DEFINITION]->(:NODE)
        AND NOT (n)-[:CALLS]->(:NODE))
    OR ('FOLDER' IN labels(n) AND NOT (n)-[:CONTAINS]->(:NODE))
  )
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path, n.start_line AS start_line, n.end_line AS end_line,
       coalesce(n.text, '') AS content
"#;

/// Get leaf nodes within a specific folder.
pub const FOLDER_LEAF_NODES_QUERY: &str = r#"
MATCH (n:NODE {entityId: $entity_id, repoId: $repo_id})
WHERE n.path STARTS WITH $folder_path AND n.layer = 'code'
  AND (
    ('FUNCTION' IN labels(n) AND NOT (n)-[:CALLS]->(:NODE))
    OR ('FILE' IN labels(n)
        AND NOT (n)-[:FUNCTION_DEFINITION|CLASS_DEFINITION]->(:NODE)
        AND NOT (n)-[:CALLS]->(:NODE))
  )
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path, n.start_line AS start_line, n.end_line AS end_line,
       coalesce(n.text, '') AS content
"#;

/// Find a node by its file path.
pub const NODE_BY_PATH_QUERY: &str = r#"
MATCH (n:NODE {path: $path, entityId: $entity_id, repoId: $repo_id})
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path, n.start_line AS start_line, n.end_line AS end_line,
       coalesce(n.text, '') AS content
LIMIT 1
"#;

/// Get direct children of a node.
pub const DIRECT_CHILDREN_QUERY: &str = r#"
MATCH (parent:NODE {node_id: $node_id, entityId: $entity_id, repoId: $repo_id})
MATCH (parent)-[:CONTAINS|FUNCTION_DEFINITION|CLASS_DEFINITION]->(child:NODE)
RETURN child.node_id AS id, child.name AS name, labels(child) AS labels,
       child.path AS path, child.start_line AS start_line, child.end_line AS end_line,
       coalesce(child.text, '') AS content
"#;

/// Find potential entry points (e.g. main functions, route handlers).
pub const POTENTIAL_ENTRY_POINTS_QUERY: &str = r#"
MATCH (n:NODE {entityId: $entity_id, repoId: $repo_id})
WHERE n.layer = 'code'
  AND NOT ()-[:CALLS]->(n)
  AND (n)-[:CALLS]->()
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path
"#;

/// Search nodes by text content.
pub const FIND_NODES_BY_TEXT_QUERY: &str = r#"
MATCH (n:NODE {entityId: $entity_id, repoId: $repo_id})
WHERE n.text CONTAINS $search_text
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path, n.start_line AS start_line, n.end_line AS end_line,
       substring(n.text, 0, 200) AS snippet
"#;

/// Grep code content with pattern matching.
pub const GREP_CODE_QUERY: &str = r#"
MATCH (n:NODE {entityId: $entity_id, repoId: $repo_id})
WHERE n.layer = 'code' AND n.text IS NOT NULL
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path, n.start_line AS start_line, n.end_line AS end_line,
       n.text AS text
"#;

/// Get file context by node ID (returns parent chain).
pub const FILE_CONTEXT_BY_ID_QUERY: &str = r#"
MATCH (target:NODE {node_id: $node_id, entityId: $entity_id, repoId: $repo_id})
OPTIONAL MATCH path = (ancestor:FILE)-[:CONTAINS|FUNCTION_DEFINITION|CLASS_DEFINITION*]->(target)
WITH target, ancestor, nodes(path) AS chain
UNWIND chain AS n
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path, coalesce(n.text, '') AS content
"#;

/// Generate a Mermaid dependency graph from a node.
pub const MERMAID_GRAPH_QUERY: &str = r#"
MATCH (root:NODE {node_id: $node_id, entityId: $entity_id, repoId: $repo_id})
OPTIONAL MATCH (root)-[r:CALLS|REFERENCES|IMPORTS]->(target:NODE)
RETURN root.node_id AS source_id, root.name AS source_name,
       type(r) AS rel_type,
       target.node_id AS target_id, target.name AS target_name
"#;

/// Get code content by node ID.
pub const CODE_BY_ID_QUERY: &str = r#"
MATCH (n:NODE {node_id: $node_id, entityId: $entity_id, repoId: $repo_id})
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path, n.start_line AS start_line, n.end_line AS end_line,
       coalesce(n.text, '') AS content
"#;

/// Look up existing documentation for a node.
pub const EXISTING_DOCUMENTATION_QUERY: &str = r#"
MATCH (doc:DOCUMENTATION)-[:DESCRIBES]->(n:NODE {node_id: $node_id})
RETURN doc.content AS content, doc.node_id AS doc_id
"#;

/// Find entry points relevant to specific file paths.
pub const ENTRY_POINTS_FOR_FILE_PATHS_QUERY: &str = r#"
MATCH (n:NODE {entityId: $entity_id, repoId: $repo_id})
WHERE n.path IN $file_paths AND n.layer = 'code'
  AND NOT ()-[:CALLS]->(n)
  AND (n)-[:CALLS]->()
RETURN n.node_id AS id, n.name AS name, labels(n) AS labels,
       n.path AS path
"#;

/// Get documentation nodes that need embeddings.
pub const DOCUMENTATION_FOR_EMBEDDING_QUERY: &str = r#"
MATCH (doc:DOCUMENTATION {entityId: $entity_id, repoId: $repo_id})
WHERE doc.embedding IS NULL
RETURN doc.node_id AS id, doc.content AS content
LIMIT $batch_size
"#;

/// Update documentation embeddings in bulk.
pub const UPDATE_EMBEDDINGS_QUERY: &str = r#"
UNWIND $updates AS update
MATCH (doc:DOCUMENTATION {node_id: update.id})
SET doc.embedding = update.embedding
RETURN count(doc) AS updated_count
"#;

/// Create a vector similarity search index.
pub const CREATE_VECTOR_INDEX_QUERY: &str = r#"
CALL db.index.vector.createNodeIndex(
  'doc_embedding_index', 'DOCUMENTATION', 'embedding', $dimensions, 'cosine'
)
"#;

/// Run vector similarity search over documentation embeddings.
pub const VECTOR_SIMILARITY_SEARCH_QUERY: &str = r#"
CALL db.index.vector.queryNodes('doc_embedding_index', $top_k, $query_embedding)
YIELD node, score
WHERE score >= $min_similarity
MATCH (node)-[:DESCRIBES]->(source:NODE)
RETURN node.node_id AS doc_id, node.content AS content,
       score AS similarity,
       source.node_id AS source_id, source.name AS source_name,
       source.path AS source_path, labels(source) AS source_labels
ORDER BY score DESC
"#;

/// Look up a node by its unique ID.
pub const NODE_BY_ID_QUERY: &str = r#"
MATCH (n:NODE {node_id: $node_id, entityId: $entity_id, repoId: $repo_id})
OPTIONAL MATCH (n)-[out_r]->(out_n:NODE)
OPTIONAL MATCH (in_n:NODE)-[in_r]->(n)
OPTIONAL MATCH (doc:DOCUMENTATION)-[:DESCRIBES]->(n)
RETURN n.node_id AS node_id, n.name AS node_name, labels(n) AS node_labels,
       n.path AS node_path, coalesce(n.text, '') AS code,
       n.start_line AS start_line, n.end_line AS end_line,
       collect(DISTINCT {type: type(out_r), id: out_n.node_id, name: out_n.name, labels: labels(out_n)}) AS outbound,
       collect(DISTINCT {type: type(in_r), id: in_n.node_id, name: in_n.name, labels: labels(in_n)}) AS inbound,
       doc.content AS documentation
"#;

/// Find nodes by name and type.
pub const NODE_BY_NAME_AND_TYPE_QUERY: &str = r#"
MATCH (n:NODE {entityId: $entity_id, repoId: $repo_id})
WHERE n.name = $name AND $node_type IN labels(n)
OPTIONAL MATCH (file:FILE)-[:CONTAINS|FUNCTION_DEFINITION|CLASS_DEFINITION*]->(n)
RETURN n.node_id AS node_id, n.name AS node_name, labels(n) AS node_type,
       coalesce(file.path, n.path) AS file_path,
       n.text AS code
"#;

/// Find code workflow paths from an entry point.
pub const CODE_WORKFLOWS_QUERY: &str = r#"
MATCH (entry:NODE {node_id: $entry_point_id, entityId: $entity_id, repoId: $repo_id})
MATCH path = (entry)-[:CALLS*1..]->(target:NODE)
WHERE length(path) <= $max_depth
WITH entry, path, nodes(path) AS chain, relationships(path) AS rels
RETURN [n IN chain | {id: n.node_id, name: n.name, labels: labels(n), path: n.path}] AS nodes,
       [r IN rels | {type: type(r)}] AS edges,
       length(path) AS depth
LIMIT $batch_size
"#;

/// Clean up stale processing status markers.
pub const CLEANUP_PROCESSING_QUERY: &str = r#"
MATCH (n:NODE {entityId: $entity_id, repoId: $repo_id})
WHERE n.processing_status IS NOT NULL
REMOVE n.processing_status, n.processing_run_id
RETURN count(n) AS cleaned_count
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_constants_not_empty() {
        #[allow(clippy::const_is_empty)]
        {
            assert!(!CODEBASE_SKELETON_QUERY.is_empty());
            assert!(!NODE_DETAILS_QUERY.is_empty());
            assert!(!NODE_RELATIONSHIPS_QUERY.is_empty());
            assert!(!GREP_CODE_QUERY.is_empty());
        }
    }

    #[test]
    fn queries_contain_expected_params() {
        assert!(CODEBASE_SKELETON_QUERY.contains("$entity_id"));
        assert!(CODEBASE_SKELETON_QUERY.contains("$repo_id"));
        assert!(NODE_BY_ID_QUERY.contains("$node_id"));
    }

    #[test]
    fn queries_return_expected_fields() {
        assert!(CODE_BY_ID_QUERY.contains("AS id"));
        assert!(NODE_BY_NAME_AND_TYPE_QUERY.contains("AS node_id"));
        assert!(VECTOR_SIMILARITY_SEARCH_QUERY.contains("AS similarity"));
    }

    #[test]
    fn workflow_query_has_depth_limit() {
        assert!(CODE_WORKFLOWS_QUERY.contains("$max_depth"));
        assert!(CODE_WORKFLOWS_QUERY.contains("$batch_size"));
    }

    #[test]
    fn cleanup_query_removes_processing_fields() {
        assert!(CLEANUP_PROCESSING_QUERY.contains("REMOVE n.processing_status"));
    }
}
