//! Cypher queries for documentation workflow operations.
//!
//! Mirrors the Python `documentation/queries/workflow_queries.py` and
//! `documentation/queries/batch_processing_queries.py`.

/// Delete orphaned documentation nodes that have no DESCRIBES relationship.
pub const CLEANUP_ORPHANED_DOCUMENTATION: &str = r#"
MATCH (doc:DOCUMENTATION {layer: 'documentation'})
WHERE NOT (doc)-[:DESCRIBES]->()
DETACH DELETE doc
RETURN count(doc) as deleted_orphans
"#;

/// Delete workflow nodes and relationships for given entry points.
pub const DELETE_WORKFLOWS_FOR_ENTRY_POINTS: &str = r#"
MATCH (w:NODE {layer: 'workflows'})
WHERE w.entry_point_id IN $entry_point_ids
WITH w, w.node_id as workflow_id
OPTIONAL MATCH ()-[ws:WORKFLOW_STEP]->()
WHERE ws.scopeText CONTAINS ('workflow_id:' + workflow_id)
DELETE ws
WITH w, count(ws) as deleted_steps
DETACH DELETE w
RETURN count(w) as deleted_workflows, sum(deleted_steps) as total_deleted_steps
"#;

/// Get a batch of leaf nodes for bottom-up processing.
pub const LEAF_NODES_BATCH: &str = r#"
MATCH (n:NODE {entityId: $entity_id, repoId: $repo_id})
WHERE (n.processing_status IS NULL OR n.processing_run_id <> $run_id) AND NOT n:DOCUMENTATION
  AND (
    ('FUNCTION' IN labels(n) AND NOT (n)-[:CALLS]->(:NODE))
    OR ('CLASS' IN labels(n)
      AND NOT (n)-[:CALLS]->(:NODE)
      AND NOT (n)-[:FUNCTION_DEFINITION|CLASS_DEFINITION]->(:NODE))
    OR ('FILE' IN labels(n)
     AND NOT (n)-[:FUNCTION_DEFINITION|CLASS_DEFINITION]->(:NODE)
     AND NOT (n)-[:CALLS]->(:NODE))
    OR ('FOLDER' IN labels(n)
     AND NOT (n)-[:CONTAINS]->(:NODE))
  )
WITH n LIMIT $batch_size
SET n.processing_status = 'in_progress',
    n.processing_run_id = $run_id
RETURN n.node_id as id, n.name as name, labels(n) as labels,
       n.path as path, n.start_line as start_line, n.end_line as end_line,
       coalesce(n.text, '') as content
"#;

/// Get processable nodes with their children's descriptions.
pub const PROCESSABLE_NODES_WITH_DESCRIPTIONS: &str = r#"
MATCH (root:NODE {node_id: $root_node_id, entityId: $entity_id, repoId: $repo_id})
MATCH (root)-[:CONTAINS|FUNCTION_DEFINITION|CLASS_DEFINITION|CALL*0..]->(n:NODE)
WHERE (n.processing_status IS NULL OR n.processing_run_id <> $run_id) AND NOT n:DOCUMENTATION

OPTIONAL MATCH (n)-[:CONTAINS|FUNCTION_DEFINITION|CLASS_DEFINITION]->(hier_child:NODE)
WITH n, collect(DISTINCT hier_child) as hier_children
WHERE ALL(child IN hier_children WHERE child.processing_status = 'completed' AND child.processing_run_id = $run_id)

OPTIONAL MATCH (n)-[:CALLS]->(call_child:NODE)
WITH n, hier_children, collect(DISTINCT call_child) as call_children
WHERE ALL(child IN call_children WHERE child.processing_status = 'completed' AND child.processing_run_id = $run_id)

OPTIONAL MATCH (hier_doc:DOCUMENTATION)-[:DESCRIBES]->(hier_child)
WHERE hier_child IN hier_children
WITH n, hier_children, call_children,
     collect(DISTINCT {id: hier_child.node_id, name: hier_child.name, labels: labels(hier_child), path: hier_child.path, description: hier_doc.content}) as hier_descriptions

OPTIONAL MATCH (call_doc:DOCUMENTATION)-[:DESCRIBES]->(call_child)
WHERE call_child IN call_children
WITH n, hier_descriptions,
     collect(DISTINCT {id: call_child.node_id, name: call_child.name, labels: labels(call_child), path: call_child.path, description: call_doc.content}) as call_descriptions

WITH n, hier_descriptions, call_descriptions
LIMIT $batch_size

SET n.processing_status = 'in_progress',
    n.processing_run_id = $run_id

RETURN n.node_id as id, n.name as name, labels(n) as labels,
       n.path as path, n.start_line as start_line, n.end_line as end_line,
       coalesce(n.text, '') as content,
       hier_descriptions, call_descriptions
"#;

/// Mark nodes as completed after documentation has been saved.
pub const MARK_NODES_COMPLETED: &str = r#"
UNWIND $node_ids as node_id
MATCH (n:NODE {node_id: node_id, entityId: $entity_id, repoId: $repo_id})
WHERE n.processing_run_id = $run_id
SET n.processing_status = 'completed'
RETURN count(n) as completed_count
"#;

/// Check if there are any pending nodes remaining under a root node.
pub const CHECK_PENDING_NODES: &str = r#"
MATCH (root:NODE {node_id: $root_node_id, entityId: $entity_id, repoId: $repo_id})
MATCH (root)-[:CONTAINS|FUNCTION_DEFINITION|CLASS_DEFINITION|CALL*0..]->(n:NODE)
WHERE n.processing_status IS NULL AND NOT n:DOCUMENTATION
RETURN count(n) as pending_count
"#;

/// Get child descriptions for a parent node.
pub const CHILD_DESCRIPTIONS: &str = r#"
MATCH (parent:NODE {node_id: $parent_node_id, entityId: $entity_id, repoId: $repo_id})
MATCH (parent)-[:CONTAINS|FUNCTION_DEFINITION|CLASS_DEFINITION|CALL]->(child:NODE)
MATCH (child_doc)-[:DESCRIBES]->(child)
RETURN child.node_id as id, child.name as name, labels(child) as labels,
       child.path as path, child.start_line as start_line,
       child.end_line as end_line, child_doc.content as description
"#;

/// Get remaining pending FUNCTION nodes (for cycle-breaking).
pub const REMAINING_PENDING_FUNCTIONS: &str = r#"
MATCH (root:NODE {node_id: $root_node_id, entityId: $entity_id, repoId: $repo_id})
MATCH (root)-[:CONTAINS|FUNCTION_DEFINITION|CLASS_DEFINITION|CALL*0..]->(n:FUNCTION)
WHERE (n.processing_status IS NULL OR n.processing_run_id <> $run_id) AND NOT n:DOCUMENTATION

OPTIONAL MATCH (n)-[:CONTAINS|FUNCTION_DEFINITION|CLASS_DEFINITION]->(hier_child:NODE)
WITH n, collect(DISTINCT hier_child) as hier_children

OPTIONAL MATCH (n)-[:CALLS]->(call_child:NODE)
WITH n, hier_children, collect(DISTINCT call_child) as call_children

OPTIONAL MATCH (hier_doc:DOCUMENTATION)-[:DESCRIBES]->(hier_child)
WHERE hier_child IN hier_children AND hier_child.processing_status = 'completed' AND hier_child.processing_run_id = $run_id
WITH n, call_children,
     collect(DISTINCT {id: hier_child.node_id, name: hier_child.name, labels: labels(hier_child), path: hier_child.path, description: hier_doc.content}) as hier_descriptions

OPTIONAL MATCH (call_doc:DOCUMENTATION)-[:DESCRIBES]->(call_child)
WHERE call_child IN call_children AND call_child.processing_status = 'completed' AND call_child.processing_run_id = $run_id
WITH n, hier_descriptions,
     collect(DISTINCT {id: call_child.node_id, name: call_child.name, labels: labels(call_child), path: call_child.path, description: call_doc.content}) as call_descriptions

WITH n, hier_descriptions, call_descriptions
LIMIT $batch_size
SET n.processing_status = 'in_progress', n.processing_run_id = $run_id
RETURN n.node_id as id, n.name as name, labels(n) as labels,
       n.path as path, n.start_line as start_line, n.end_line as end_line,
       coalesce(n.text, '') as content,
       hier_descriptions, call_descriptions
"#;

/// Get hierarchical parents of a node.
pub const HIERARCHICAL_PARENTS: &str = r#"
MATCH (child:NODE {node_id: $node_id, entityId: $entity_id, repoId: $repo_id})
OPTIONAL MATCH path = (parent:NODE)-[:CONTAINS|FUNCTION_DEFINITION|CLASS_DEFINITION*1..]->(child)
WITH child, collect(DISTINCT {node: parent, depth: length(path)}) as parents
OPTIONAL MATCH (child)-[:CLASS_DEFINITION]->(defined_class:CLASS)
WITH child, parents, collect(DISTINCT defined_class) as defined_classes
WITH [{node: child, depth: 0, sortKey: 0}] as child_entry,
    [class_node IN defined_classes | {node: class_node, depth: 1, sortKey: 1}] as class_entries,
    [parent_map IN parents | {node: parent_map.node, depth: parent_map.depth + 1, sortKey: 2}] as parent_entries
WITH child_entry + class_entries + parent_entries as nodes_info
UNWIND nodes_info as info
RETURN info.node.node_id as id, info.node.name as name, labels(info.node) as labels,
       info.node.path as path, info.node.start_line as start_line, info.node.end_line as end_line
ORDER BY info.sortKey ASC, info.depth ASC
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_query_targets_documentation_nodes() {
        assert!(CLEANUP_ORPHANED_DOCUMENTATION.contains("DOCUMENTATION"));
        assert!(CLEANUP_ORPHANED_DOCUMENTATION.contains("DETACH DELETE"));
    }

    #[test]
    fn batch_queries_use_run_id() {
        assert!(LEAF_NODES_BATCH.contains("$run_id"));
        assert!(PROCESSABLE_NODES_WITH_DESCRIPTIONS.contains("$run_id"));
        assert!(MARK_NODES_COMPLETED.contains("$run_id"));
    }

    #[test]
    fn check_pending_returns_count() {
        assert!(CHECK_PENDING_NODES.contains("pending_count"));
    }

    #[test]
    fn remaining_functions_handles_cycles() {
        // This query does NOT check if all children are completed
        assert!(REMAINING_PENDING_FUNCTIONS.contains("FUNCTION"));
        assert!(!REMAINING_PENDING_FUNCTIONS.contains("WHERE ALL(child"));
    }

    #[test]
    fn hierarchical_parents_orders_by_depth() {
        assert!(HIERARCHICAL_PARENTS.contains("ORDER BY"));
        assert!(HIERARCHICAL_PARENTS.contains("sortKey"));
    }
}
