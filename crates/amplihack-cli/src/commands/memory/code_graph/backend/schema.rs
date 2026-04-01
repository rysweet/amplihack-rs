pub(in crate::commands::memory::code_graph) const GRAPH_CODE_GRAPH_SCHEMA: &[&str] = &[
    r#"CREATE NODE TABLE IF NOT EXISTS CodeFile(
        file_id STRING,
        file_path STRING,
        language STRING,
        size_bytes INT64,
        last_modified TIMESTAMP,
        created_at TIMESTAMP,
        metadata STRING,
        PRIMARY KEY (file_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS CodeClass(
        class_id STRING,
        class_name STRING,
        fully_qualified_name STRING,
        file_path STRING,
        line_number INT64,
        docstring STRING,
        is_abstract BOOL,
        created_at TIMESTAMP,
        metadata STRING,
        PRIMARY KEY (class_id)
    )"#,
    r#"CREATE NODE TABLE IF NOT EXISTS CodeFunction(
        function_id STRING,
        function_name STRING,
        fully_qualified_name STRING,
        signature STRING,
        file_path STRING,
        line_number INT64,
        parameters STRING,
        return_type STRING,
        docstring STRING,
        is_async BOOL,
        cyclomatic_complexity INT64,
        created_at TIMESTAMP,
        metadata STRING,
        PRIMARY KEY (function_id)
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS DEFINED_IN(
        FROM CodeFunction TO CodeFile,
        line_number INT64,
        end_line INT64
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS CLASS_DEFINED_IN(
        FROM CodeClass TO CodeFile,
        line_number INT64
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS METHOD_OF(
        FROM CodeFunction TO CodeClass,
        method_type STRING,
        visibility STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS CALLS(
        FROM CodeFunction TO CodeFunction,
        call_count INT64,
        context STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS INHERITS(
        FROM CodeClass TO CodeClass,
        inheritance_order INT64,
        inheritance_type STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS REFERENCES_CLASS(
        FROM CodeFunction TO CodeClass,
        reference_type STRING,
        context STRING
    )"#,
    r#"CREATE REL TABLE IF NOT EXISTS IMPORTS(
        FROM CodeFile TO CodeFile,
        import_type STRING,
        alias STRING
    )"#,
];

pub(in crate::commands::memory::code_graph) const GRAPH_MEMORY_FILE_LINK_TABLES: &[(&str, &str)] = &[
    ("EpisodicMemory", "RELATES_TO_FILE_EPISODIC"),
    ("SemanticMemory", "RELATES_TO_FILE_SEMANTIC"),
    ("ProceduralMemory", "RELATES_TO_FILE_PROCEDURAL"),
    ("ProspectiveMemory", "RELATES_TO_FILE_PROSPECTIVE"),
    ("WorkingMemory", "RELATES_TO_FILE_WORKING"),
];

pub(in crate::commands::memory::code_graph) const GRAPH_MEMORY_FUNCTION_LINK_TABLES: &[(&str, &str)] = &[
    ("EpisodicMemory", "RELATES_TO_FUNCTION_EPISODIC"),
    ("SemanticMemory", "RELATES_TO_FUNCTION_SEMANTIC"),
    ("ProceduralMemory", "RELATES_TO_FUNCTION_PROCEDURAL"),
    ("ProspectiveMemory", "RELATES_TO_FUNCTION_PROSPECTIVE"),
    ("WorkingMemory", "RELATES_TO_FUNCTION_WORKING"),
];
