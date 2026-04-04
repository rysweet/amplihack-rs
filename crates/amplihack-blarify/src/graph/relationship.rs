use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// RelationshipType
// ---------------------------------------------------------------------------

/// All relationship types in the code graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RelationshipType {
    // Code hierarchy
    Contains,
    FunctionDefinition,
    ClassDefinition,
    // Code references
    Imports,
    Calls,
    Inherits,
    Instantiates,
    Types,
    Assigns,
    Uses,
    // Diff
    Modified,
    Deleted,
    Added,
    // Workflow
    WorkflowStep,
    BelongsToWorkflow,
    BelongsToSpec,
    Describes,
    // Integration
    ModifiedBy,
    Affects,
    IntegrationSequence,
}

impl fmt::Display for RelationshipType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Contains => "CONTAINS",
            Self::FunctionDefinition => "FUNCTION_DEFINITION",
            Self::ClassDefinition => "CLASS_DEFINITION",
            Self::Imports => "IMPORTS",
            Self::Calls => "CALLS",
            Self::Inherits => "INHERITS",
            Self::Instantiates => "INSTANTIATES",
            Self::Types => "TYPES",
            Self::Assigns => "ASSIGNS",
            Self::Uses => "USES",
            Self::Modified => "MODIFIED",
            Self::Deleted => "DELETED",
            Self::Added => "ADDED",
            Self::WorkflowStep => "WORKFLOW_STEP",
            Self::BelongsToWorkflow => "BELONGS_TO_WORKFLOW",
            Self::BelongsToSpec => "BELONGS_TO_SPEC",
            Self::Describes => "DESCRIBES",
            Self::ModifiedBy => "MODIFIED_BY",
            Self::Affects => "AFFECTS",
            Self::IntegrationSequence => "INTEGRATION_SEQUENCE",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// Relationship
// ---------------------------------------------------------------------------

/// An edge between two nodes in the code graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub source_id: String,
    pub target_id: String,
    pub rel_type: RelationshipType,
    #[serde(default)]
    pub scope_text: String,
    pub start_line: Option<u32>,
    pub reference_character: Option<u32>,
    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,
}

impl Relationship {
    pub fn new(
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        rel_type: RelationshipType,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            target_id: target_id.into(),
            rel_type,
            scope_text: String::new(),
            start_line: None,
            reference_character: None,
            attributes: HashMap::new(),
        }
    }

    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope_text = scope.into();
        self
    }

    pub fn with_line(mut self, line: u32, character: u32) -> Self {
        self.start_line = Some(line);
        self.reference_character = Some(character);
        self
    }

    pub fn with_attribute(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// Serialize to the object format expected by graph DB exporters.
    pub fn as_object(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("sourceId".into(), serde_json::json!(self.source_id));
        map.insert("targetId".into(), serde_json::json!(self.target_id));
        map.insert("type".into(), serde_json::json!(self.rel_type.to_string()));
        map.insert("scopeText".into(), serde_json::json!(self.scope_text));

        if let Some(line) = self.start_line {
            map.insert("startLine".into(), serde_json::json!(line));
        }
        if let Some(ch) = self.reference_character {
            map.insert("referenceCharacter".into(), serde_json::json!(ch));
        }
        for (k, v) in &self.attributes {
            map.insert(k.clone(), v.clone());
        }

        serde_json::Value::Object(map)
    }
}

impl fmt::Display for Relationship {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "({}) -[{}]-> ({})",
            self.source_id, self.rel_type, self.target_id
        )
    }
}

// ---------------------------------------------------------------------------
// WorkflowStepRelationship
// ---------------------------------------------------------------------------

/// Extended relationship for workflow step edges with ordering metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepRelationship {
    #[serde(flatten)]
    pub base: Relationship,
    pub step_order: Option<u32>,
    pub depth: Option<u32>,
    pub call_line: Option<u32>,
    pub call_character: Option<u32>,
}

impl WorkflowStepRelationship {
    pub fn new(
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        step_order: Option<u32>,
        depth: Option<u32>,
    ) -> Self {
        Self {
            base: Relationship::new(source_id, target_id, RelationshipType::WorkflowStep),
            step_order,
            depth,
            call_line: None,
            call_character: None,
        }
    }

    pub fn as_object(&self) -> serde_json::Value {
        let mut obj = self.base.as_object();
        if let serde_json::Value::Object(ref mut map) = obj {
            if let Some(order) = self.step_order {
                map.insert("step_order".into(), serde_json::json!(order));
            }
            if let Some(d) = self.depth {
                map.insert("depth".into(), serde_json::json!(d));
            }
            if let Some(line) = self.call_line {
                map.insert("call_line".into(), serde_json::json!(line));
            }
            if let Some(ch) = self.call_character {
                map.insert("call_character".into(), serde_json::json!(ch));
            }
        }
        obj
    }
}

// ---------------------------------------------------------------------------
// ExternalRelationship
// ---------------------------------------------------------------------------

/// A relationship whose endpoints are identified by raw IDs
/// (not necessarily present in the current graph).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalRelationship {
    pub source_id: String,
    pub target_id: String,
    pub rel_type: RelationshipType,
}

impl ExternalRelationship {
    pub fn new(
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        rel_type: RelationshipType,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            target_id: target_id.into(),
            rel_type,
        }
    }

    pub fn as_object(&self) -> serde_json::Value {
        serde_json::json!({
            "sourceId": self.source_id,
            "targetId": self.target_id,
            "type": self.rel_type.to_string(),
            "scopeText": "",
        })
    }
}

// ---------------------------------------------------------------------------
// ExternalRelationshipStore
// ---------------------------------------------------------------------------

/// Accumulator for external relationships that reference nodes outside the
/// current graph snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalRelationshipStore {
    relationships: Vec<ExternalRelationship>,
}

impl ExternalRelationshipStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, rel: ExternalRelationship) {
        self.relationships.push(rel);
    }

    pub fn create_and_add(
        &mut self,
        source_id: impl Into<String>,
        target_id: impl Into<String>,
        rel_type: RelationshipType,
    ) {
        self.add(ExternalRelationship::new(source_id, target_id, rel_type));
    }

    pub fn as_objects(&self) -> Vec<serde_json::Value> {
        self.relationships.iter().map(|r| r.as_object()).collect()
    }

    pub fn len(&self) -> usize {
        self.relationships.len()
    }

    pub fn is_empty(&self) -> bool {
        self.relationships.is_empty()
    }
}

// ---------------------------------------------------------------------------
// RelationshipCreator — high-level relationship builders
// ---------------------------------------------------------------------------

/// Factory functions for creating typed relationships between nodes.
pub struct RelationshipCreator;

impl RelationshipCreator {
    /// Build a CONTAINS relationship from a folder to a child.
    pub fn create_contains_relationship(
        folder_hashed_id: &str,
        child_hashed_id: &str,
    ) -> Relationship {
        Relationship::new(
            folder_hashed_id,
            child_hashed_id,
            RelationshipType::Contains,
        )
    }

    /// Build a DEFINES relationship (FUNCTION_DEFINITION or CLASS_DEFINITION).
    pub fn create_defines_relationship(
        parent_hashed_id: &str,
        child_hashed_id: &str,
        child_label: super::node::NodeLabel,
    ) -> Relationship {
        let rel_type = match child_label {
            super::node::NodeLabel::Function | super::node::NodeLabel::Method => {
                RelationshipType::FunctionDefinition
            }
            super::node::NodeLabel::Class => RelationshipType::ClassDefinition,
            _ => RelationshipType::FunctionDefinition,
        };
        Relationship::new(parent_hashed_id, child_hashed_id, rel_type)
    }

    /// Build a DESCRIBES relationship from documentation to its source.
    pub fn create_describes_relationship(doc_hashed_id: &str, source_id: &str) -> Relationship {
        Relationship::new(doc_hashed_id, source_id, RelationshipType::Describes)
            .with_scope("semantic_documentation")
    }

    /// Build a BELONGS_TO_WORKFLOW relationship.
    pub fn create_belongs_to_workflow(
        node_hashed_id: &str,
        workflow_hashed_id: &str,
    ) -> Relationship {
        Relationship::new(
            node_hashed_id,
            workflow_hashed_id,
            RelationshipType::BelongsToWorkflow,
        )
    }

    /// Build a WORKFLOW_STEP relationship with ordering.
    pub fn create_workflow_step(
        current_id: &str,
        next_id: &str,
        step_order: Option<u32>,
        depth: Option<u32>,
    ) -> WorkflowStepRelationship {
        WorkflowStepRelationship::new(current_id, next_id, step_order, depth)
    }

    /// Build INTEGRATION_SEQUENCE relationships from a PR node to commit nodes.
    pub fn create_integration_sequence(
        pr_hashed_id: &str,
        commit_hashed_ids: &[String],
    ) -> Vec<Relationship> {
        commit_hashed_ids
            .iter()
            .enumerate()
            .map(|(i, cid)| {
                Relationship::new(
                    pr_hashed_id,
                    cid.as_str(),
                    RelationshipType::IntegrationSequence,
                )
                .with_attribute("order", serde_json::json!(i))
            })
            .collect()
    }

    /// Build a MODIFIED_BY relationship from a code node to a commit node.
    pub fn create_modified_by(
        code_node_id: &str,
        commit_hashed_id: &str,
        attrs: HashMap<String, serde_json::Value>,
    ) -> Relationship {
        let mut rel =
            Relationship::new(code_node_id, commit_hashed_id, RelationshipType::ModifiedBy);
        rel.attributes = attrs;
        rel
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relationship_type_display() {
        assert_eq!(RelationshipType::Contains.to_string(), "CONTAINS");
        assert_eq!(
            RelationshipType::FunctionDefinition.to_string(),
            "FUNCTION_DEFINITION"
        );
        assert_eq!(
            RelationshipType::IntegrationSequence.to_string(),
            "INTEGRATION_SEQUENCE"
        );
    }

    #[test]
    fn relationship_type_serde_round_trip() {
        let rt = RelationshipType::Calls;
        let json = serde_json::to_string(&rt).unwrap();
        assert_eq!(json, "\"CALLS\"");
        let back: RelationshipType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rt);
    }

    #[test]
    fn relationship_as_object() {
        let rel = Relationship::new("src-1", "tgt-2", RelationshipType::Imports)
            .with_scope("module")
            .with_line(10, 5);
        let obj = rel.as_object();
        assert_eq!(obj["sourceId"], "src-1");
        assert_eq!(obj["targetId"], "tgt-2");
        assert_eq!(obj["type"], "IMPORTS");
        assert_eq!(obj["scopeText"], "module");
        assert_eq!(obj["startLine"], 10);
        assert_eq!(obj["referenceCharacter"], 5);
    }

    #[test]
    fn relationship_display() {
        let rel = Relationship::new("a", "b", RelationshipType::Calls);
        assert_eq!(rel.to_string(), "(a) -[CALLS]-> (b)");
    }

    #[test]
    fn external_relationship_as_object() {
        let ext = ExternalRelationship::new("x", "y", RelationshipType::Inherits);
        let obj = ext.as_object();
        assert_eq!(obj["sourceId"], "x");
        assert_eq!(obj["targetId"], "y");
        assert_eq!(obj["type"], "INHERITS");
        assert_eq!(obj["scopeText"], "");
    }

    #[test]
    fn external_store_add_and_query() {
        let mut store = ExternalRelationshipStore::new();
        assert!(store.is_empty());
        store.create_and_add("a", "b", RelationshipType::Uses);
        store.create_and_add("c", "d", RelationshipType::Assigns);
        assert_eq!(store.len(), 2);
        let objs = store.as_objects();
        assert_eq!(objs.len(), 2);
    }

    #[test]
    fn workflow_step_relationship_as_object() {
        let ws = WorkflowStepRelationship::new("s1", "s2", Some(1), Some(0));
        let obj = ws.as_object();
        assert_eq!(obj["type"], "WORKFLOW_STEP");
        assert_eq!(obj["step_order"], 1);
        assert_eq!(obj["depth"], 0);
    }

    #[test]
    fn relationship_creator_contains() {
        let rel = RelationshipCreator::create_contains_relationship("folder-1", "file-1");
        assert_eq!(rel.rel_type, RelationshipType::Contains);
        assert_eq!(rel.source_id, "folder-1");
    }

    #[test]
    fn relationship_creator_defines() {
        let rel = RelationshipCreator::create_defines_relationship(
            "file-1",
            "func-1",
            super::super::node::NodeLabel::Function,
        );
        assert_eq!(rel.rel_type, RelationshipType::FunctionDefinition);

        let rel2 = RelationshipCreator::create_defines_relationship(
            "file-1",
            "cls-1",
            super::super::node::NodeLabel::Class,
        );
        assert_eq!(rel2.rel_type, RelationshipType::ClassDefinition);
    }

    #[test]
    fn relationship_creator_integration_sequence() {
        let rels = RelationshipCreator::create_integration_sequence(
            "pr-1",
            &["c1".into(), "c2".into(), "c3".into()],
        );
        assert_eq!(rels.len(), 3);
        assert_eq!(rels[0].attributes["order"], 0);
        assert_eq!(rels[2].attributes["order"], 2);
    }

    #[test]
    fn relationship_with_attributes() {
        let rel = Relationship::new("a", "b", RelationshipType::ModifiedBy)
            .with_attribute("lines_added", serde_json::json!(42))
            .with_attribute("change_type", serde_json::json!("modified"));
        let obj = rel.as_object();
        assert_eq!(obj["lines_added"], 42);
        assert_eq!(obj["change_type"], "modified");
    }
}
