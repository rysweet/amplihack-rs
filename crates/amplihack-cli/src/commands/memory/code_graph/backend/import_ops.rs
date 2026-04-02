use super::super::super::backend::graph_db::{GraphDbConnection, GraphDbValue, graph_rows};
use super::super::{BlarifyClass, BlarifyFile, BlarifyFunction, BlarifyImport};
use super::{node_exists, parse_blarify_timestamp, relationship_exists};
use anyhow::Result;
use time::OffsetDateTime;

pub(super) fn import_files(conn: &GraphDbConnection<'_>, files: &[BlarifyFile]) -> Result<usize> {
    let now = OffsetDateTime::now_utc();
    let mut imported = 0usize;

    for file in files {
        if file.path.trim().is_empty() {
            continue;
        }

        let last_modified = parse_blarify_timestamp(file.last_modified.as_deref()).unwrap_or(now);
        let exists = node_exists(
            conn,
            "MATCH (cf:CodeFile {file_id: $file_id}) RETURN COUNT(cf)",
            vec![("file_id", GraphDbValue::String(file.path.clone()))],
        )?;

        if exists {
            graph_rows(
                conn,
                "MATCH (cf:CodeFile {file_id: $file_id}) SET cf.file_path = $file_path, cf.language = $language, cf.size_bytes = $size_bytes, cf.last_modified = $last_modified",
                vec![
                    ("file_id", GraphDbValue::String(file.path.clone())),
                    ("file_path", GraphDbValue::String(file.path.clone())),
                    ("language", GraphDbValue::String(file.language.clone())),
                    ("size_bytes", GraphDbValue::Int64(file.lines_of_code)),
                    ("last_modified", GraphDbValue::Timestamp(last_modified)),
                ],
            )?;
        } else {
            graph_rows(
                conn,
                "CREATE (cf:CodeFile {file_id: $file_id, file_path: $file_path, language: $language, size_bytes: $size_bytes, last_modified: $last_modified, created_at: $created_at, metadata: $metadata})",
                vec![
                    ("file_id", GraphDbValue::String(file.path.clone())),
                    ("file_path", GraphDbValue::String(file.path.clone())),
                    ("language", GraphDbValue::String(file.language.clone())),
                    ("size_bytes", GraphDbValue::Int64(file.lines_of_code)),
                    ("last_modified", GraphDbValue::Timestamp(last_modified)),
                    ("created_at", GraphDbValue::Timestamp(now)),
                    ("metadata", GraphDbValue::String("{}".to_string())),
                ],
            )?;
        }

        imported += 1;
    }

    Ok(imported)
}

pub(super) fn import_classes(
    conn: &GraphDbConnection<'_>,
    classes: &[BlarifyClass],
) -> Result<usize> {
    let now = OffsetDateTime::now_utc();
    let mut imported = 0usize;

    for class in classes {
        if class.id.trim().is_empty() {
            continue;
        }

        let metadata = serde_json::json!({ "line_number": class.line_number }).to_string();
        let exists = node_exists(
            conn,
            "MATCH (c:CodeClass {class_id: $class_id}) RETURN COUNT(c)",
            vec![("class_id", GraphDbValue::String(class.id.clone()))],
        )?;

        if exists {
            graph_rows(
                conn,
                "MATCH (c:CodeClass {class_id: $class_id}) SET c.class_name = $class_name, c.fully_qualified_name = $fully_qualified_name, c.file_path = $file_path, c.line_number = $line_number, c.docstring = $docstring, c.is_abstract = $is_abstract, c.metadata = $metadata",
                vec![
                    ("class_id", GraphDbValue::String(class.id.clone())),
                    ("class_name", GraphDbValue::String(class.name.clone())),
                    (
                        "fully_qualified_name",
                        GraphDbValue::String(class.id.clone()),
                    ),
                    ("file_path", GraphDbValue::String(class.file_path.clone())),
                    ("line_number", GraphDbValue::Int64(class.line_number)),
                    ("docstring", GraphDbValue::String(class.docstring.clone())),
                    ("is_abstract", GraphDbValue::Bool(class.is_abstract)),
                    ("metadata", GraphDbValue::String(metadata.clone())),
                ],
            )?;
        } else {
            graph_rows(
                conn,
                "CREATE (c:CodeClass {class_id: $class_id, class_name: $class_name, fully_qualified_name: $fully_qualified_name, file_path: $file_path, line_number: $line_number, docstring: $docstring, is_abstract: $is_abstract, created_at: $created_at, metadata: $metadata})",
                vec![
                    ("class_id", GraphDbValue::String(class.id.clone())),
                    ("class_name", GraphDbValue::String(class.name.clone())),
                    (
                        "fully_qualified_name",
                        GraphDbValue::String(class.id.clone()),
                    ),
                    ("file_path", GraphDbValue::String(class.file_path.clone())),
                    ("line_number", GraphDbValue::Int64(class.line_number)),
                    ("docstring", GraphDbValue::String(class.docstring.clone())),
                    ("is_abstract", GraphDbValue::Bool(class.is_abstract)),
                    ("created_at", GraphDbValue::Timestamp(now)),
                    ("metadata", GraphDbValue::String(metadata)),
                ],
            )?;
        }

        if !class.file_path.is_empty()
            && !relationship_exists(
                conn,
                "MATCH (c:CodeClass {class_id: $class_id})-[r:CLASS_DEFINED_IN]->(cf:CodeFile {file_id: $file_id}) RETURN COUNT(r)",
                vec![
                    ("class_id", GraphDbValue::String(class.id.clone())),
                    ("file_id", GraphDbValue::String(class.file_path.clone())),
                ],
            )?
        {
            graph_rows(
                conn,
                "MATCH (c:CodeClass {class_id: $class_id}) MATCH (cf:CodeFile {file_id: $file_id}) CREATE (c)-[:CLASS_DEFINED_IN {line_number: $line_number}]->(cf)",
                vec![
                    ("class_id", GraphDbValue::String(class.id.clone())),
                    ("file_id", GraphDbValue::String(class.file_path.clone())),
                    ("line_number", GraphDbValue::Int64(class.line_number)),
                ],
            )?;
        }

        imported += 1;
    }

    Ok(imported)
}

pub(super) fn import_functions(
    conn: &GraphDbConnection<'_>,
    functions: &[BlarifyFunction],
) -> Result<usize> {
    let now = OffsetDateTime::now_utc();
    let mut imported = 0usize;

    for function in functions {
        if function.id.trim().is_empty() {
            continue;
        }

        let parameters_json = serde_json::to_string(&function.parameters)?;
        let signature = format!("{}({})", function.name, function.parameters.join(", "));
        let metadata = serde_json::json!({
            "line_number": function.line_number,
            "parameters": function.parameters,
            "return_type": function.return_type,
        })
        .to_string();
        let exists = node_exists(
            conn,
            "MATCH (f:CodeFunction {function_id: $function_id}) RETURN COUNT(f)",
            vec![("function_id", GraphDbValue::String(function.id.clone()))],
        )?;

        if exists {
            graph_rows(
                conn,
                "MATCH (f:CodeFunction {function_id: $function_id}) SET f.function_name = $function_name, f.fully_qualified_name = $fully_qualified_name, f.signature = $signature, f.file_path = $file_path, f.line_number = $line_number, f.parameters = $parameters, f.return_type = $return_type, f.docstring = $docstring, f.is_async = $is_async, f.cyclomatic_complexity = $cyclomatic_complexity, f.metadata = $metadata",
                vec![
                    ("function_id", GraphDbValue::String(function.id.clone())),
                    ("function_name", GraphDbValue::String(function.name.clone())),
                    (
                        "fully_qualified_name",
                        GraphDbValue::String(function.id.clone()),
                    ),
                    ("signature", GraphDbValue::String(signature.clone())),
                    (
                        "file_path",
                        GraphDbValue::String(function.file_path.clone()),
                    ),
                    ("line_number", GraphDbValue::Int64(function.line_number)),
                    ("parameters", GraphDbValue::String(parameters_json.clone())),
                    (
                        "return_type",
                        GraphDbValue::String(function.return_type.clone()),
                    ),
                    (
                        "docstring",
                        GraphDbValue::String(function.docstring.clone()),
                    ),
                    ("is_async", GraphDbValue::Bool(function.is_async)),
                    (
                        "cyclomatic_complexity",
                        GraphDbValue::Int64(function.complexity),
                    ),
                    ("metadata", GraphDbValue::String(metadata.clone())),
                ],
            )?;
        } else {
            graph_rows(
                conn,
                "CREATE (f:CodeFunction {function_id: $function_id, function_name: $function_name, fully_qualified_name: $fully_qualified_name, signature: $signature, file_path: $file_path, line_number: $line_number, parameters: $parameters, return_type: $return_type, docstring: $docstring, is_async: $is_async, cyclomatic_complexity: $cyclomatic_complexity, created_at: $created_at, metadata: $metadata})",
                vec![
                    ("function_id", GraphDbValue::String(function.id.clone())),
                    ("function_name", GraphDbValue::String(function.name.clone())),
                    (
                        "fully_qualified_name",
                        GraphDbValue::String(function.id.clone()),
                    ),
                    ("signature", GraphDbValue::String(signature)),
                    (
                        "file_path",
                        GraphDbValue::String(function.file_path.clone()),
                    ),
                    ("line_number", GraphDbValue::Int64(function.line_number)),
                    ("parameters", GraphDbValue::String(parameters_json)),
                    (
                        "return_type",
                        GraphDbValue::String(function.return_type.clone()),
                    ),
                    (
                        "docstring",
                        GraphDbValue::String(function.docstring.clone()),
                    ),
                    ("is_async", GraphDbValue::Bool(function.is_async)),
                    (
                        "cyclomatic_complexity",
                        GraphDbValue::Int64(function.complexity),
                    ),
                    ("created_at", GraphDbValue::Timestamp(now)),
                    ("metadata", GraphDbValue::String(metadata)),
                ],
            )?;
        }

        if !function.file_path.is_empty()
            && !relationship_exists(
                conn,
                "MATCH (f:CodeFunction {function_id: $function_id})-[r:DEFINED_IN]->(cf:CodeFile {file_id: $file_id}) RETURN COUNT(r)",
                vec![
                    ("function_id", GraphDbValue::String(function.id.clone())),
                    ("file_id", GraphDbValue::String(function.file_path.clone())),
                ],
            )?
        {
            graph_rows(
                conn,
                "MATCH (f:CodeFunction {function_id: $function_id}) MATCH (cf:CodeFile {file_id: $file_id}) CREATE (f)-[:DEFINED_IN {line_number: $line_number, end_line: $end_line}]->(cf)",
                vec![
                    ("function_id", GraphDbValue::String(function.id.clone())),
                    ("file_id", GraphDbValue::String(function.file_path.clone())),
                    ("line_number", GraphDbValue::Int64(function.line_number)),
                    ("end_line", GraphDbValue::Int64(function.line_number)),
                ],
            )?;
        }

        if let Some(class_id) = function.class_id.as_ref()
            && !relationship_exists(
                conn,
                "MATCH (f:CodeFunction {function_id: $function_id})-[r:METHOD_OF]->(c:CodeClass {class_id: $class_id}) RETURN COUNT(r)",
                vec![
                    ("function_id", GraphDbValue::String(function.id.clone())),
                    ("class_id", GraphDbValue::String(class_id.clone())),
                ],
            )?
        {
            graph_rows(
                conn,
                "MATCH (f:CodeFunction {function_id: $function_id}) MATCH (c:CodeClass {class_id: $class_id}) CREATE (f)-[:METHOD_OF {method_type: $method_type, visibility: $visibility}]->(c)",
                vec![
                    ("function_id", GraphDbValue::String(function.id.clone())),
                    ("class_id", GraphDbValue::String(class_id.clone())),
                    ("method_type", GraphDbValue::String("instance".to_string())),
                    ("visibility", GraphDbValue::String("public".to_string())),
                ],
            )?;
        }

        imported += 1;
    }

    Ok(imported)
}

pub(super) fn import_imports(
    conn: &GraphDbConnection<'_>,
    imports: &[BlarifyImport],
) -> Result<usize> {
    let mut imported = 0usize;

    for import in imports {
        if import.source_file.trim().is_empty() || import.target_file.trim().is_empty() {
            continue;
        }

        if relationship_exists(
            conn,
            "MATCH (source:CodeFile {file_id: $source_file})-[r:IMPORTS]->(target:CodeFile {file_id: $target_file}) WHERE r.import_type = $import_type RETURN COUNT(r)",
            vec![
                (
                    "source_file",
                    GraphDbValue::String(import.source_file.clone()),
                ),
                (
                    "target_file",
                    GraphDbValue::String(import.target_file.clone()),
                ),
                ("import_type", GraphDbValue::String(import.symbol.clone())),
            ],
        )? {
            continue;
        }

        graph_rows(
            conn,
            "MATCH (source:CodeFile {file_id: $source_file}) MATCH (target:CodeFile {file_id: $target_file}) CREATE (source)-[:IMPORTS {import_type: $import_type, alias: $alias}]->(target)",
            vec![
                (
                    "source_file",
                    GraphDbValue::String(import.source_file.clone()),
                ),
                (
                    "target_file",
                    GraphDbValue::String(import.target_file.clone()),
                ),
                ("import_type", GraphDbValue::String(import.symbol.clone())),
                (
                    "alias",
                    GraphDbValue::String(import.alias.clone().unwrap_or_default()),
                ),
            ],
        )?;
        imported += 1;
    }

    Ok(imported)
}
