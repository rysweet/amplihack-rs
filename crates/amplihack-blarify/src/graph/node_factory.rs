use super::node::{
    ClassNode, DeletedNode, FileNode, FolderNode, FunctionNode, GraphEnvironment, NodeLabel,
    SourceRange,
};
use crate::project::file_explorer::Folder;

/// Factory functions for creating graph nodes with consistent defaults.
pub struct NodeFactory;

impl NodeFactory {
    /// Create a [`FolderNode`] from a project folder.
    pub fn create_folder_node(
        folder: &Folder,
        parent_identifier: Option<String>,
        graph_environment: Option<GraphEnvironment>,
    ) -> FolderNode {
        FolderNode::new(
            folder.uri_path(),
            &folder.name,
            folder.level,
            graph_environment,
            parent_identifier,
        )
    }

    /// Create a [`FileNode`].
    #[allow(clippy::too_many_arguments)]
    pub fn create_file_node(
        path: impl Into<String>,
        name: impl Into<String>,
        level: u32,
        node_range: Option<SourceRange>,
        definition_range: Option<SourceRange>,
        code_text: impl Into<String>,
        parent_identifier: Option<String>,
        graph_environment: Option<GraphEnvironment>,
    ) -> FileNode {
        FileNode::new(
            path,
            name,
            level,
            definition_range,
            node_range,
            code_text,
            graph_environment,
            parent_identifier,
        )
    }

    /// Create a [`ClassNode`].
    #[allow(clippy::too_many_arguments)]
    pub fn create_class_node(
        class_name: impl Into<String>,
        path: impl Into<String>,
        definition_range: Option<SourceRange>,
        node_range: Option<SourceRange>,
        code_text: impl Into<String>,
        level: u32,
        parent_identifier: Option<String>,
        graph_environment: Option<GraphEnvironment>,
        methods_defined: u32,
    ) -> ClassNode {
        ClassNode::new(
            path,
            class_name,
            level,
            definition_range,
            node_range,
            code_text,
            graph_environment,
            parent_identifier,
            methods_defined,
        )
    }

    /// Create a [`FunctionNode`].
    #[allow(clippy::too_many_arguments)]
    pub fn create_function_node(
        function_name: impl Into<String>,
        path: impl Into<String>,
        definition_range: Option<SourceRange>,
        node_range: Option<SourceRange>,
        code_text: impl Into<String>,
        level: u32,
        parent_identifier: Option<String>,
        graph_environment: Option<GraphEnvironment>,
        parameter_count: u32,
    ) -> FunctionNode {
        FunctionNode::new(
            path,
            function_name,
            level,
            definition_range,
            node_range,
            code_text,
            graph_environment,
            parent_identifier,
            parameter_count,
        )
    }

    /// Create a node based on a label (only CLASS and FUNCTION supported).
    #[allow(clippy::too_many_arguments)]
    pub fn create_node_based_on_label(
        label: NodeLabel,
        name: impl Into<String>,
        path: impl Into<String>,
        definition_range: Option<SourceRange>,
        node_range: Option<SourceRange>,
        code_text: impl Into<String>,
        level: u32,
        parent_identifier: Option<String>,
        graph_environment: Option<GraphEnvironment>,
    ) -> anyhow::Result<super::node::Node> {
        match label {
            NodeLabel::Class => Ok(super::node::Node::Class(Self::create_class_node(
                name,
                path,
                definition_range,
                node_range,
                code_text,
                level,
                parent_identifier,
                graph_environment,
                0,
            ))),
            NodeLabel::Function => Ok(super::node::Node::Function(Self::create_function_node(
                name,
                path,
                definition_range,
                node_range,
                code_text,
                level,
                parent_identifier,
                graph_environment,
                0,
            ))),
            other => anyhow::bail!("cannot create node for label: {other}"),
        }
    }

    /// Create a [`DeletedNode`] with a unique name.
    pub fn create_deleted_node(
        root_path: &str,
        unique_suffix: &str,
        graph_environment: Option<GraphEnvironment>,
    ) -> DeletedNode {
        let path = format!("file://{root_path}/DELETED-{unique_suffix}");
        DeletedNode::new(path, graph_environment)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::file_explorer::Folder;

    #[test]
    fn create_folder_node_from_folder() {
        let folder = Folder {
            name: "src".into(),
            path: "/repo/src".into(),
            files: vec![],
            folders: vec![],
            level: 1,
        };
        let node = NodeFactory::create_folder_node(&folder, None, None);
        assert_eq!(node.base.name, "src");
        assert_eq!(node.base.level, 1);
    }

    #[test]
    fn create_file_node() {
        let node = NodeFactory::create_file_node(
            "file:///repo/main.py",
            "main.py",
            1,
            None,
            None,
            "print('hi')",
            None,
            None,
        );
        assert_eq!(node.def.base.name, "main.py");
        assert_eq!(node.def.code_text, "print('hi')");
    }

    #[test]
    fn create_function_node() {
        let node = NodeFactory::create_function_node(
            "do_work",
            "file:///repo/main.py",
            None,
            None,
            "def do_work(): pass",
            2,
            None,
            None,
            3,
        );
        assert_eq!(node.def.base.name, "do_work");
        assert_eq!(node.parameter_count, 3);
    }

    #[test]
    fn create_class_node() {
        let node = NodeFactory::create_class_node(
            "User",
            "file:///repo/models.py",
            None,
            None,
            "class User: ...",
            2,
            None,
            None,
            5,
        );
        assert_eq!(node.def.base.name, "User");
        assert_eq!(node.methods_defined, 5);
    }

    #[test]
    fn create_node_based_on_label_class() {
        let node = NodeFactory::create_node_based_on_label(
            NodeLabel::Class,
            "MyClass",
            "file:///a.py",
            None,
            None,
            "",
            0,
            None,
            None,
        )
        .unwrap();
        assert_eq!(node.label(), NodeLabel::Class);
    }

    #[test]
    fn create_node_based_on_label_unsupported() {
        let result = NodeFactory::create_node_based_on_label(
            NodeLabel::Folder,
            "x",
            "file:///x",
            None,
            None,
            "",
            0,
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn create_deleted_node() {
        let node = NodeFactory::create_deleted_node("/repo", "abc-123", None);
        assert_eq!(node.base.path, "file:///repo/DELETED-abc-123");
        assert_eq!(node.base.name, "DELETED");
    }
}
