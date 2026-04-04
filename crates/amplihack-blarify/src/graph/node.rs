use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::relationship::{Relationship, RelationshipType};

// ---------------------------------------------------------------------------
// NodeLabel
// ---------------------------------------------------------------------------

/// Labels that categorize graph nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NodeLabel {
    Folder,
    File,
    Function,
    Class,
    Method,
    Module,
    Deleted,
    Documentation,
    Workflow,
    Integration,
}

impl fmt::Display for NodeLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Folder => "FOLDER",
            Self::File => "FILE",
            Self::Function => "FUNCTION",
            Self::Class => "CLASS",
            Self::Method => "METHOD",
            Self::Module => "MODULE",
            Self::Deleted => "DELETED",
            Self::Documentation => "DOCUMENTATION",
            Self::Workflow => "WORKFLOW",
            Self::Integration => "INTEGRATION",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// GraphEnvironment
// ---------------------------------------------------------------------------

/// Environment context attached to every node for namespacing identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEnvironment {
    pub environment: String,
    pub diff_identifier: String,
    pub root_path: String,
}

impl GraphEnvironment {
    pub fn new(
        environment: impl Into<String>,
        diff_identifier: impl Into<String>,
        root_path: impl Into<String>,
    ) -> Self {
        Self {
            environment: environment.into(),
            diff_identifier: diff_identifier.into(),
            root_path: root_path.into(),
        }
    }
}

impl fmt::Display for GraphEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}/{}", self.environment, self.diff_identifier)
    }
}

// ---------------------------------------------------------------------------
// NestingStats
// ---------------------------------------------------------------------------

/// Code complexity metrics for definition nodes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NestingStats {
    pub max_indentation: f64,
    pub min_indentation: f64,
    pub average_indentation: f64,
    pub sd: f64,
}

// ---------------------------------------------------------------------------
// Range helpers (for definition / node ranges)
// ---------------------------------------------------------------------------

/// A (line, character) position in source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// A start–end range in source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    pub start: Position,
    pub end: Position,
}

impl SourceRange {
    pub fn new(start_line: u32, start_char: u32, end_line: u32, end_char: u32) -> Self {
        Self {
            start: Position {
                line: start_line,
                character: start_char,
            },
            end: Position {
                line: end_line,
                character: end_char,
            },
        }
    }

    pub fn zero() -> Self {
        Self::new(0, 0, 0, 0)
    }
}

// ---------------------------------------------------------------------------
// Node – enum-based approach
// ---------------------------------------------------------------------------

/// The core node type. We use an enum rather than trait objects for ergonomics,
/// pattern-matching, and `Serialize`/`Deserialize` support.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Node {
    Folder(FolderNode),
    File(FileNode),
    Function(FunctionNode),
    Class(ClassNode),
    Deleted(DeletedNode),
    Documentation(DocumentationNode),
    Workflow(WorkflowNode),
    Integration(IntegrationNode),
}

impl Node {
    // -- Accessors that delegate to inner types --

    pub fn label(&self) -> NodeLabel {
        match self {
            Self::Folder(_) => NodeLabel::Folder,
            Self::File(_) => NodeLabel::File,
            Self::Function(_) => NodeLabel::Function,
            Self::Class(_) => NodeLabel::Class,
            Self::Deleted(_) => NodeLabel::Deleted,
            Self::Documentation(_) => NodeLabel::Documentation,
            Self::Workflow(_) => NodeLabel::Workflow,
            Self::Integration(_) => NodeLabel::Integration,
        }
    }

    pub fn path(&self) -> &str {
        match self {
            Self::Folder(n) => &n.base.path,
            Self::File(n) => &n.def.base.path,
            Self::Function(n) => &n.def.base.path,
            Self::Class(n) => &n.def.base.path,
            Self::Deleted(n) => &n.base.path,
            Self::Documentation(n) => &n.base.path,
            Self::Workflow(n) => &n.base.path,
            Self::Integration(n) => &n.base.path,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Folder(n) => &n.base.name,
            Self::File(n) => &n.def.base.name,
            Self::Function(n) => &n.def.base.name,
            Self::Class(n) => &n.def.base.name,
            Self::Deleted(n) => &n.base.name,
            Self::Documentation(n) => &n.base.name,
            Self::Workflow(n) => &n.base.name,
            Self::Integration(n) => &n.base.name,
        }
    }

    pub fn level(&self) -> u32 {
        match self {
            Self::Folder(n) => n.base.level,
            Self::File(n) => n.def.base.level,
            Self::Function(n) => n.def.base.level,
            Self::Class(n) => n.def.base.level,
            Self::Deleted(n) => n.base.level,
            Self::Documentation(n) => n.base.level,
            Self::Workflow(n) => n.base.level,
            Self::Integration(n) => n.base.level,
        }
    }

    pub fn layer(&self) -> &str {
        match self {
            Self::Folder(n) => &n.base.layer,
            Self::File(n) => &n.def.base.layer,
            Self::Function(n) => &n.def.base.layer,
            Self::Class(n) => &n.def.base.layer,
            Self::Deleted(n) => &n.base.layer,
            Self::Documentation(n) => &n.base.layer,
            Self::Workflow(n) => &n.base.layer,
            Self::Integration(n) => &n.base.layer,
        }
    }

    fn base(&self) -> &NodeBase {
        match self {
            Self::Folder(n) => &n.base,
            Self::File(n) => &n.def.base,
            Self::Function(n) => &n.def.base,
            Self::Class(n) => &n.def.base,
            Self::Deleted(n) => &n.base,
            Self::Documentation(n) => &n.base,
            Self::Workflow(n) => &n.base,
            Self::Integration(n) => &n.base,
        }
    }

    /// The fully-qualified identifier including graph environment prefix.
    pub fn id(&self) -> String {
        let env_prefix = self
            .base()
            .graph_environment
            .as_ref()
            .map(|e| e.to_string())
            .unwrap_or_default();
        format!("{env_prefix}{}", self.node_repr_chain())
    }

    /// SHA-256 hash of the full identifier (hex string).
    pub fn hashed_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.id().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// The path portion without `file://` prefix.
    pub fn pure_path(&self) -> &str {
        self.path().strip_prefix("file://").unwrap_or(self.path())
    }

    /// File extension extracted from the path, including the dot.
    pub fn extension(&self) -> &str {
        Path::new(self.pure_path())
            .extension()
            .and_then(|s| s.to_str())
            .map(|_ext| {
                // Return the dot + extension from the original path
                let path = self.pure_path();
                let dot_pos = path.rfind('.').unwrap_or(path.len());
                &path[dot_pos..]
            })
            .unwrap_or("")
    }

    /// Build the full identifier chain by walking parent repr segments.
    fn node_repr_chain(&self) -> String {
        let repr = self.node_repr_for_identifier();
        match self {
            Self::Folder(n) => {
                if let Some(ref parent_repr) = n.parent_identifier {
                    format!("{parent_repr}{repr}")
                } else {
                    repr
                }
            }
            Self::File(n) => {
                if let Some(ref parent_repr) = n.def.parent_identifier {
                    format!("{parent_repr}{repr}")
                } else {
                    repr
                }
            }
            Self::Function(n) => {
                if let Some(ref parent_repr) = n.def.parent_identifier {
                    format!("{parent_repr}{repr}")
                } else {
                    repr
                }
            }
            Self::Class(n) => {
                if let Some(ref parent_repr) = n.def.parent_identifier {
                    format!("{parent_repr}{repr}")
                } else {
                    repr
                }
            }
            Self::Deleted(n) => {
                if let Some(ref parent_repr) = n.parent_identifier {
                    format!("{parent_repr}{repr}")
                } else {
                    repr
                }
            }
            Self::Documentation(n) => {
                if let Some(ref parent_repr) = n.parent_identifier {
                    format!("{parent_repr}{repr}")
                } else {
                    repr
                }
            }
            Self::Workflow(n) => {
                if let Some(ref parent_repr) = n.parent_identifier {
                    format!("{parent_repr}{repr}")
                } else {
                    repr
                }
            }
            Self::Integration(n) => {
                if let Some(ref parent_repr) = n.parent_identifier {
                    format!("{parent_repr}{repr}")
                } else {
                    repr
                }
            }
        }
    }

    /// Per-type identifier fragment used in the recursive ID chain.
    pub fn node_repr_for_identifier(&self) -> String {
        match self {
            Self::Folder(n) => format!("/{}", n.base.name),
            Self::File(n) => format!("/{}", n.def.base.name),
            Self::Function(n) => format!(".{}", n.def.base.name),
            Self::Class(n) => format!("#{}", n.def.base.name),
            Self::Deleted(n) => format!("/DELETED-{}", n.base.name),
            Self::Documentation(n) => format!("{}@info", n.source_id),
            Self::Workflow(n) => format!("{}@workflow", n.source_name),
            Self::Integration(n) => {
                format!("{}_{}_{}", n.source, n.source_type, n.external_id)
            }
        }
    }

    /// Serialize the node to a property map suitable for graph DB export.
    pub fn as_object(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("label".into(), serde_json::json!(self.label().to_string()));
        map.insert("path".into(), serde_json::json!(self.path()));
        map.insert("node_id".into(), serde_json::json!(self.id()));
        map.insert("node_path".into(), serde_json::json!(self.path()));
        map.insert("name".into(), serde_json::json!(self.name()));
        map.insert("level".into(), serde_json::json!(self.level()));
        map.insert("hashed_id".into(), serde_json::json!(self.hashed_id()));
        map.insert("layer".into(), serde_json::json!(self.layer()));

        let mut extra_labels = Vec::<String>::new();

        match self {
            Self::Folder(_) => {}
            Self::File(n) => {
                map.insert("text".into(), serde_json::json!(n.def.code_text));
                extra_labels.extend(n.def.extra_labels.iter().cloned());
                Self::insert_nesting_stats(&mut map, &n.def.stats);
            }
            Self::Function(n) => {
                if let (Some(start), Some(end)) = (
                    n.def.node_range.as_ref().map(|r| r.start.line),
                    n.def.node_range.as_ref().map(|r| r.end.line),
                ) {
                    map.insert("start_line".into(), serde_json::json!(start));
                    map.insert("end_line".into(), serde_json::json!(end));
                }
                map.insert("text".into(), serde_json::json!(n.def.code_text));
                map.insert(
                    "stats_parameter_count".into(),
                    serde_json::json!(n.parameter_count),
                );
                extra_labels.extend(n.def.extra_labels.iter().cloned());
                Self::insert_nesting_stats(&mut map, &n.def.stats);
            }
            Self::Class(n) => {
                if let (Some(start), Some(end)) = (
                    n.def.node_range.as_ref().map(|r| r.start.line),
                    n.def.node_range.as_ref().map(|r| r.end.line),
                ) {
                    map.insert("start_line".into(), serde_json::json!(start));
                    map.insert("end_line".into(), serde_json::json!(end));
                }
                map.insert("text".into(), serde_json::json!(n.def.code_text));
                map.insert(
                    "stats_methods_defined".into(),
                    serde_json::json!(n.methods_defined),
                );
                extra_labels.extend(n.def.extra_labels.iter().cloned());
                Self::insert_nesting_stats(&mut map, &n.def.stats);
            }
            Self::Deleted(_) => {}
            Self::Documentation(n) => {
                map.insert("content".into(), serde_json::json!(n.content));
                map.insert("info_type".into(), serde_json::json!(n.info_type));
                map.insert("source_type".into(), serde_json::json!(n.source_type));
                map.insert("source_path".into(), serde_json::json!(n.source_path));
                map.insert("source_node_id".into(), serde_json::json!(n.source_id));
                if let Some(ref ec) = n.enhanced_content {
                    map.insert("enhanced_content".into(), serde_json::json!(ec));
                }
            }
            Self::Workflow(n) => {
                map.insert("title".into(), serde_json::json!(n.title));
                map.insert("content".into(), serde_json::json!(n.content));
                map.insert("entry_point_id".into(), serde_json::json!(n.entry_point_id));
                map.insert(
                    "entry_point_name".into(),
                    serde_json::json!(n.entry_point_name),
                );
                map.insert("end_point_id".into(), serde_json::json!(n.end_point_id));
                map.insert("end_point_name".into(), serde_json::json!(n.end_point_name));
                map.insert("steps".into(), serde_json::json!(n.workflow_nodes.len()));
                if let Some(ref ec) = n.enhanced_content {
                    map.insert("enhanced_content".into(), serde_json::json!(ec));
                }
            }
            Self::Integration(n) => {
                map.insert("source".into(), serde_json::json!(n.source));
                map.insert("source_type".into(), serde_json::json!(n.source_type));
                map.insert("external_id".into(), serde_json::json!(n.external_id));
                map.insert("title".into(), serde_json::json!(n.title));
                map.insert("content".into(), serde_json::json!(n.content));
                if let Some(ref ts) = n.timestamp {
                    map.insert("timestamp".into(), serde_json::json!(ts));
                }
                if let Some(ref author) = n.author {
                    map.insert("author".into(), serde_json::json!(author));
                }
                if let Some(ref url) = n.url {
                    map.insert("url".into(), serde_json::json!(url));
                }
            }
        }

        if !extra_labels.is_empty() {
            map.insert("extra_labels".into(), serde_json::json!(extra_labels));
        }

        serde_json::Value::Object(map)
    }

    fn insert_nesting_stats(
        map: &mut serde_json::Map<String, serde_json::Value>,
        stats: &NestingStats,
    ) {
        map.insert(
            "stats_max_indentation".into(),
            serde_json::json!(stats.max_indentation),
        );
        map.insert(
            "stats_min_indentation".into(),
            serde_json::json!(stats.min_indentation),
        );
        map.insert(
            "stats_average_indentation".into(),
            serde_json::json!(stats.average_indentation),
        );
        map.insert("stats_sd_indentation".into(), serde_json::json!(stats.sd));
    }

    /// Get the internal list of child definition IDs this node defines.
    pub fn defined_children_ids(&self) -> Vec<String> {
        match self {
            Self::File(n) => n.def.defines.clone(),
            Self::Function(n) => n.def.defines.clone(),
            Self::Class(n) => n.def.defines.clone(),
            _ => vec![],
        }
    }

    /// Get the internal list of child node IDs this folder contains.
    pub fn contained_children_ids(&self) -> Vec<String> {
        match self {
            Self::Folder(n) => n.contains.clone(),
            _ => vec![],
        }
    }

    /// Build relationships implied by this node's structure.
    pub fn get_relationships(&self) -> Vec<Relationship> {
        let mut rels = Vec::new();
        let self_id = self.hashed_id();

        match self {
            Self::Folder(n) => {
                for child_id in &n.contains {
                    rels.push(Relationship::new(
                        self_id.clone(),
                        child_id.clone(),
                        RelationshipType::Contains,
                    ));
                }
            }
            Self::File(n) => {
                for child_id in &n.def.defines {
                    rels.push(Relationship::new(
                        self_id.clone(),
                        child_id.clone(),
                        relationship_type_for_define(NodeLabel::File),
                    ));
                }
            }
            Self::Function(n) => {
                for child_id in &n.def.defines {
                    rels.push(Relationship::new(
                        self_id.clone(),
                        child_id.clone(),
                        relationship_type_for_define(NodeLabel::Function),
                    ));
                }
            }
            Self::Class(n) => {
                for child_id in &n.def.defines {
                    rels.push(Relationship::new(
                        self_id.clone(),
                        child_id.clone(),
                        relationship_type_for_define(NodeLabel::Class),
                    ));
                }
            }
            _ => {}
        }

        rels
    }
}

/// Map a parent node label to the appropriate define-relationship type.
fn relationship_type_for_define(parent_label: NodeLabel) -> RelationshipType {
    match parent_label {
        NodeLabel::Function | NodeLabel::Method => RelationshipType::FunctionDefinition,
        NodeLabel::Class => RelationshipType::ClassDefinition,
        _ => RelationshipType::FunctionDefinition,
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id())
    }
}

// ---------------------------------------------------------------------------
// NodeBase — shared fields for every node kind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeBase {
    pub path: String,
    pub name: String,
    pub level: u32,
    pub layer: String,
    pub graph_environment: Option<GraphEnvironment>,
}

impl NodeBase {
    pub fn new(
        path: impl Into<String>,
        name: impl Into<String>,
        level: u32,
        layer: impl Into<String>,
        graph_environment: Option<GraphEnvironment>,
    ) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            level,
            layer: layer.into(),
            graph_environment,
        }
    }
}

// ---------------------------------------------------------------------------
// DefinitionBase — shared fields for definition nodes (File, Function, Class)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefinitionBase {
    #[serde(flatten)]
    pub base: NodeBase,
    /// Hashed IDs of nodes this definition defines (children).
    pub defines: Vec<String>,
    pub definition_range: Option<SourceRange>,
    pub node_range: Option<SourceRange>,
    pub code_text: String,
    pub extra_labels: Vec<String>,
    pub extra_attributes: HashMap<String, String>,
    pub stats: NestingStats,
    /// Parent node's identifier chain (for recursive ID building).
    pub parent_identifier: Option<String>,
}

impl DefinitionBase {
    pub fn new(
        base: NodeBase,
        definition_range: Option<SourceRange>,
        node_range: Option<SourceRange>,
        code_text: impl Into<String>,
        parent_identifier: Option<String>,
    ) -> Self {
        Self {
            base,
            defines: Vec::new(),
            definition_range,
            node_range,
            code_text: code_text.into(),
            extra_labels: Vec::new(),
            extra_attributes: HashMap::new(),
            stats: NestingStats::default(),
            parent_identifier,
        }
    }

    /// Start and end line from node_range.
    pub fn get_start_and_end_line(&self) -> Option<(u32, u32)> {
        self.node_range.map(|r| (r.start.line, r.end.line))
    }

    /// Check if a reference falls within this node's line range.
    pub fn contains_line(&self, line: u32) -> bool {
        self.node_range
            .is_some_and(|r| line >= r.start.line && line <= r.end.line)
    }
}

// ---------------------------------------------------------------------------
// Concrete node types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderNode {
    #[serde(flatten)]
    pub base: NodeBase,
    /// Hashed IDs of directly-contained child nodes.
    pub contains: Vec<String>,
    pub parent_identifier: Option<String>,
}

impl FolderNode {
    pub fn new(
        path: impl Into<String>,
        name: impl Into<String>,
        level: u32,
        graph_environment: Option<GraphEnvironment>,
        parent_identifier: Option<String>,
    ) -> Self {
        let mut p: String = path.into();
        if p.ends_with('/') && p.len() > 1 {
            p.pop();
        }
        Self {
            base: NodeBase::new(p, name, level, "code", graph_environment),
            contains: Vec::new(),
            parent_identifier,
        }
    }

    pub fn add_child(&mut self, child_hashed_id: String) {
        self.contains.push(child_hashed_id);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    #[serde(flatten)]
    pub def: DefinitionBase,
}

impl FileNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: impl Into<String>,
        name: impl Into<String>,
        level: u32,
        definition_range: Option<SourceRange>,
        node_range: Option<SourceRange>,
        code_text: impl Into<String>,
        graph_environment: Option<GraphEnvironment>,
        parent_identifier: Option<String>,
    ) -> Self {
        let base = NodeBase::new(path, name, level, "code", graph_environment);
        Self {
            def: DefinitionBase::new(
                base,
                definition_range,
                node_range,
                code_text,
                parent_identifier,
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionNode {
    #[serde(flatten)]
    pub def: DefinitionBase,
    pub parameter_count: u32,
}

impl FunctionNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: impl Into<String>,
        name: impl Into<String>,
        level: u32,
        definition_range: Option<SourceRange>,
        node_range: Option<SourceRange>,
        code_text: impl Into<String>,
        graph_environment: Option<GraphEnvironment>,
        parent_identifier: Option<String>,
        parameter_count: u32,
    ) -> Self {
        let base = NodeBase::new(path, name, level, "code", graph_environment);
        Self {
            def: DefinitionBase::new(
                base,
                definition_range,
                node_range,
                code_text,
                parent_identifier,
            ),
            parameter_count,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassNode {
    #[serde(flatten)]
    pub def: DefinitionBase,
    pub methods_defined: u32,
}

impl ClassNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: impl Into<String>,
        name: impl Into<String>,
        level: u32,
        definition_range: Option<SourceRange>,
        node_range: Option<SourceRange>,
        code_text: impl Into<String>,
        graph_environment: Option<GraphEnvironment>,
        parent_identifier: Option<String>,
        methods_defined: u32,
    ) -> Self {
        let base = NodeBase::new(path, name, level, "code", graph_environment);
        Self {
            def: DefinitionBase::new(
                base,
                definition_range,
                node_range,
                code_text,
                parent_identifier,
            ),
            methods_defined,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedNode {
    #[serde(flatten)]
    pub base: NodeBase,
    pub parent_identifier: Option<String>,
}

impl DeletedNode {
    pub fn new(path: impl Into<String>, graph_environment: Option<GraphEnvironment>) -> Self {
        Self {
            base: NodeBase::new(path, "DELETED", 0, "code", graph_environment),
            parent_identifier: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentationNode {
    #[serde(flatten)]
    pub base: NodeBase,
    pub content: String,
    pub info_type: String,
    pub source_type: String,
    pub source_path: String,
    pub source_id: String,
    pub source_name: String,
    pub source_labels: Vec<String>,
    pub examples: Option<Vec<String>>,
    pub enhanced_content: Option<String>,
    pub children_count: Option<u32>,
    pub metadata: Option<HashMap<String, String>>,
    pub parent_identifier: Option<String>,
}

impl DocumentationNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        content: impl Into<String>,
        info_type: impl Into<String>,
        source_type: impl Into<String>,
        source_path: impl Into<String>,
        source_name: impl Into<String>,
        source_id: impl Into<String>,
        level: u32,
        graph_environment: Option<GraphEnvironment>,
        parent_identifier: Option<String>,
    ) -> Self {
        let source_id_val: String = source_id.into();
        let source_path_val: String = source_path.into();
        let name = format!("{source_id_val}@info");
        Self {
            base: NodeBase::new(
                source_path_val.clone(),
                name,
                level,
                "documentation",
                graph_environment,
            ),
            content: content.into(),
            info_type: info_type.into(),
            source_type: source_type.into(),
            source_path: source_path_val,
            source_id: source_id_val,
            source_name: source_name.into(),
            source_labels: Vec::new(),
            examples: None,
            enhanced_content: None,
            children_count: None,
            metadata: None,
            parent_identifier,
        }
    }

    /// Mark this documentation node as participating in a cycle.
    pub fn mark_cycle(&mut self) {
        self.content
            .push_str("\n[Note: Circular dependency detected in documentation hierarchy]");
        let mut meta = self.metadata.take().unwrap_or_default();
        meta.insert("has_cycle".into(), "true".into());
        self.metadata = Some(meta);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    #[serde(flatten)]
    pub base: NodeBase,
    pub title: String,
    pub content: String,
    pub entry_point_id: String,
    pub entry_point_name: String,
    pub entry_point_path: String,
    pub end_point_id: String,
    pub end_point_name: String,
    pub end_point_path: String,
    pub workflow_nodes: Vec<String>,
    pub source_type: String,
    pub source_path: String,
    pub source_name: String,
    pub source_labels: Vec<String>,
    pub enhanced_content: Option<String>,
    pub parent_identifier: Option<String>,
}

impl WorkflowNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        title: impl Into<String>,
        content: impl Into<String>,
        entry_point_id: impl Into<String>,
        entry_point_name: impl Into<String>,
        entry_point_path: impl Into<String>,
        end_point_id: impl Into<String>,
        end_point_name: impl Into<String>,
        end_point_path: impl Into<String>,
        workflow_nodes: Vec<String>,
        level: u32,
        graph_environment: Option<GraphEnvironment>,
        parent_identifier: Option<String>,
    ) -> Self {
        let ep_id: String = entry_point_id.into();
        let ep_name: String = entry_point_name.into();
        let ed_id: String = end_point_id.into();
        let source_name = format!("workflow_{ep_id}_{ed_id}");
        let name = format!("{source_name}@workflow");
        let source_path = format!("file:///workflows/{ep_name}_to_workflow");

        Self {
            base: NodeBase::new(
                source_path.clone(),
                name,
                level,
                "workflows",
                graph_environment,
            ),
            title: title.into(),
            content: content.into(),
            entry_point_id: ep_id,
            entry_point_name: ep_name,
            entry_point_path: entry_point_path.into(),
            end_point_id: ed_id,
            end_point_name: end_point_name.into(),
            end_point_path: end_point_path.into(),
            workflow_nodes,
            source_type: "workflow_analysis".into(),
            source_path,
            source_name,
            source_labels: vec!["WORKFLOW".into()],
            enhanced_content: None,
            parent_identifier,
        }
    }

    pub fn step_count(&self) -> usize {
        self.workflow_nodes.len()
    }

    pub fn has_valid_endpoints(&self) -> bool {
        !self.entry_point_id.is_empty() && !self.end_point_id.is_empty()
    }

    pub fn content_preview(&self, max_length: usize) -> &str {
        if self.content.len() <= max_length {
            &self.content
        } else {
            &self.content[..max_length]
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationNode {
    #[serde(flatten)]
    pub base: NodeBase,
    pub source: String,
    pub source_type: String,
    pub external_id: String,
    pub title: String,
    pub content: String,
    pub timestamp: Option<String>,
    pub author: Option<String>,
    pub url: Option<String>,
    pub metadata: HashMap<String, String>,
    pub parent_identifier: Option<String>,
}

impl IntegrationNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source: impl Into<String>,
        source_type: impl Into<String>,
        external_id: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
        graph_environment: Option<GraphEnvironment>,
    ) -> Self {
        let src: String = source.into();
        let st: String = source_type.into();
        let eid: String = external_id.into();
        let t: String = title.into();
        let path = format!("integration://{src}/{st}/{eid}");
        Self {
            base: NodeBase::new(path, t.clone(), 0, "integrations", graph_environment),
            source: src,
            source_type: st,
            external_id: eid,
            title: t,
            content: content.into(),
            timestamp: None,
            author: None,
            url: None,
            metadata: HashMap::new(),
            parent_identifier: None,
        }
    }

    /// Create a commit-type integration node.
    pub fn commit(
        external_id: impl Into<String>,
        title: impl Into<String>,
        diff_text: impl Into<String>,
        graph_environment: Option<GraphEnvironment>,
    ) -> Self {
        Self::new(
            "github",
            "commit",
            external_id,
            title,
            diff_text,
            graph_environment,
        )
    }

    /// Create a pull-request-type integration node.
    pub fn pull_request(
        external_id: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
        graph_environment: Option<GraphEnvironment>,
    ) -> Self {
        Self::new(
            "github",
            "pull_request",
            external_id,
            title,
            description,
            graph_environment,
        )
    }
}

// ---------------------------------------------------------------------------
// IdCalculator
// ---------------------------------------------------------------------------

/// Utilities for computing deterministic node identifiers.
pub struct IdCalculator;

impl IdCalculator {
    /// Compute a file-level ID: `/{environment}/{pr_id}/{relative_path}`.
    pub fn generate_file_id(environment: &str, pr_id: &str, path: &str) -> String {
        let clean = path.strip_prefix('/').unwrap_or(path);
        format!("/{environment}/{pr_id}/{clean}")
    }

    /// SHA-256 hash of a file ID.
    pub fn generate_hashed_file_id(environment: &str, pr_id: &str, path: &str) -> String {
        let id = Self::generate_file_id(environment, pr_id, path);
        Self::hash_id(&id)
    }

    /// SHA-256 hex digest of an arbitrary string.
    pub fn hash_id(id: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(id.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_env() -> GraphEnvironment {
        GraphEnvironment::new("test", "main", "/repo")
    }

    #[test]
    fn node_label_display() {
        assert_eq!(NodeLabel::File.to_string(), "FILE");
        assert_eq!(NodeLabel::Folder.to_string(), "FOLDER");
        assert_eq!(NodeLabel::Function.to_string(), "FUNCTION");
        assert_eq!(NodeLabel::Integration.to_string(), "INTEGRATION");
    }

    #[test]
    fn node_label_serde_round_trip() {
        let label = NodeLabel::Class;
        let json = serde_json::to_string(&label).unwrap();
        assert_eq!(json, "\"CLASS\"");
        let back: NodeLabel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, label);
    }

    #[test]
    fn graph_environment_display() {
        let env = test_env();
        assert_eq!(env.to_string(), "/test/main");
    }

    #[test]
    fn folder_node_trailing_slash_stripped() {
        let f = FolderNode::new("file:///repo/src/", "src", 1, None, None);
        assert_eq!(f.base.path, "file:///repo/src");
    }

    #[test]
    fn folder_node_id_and_repr() {
        let env = test_env();
        let n = Node::Folder(FolderNode::new(
            "file:///repo/src",
            "src",
            1,
            Some(env),
            None,
        ));
        assert_eq!(n.node_repr_for_identifier(), "/src");
        assert!(n.id().starts_with("/test/main"));
    }

    #[test]
    fn file_node_as_object_contains_text() {
        let n = Node::File(FileNode::new(
            "file:///repo/main.py",
            "main.py",
            1,
            None,
            None,
            "print('hello')",
            None,
            None,
        ));
        let obj = n.as_object();
        assert_eq!(obj["text"], "print('hello')");
        assert_eq!(obj["label"], "FILE");
    }

    #[test]
    fn function_node_repr() {
        let n = Node::Function(FunctionNode::new(
            "file:///repo/main.py",
            "do_work",
            2,
            Some(SourceRange::new(1, 0, 10, 0)),
            Some(SourceRange::new(1, 0, 10, 0)),
            "def do_work(): ...",
            None,
            Some("/main.py".into()),
            3,
        ));
        assert_eq!(n.node_repr_for_identifier(), ".do_work");
        assert_eq!(n.id(), "/main.py.do_work");
    }

    #[test]
    fn class_node_repr() {
        let n = Node::Class(ClassNode::new(
            "file:///repo/models.py",
            "User",
            2,
            None,
            None,
            "class User: ...",
            None,
            Some("/models.py".into()),
            5,
        ));
        assert_eq!(n.node_repr_for_identifier(), "#User");
        assert_eq!(n.id(), "/models.py#User");
    }

    #[test]
    fn hashed_id_is_deterministic() {
        let n1 = Node::File(FileNode::new(
            "file:///a.py",
            "a.py",
            0,
            None,
            None,
            "",
            None,
            None,
        ));
        let n2 = Node::File(FileNode::new(
            "file:///a.py",
            "a.py",
            0,
            None,
            None,
            "",
            None,
            None,
        ));
        assert_eq!(n1.hashed_id(), n2.hashed_id());
    }

    #[test]
    fn id_calculator_generate_file_id() {
        let id = IdCalculator::generate_file_id("blarify", "repo", "src/main.py");
        assert_eq!(id, "/blarify/repo/src/main.py");
    }

    #[test]
    fn id_calculator_hash_id_is_hex() {
        let hash = IdCalculator::hash_id("test");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn integration_node_commit_shortcut() {
        let n = IntegrationNode::commit("abc123", "fix bug", "diff...", None);
        assert_eq!(n.source, "github");
        assert_eq!(n.source_type, "commit");
        assert_eq!(n.external_id, "abc123");
    }

    #[test]
    fn documentation_node_mark_cycle() {
        let mut doc = DocumentationNode::new(
            "Some docs",
            "summary",
            "auto",
            "file:///src/lib.rs",
            "lib",
            "node-1",
            0,
            None,
            None,
        );
        doc.mark_cycle();
        assert!(doc.content.contains("Circular dependency"));
        assert_eq!(
            doc.metadata.as_ref().unwrap().get("has_cycle").unwrap(),
            "true"
        );
    }

    #[test]
    fn workflow_node_step_count() {
        let wf = WorkflowNode::new(
            "Test workflow",
            "content",
            "ep1",
            "entry",
            "file:///a",
            "ed1",
            "exit",
            "file:///b",
            vec!["s1".into(), "s2".into(), "s3".into()],
            0,
            None,
            None,
        );
        assert_eq!(wf.step_count(), 3);
        assert!(wf.has_valid_endpoints());
    }

    #[test]
    fn deleted_node_basics() {
        let n = Node::Deleted(DeletedNode::new("file:///repo/DELETED-1234", None));
        assert_eq!(n.label(), NodeLabel::Deleted);
        assert_eq!(n.name(), "DELETED");
    }

    #[test]
    fn folder_node_get_relationships() {
        let mut folder = FolderNode::new("file:///repo/src", "src", 0, None, None);
        folder.add_child("child-hash-1".into());
        folder.add_child("child-hash-2".into());
        let node = Node::Folder(folder);
        let rels = node.get_relationships();
        assert_eq!(rels.len(), 2);
        assert_eq!(rels[0].rel_type, RelationshipType::Contains);
    }

    #[test]
    fn nesting_stats_default_zeroed() {
        let s = NestingStats::default();
        assert_eq!(s.max_indentation, 0.0);
        assert_eq!(s.sd, 0.0);
    }

    #[test]
    fn source_range_zero() {
        let r = SourceRange::zero();
        assert_eq!(r.start.line, 0);
        assert_eq!(r.end.character, 0);
    }

    #[test]
    fn definition_base_contains_line() {
        let base = DefinitionBase::new(
            NodeBase::new("file:///a.py", "a.py", 0, "code", None),
            None,
            Some(SourceRange::new(5, 0, 20, 0)),
            "",
            None,
        );
        assert!(base.contains_line(5));
        assert!(base.contains_line(15));
        assert!(base.contains_line(20));
        assert!(!base.contains_line(4));
        assert!(!base.contains_line(21));
    }
}
