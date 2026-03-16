use crate::QueryCodeCommands;
use crate::commands::memory::{
    code_graph::{
        code_memory_link_counts, ensure_memory_code_link_schema, init_kuzu_code_graph_schema,
        open_kuzu_code_graph_db,
    },
    kuzu_i64, kuzu_rows, kuzu_string,
};
use anyhow::Result;
use kuzu::Connection as KuzuConnection;
use kuzu::Value as KuzuValue;
use serde_json::json;
use std::path::Path;

const MEMORY_FILE_RELATIONSHIPS: &[(&str, &str)] = &[
    ("EpisodicMemory", "RELATES_TO_FILE_EPISODIC"),
    ("SemanticMemory", "RELATES_TO_FILE_SEMANTIC"),
    ("ProceduralMemory", "RELATES_TO_FILE_PROCEDURAL"),
    ("ProspectiveMemory", "RELATES_TO_FILE_PROSPECTIVE"),
    ("WorkingMemory", "RELATES_TO_FILE_WORKING"),
];

const MEMORY_FUNCTION_RELATIONSHIPS: &[(&str, &str)] = &[
    ("EpisodicMemory", "RELATES_TO_FUNCTION_EPISODIC"),
    ("SemanticMemory", "RELATES_TO_FUNCTION_SEMANTIC"),
    ("ProceduralMemory", "RELATES_TO_FUNCTION_PROCEDURAL"),
    ("ProspectiveMemory", "RELATES_TO_FUNCTION_PROSPECTIVE"),
    ("WorkingMemory", "RELATES_TO_FUNCTION_WORKING"),
];

pub fn run_query_code(
    command: QueryCodeCommands,
    kuzu_path: Option<&Path>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    let db = open_kuzu_code_graph_db(kuzu_path)?;
    let conn = KuzuConnection::new(&db)?;
    init_kuzu_code_graph_schema(&conn)?;
    ensure_memory_code_link_schema(&conn)?;

    match command {
        QueryCodeCommands::Stats => run_stats(&conn, json_output),
        QueryCodeCommands::Context { memory_id } => run_context(&conn, &memory_id, json_output),
        QueryCodeCommands::Files { pattern } => {
            run_files(&conn, pattern.as_deref(), json_output, limit)
        }
        QueryCodeCommands::Functions { file } => {
            run_functions(&conn, file.as_deref(), json_output, limit)
        }
        QueryCodeCommands::Classes { file } => {
            run_classes(&conn, file.as_deref(), json_output, limit)
        }
        QueryCodeCommands::Search { name } => run_search(&conn, &name, json_output, limit),
        QueryCodeCommands::Callers { name } => run_callers(&conn, &name, json_output, limit),
        QueryCodeCommands::Callees { name } => run_callees(&conn, &name, json_output, limit),
    }
}

fn run_stats(conn: &KuzuConnection<'_>, json_output: bool) -> Result<()> {
    let files = scalar_count(conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)")?;
    let classes = scalar_count(conn, "MATCH (c:CodeClass) RETURN COUNT(c)")?;
    let functions = scalar_count(conn, "MATCH (f:CodeFunction) RETURN COUNT(f)")?;
    let (memory_file_links, memory_function_links) = code_memory_link_counts(conn)?;

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "files": files,
                "classes": classes,
                "functions": functions,
                "memory_file_links": memory_file_links,
                "memory_function_links": memory_function_links
            }))?
        );
    } else {
        println!("Code Graph Statistics:");
        println!("  Files:     {files}");
        println!("  Classes:   {classes}");
        println!("  Functions: {functions}");
        println!("  Memory→File links:     {memory_file_links}");
        println!("  Memory→Function links: {memory_function_links}");
    }

    Ok(())
}

fn run_context(conn: &KuzuConnection<'_>, memory_id: &str, json_output: bool) -> Result<()> {
    let payload = query_context_payload(conn, memory_id)?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("Code context for memory '{memory_id}':");

    let files = payload["files"].as_array().cloned().unwrap_or_default();
    if files.is_empty() {
        println!("  Files: none");
    } else {
        println!("  Files:");
        for file in files {
            println!(
                "    - {} [{}] ({} bytes)",
                file["path"].as_str().unwrap_or_default(),
                file["language"].as_str().unwrap_or_default(),
                file["size_bytes"].as_i64().unwrap_or_default()
            );
        }
    }

    let functions = payload["functions"].as_array().cloned().unwrap_or_default();
    if functions.is_empty() {
        println!("  Functions: none");
    } else {
        println!("  Functions:");
        for function in functions {
            println!(
                "    - {} :: {}",
                function["name"].as_str().unwrap_or_default(),
                function["signature"].as_str().unwrap_or_default()
            );
        }
    }

    let classes = payload["classes"].as_array().cloned().unwrap_or_default();
    if classes.is_empty() {
        println!("  Classes: none");
    } else {
        println!("  Classes:");
        for class in classes {
            println!(
                "    - {} ({})",
                class["name"].as_str().unwrap_or_default(),
                class["fully_qualified_name"].as_str().unwrap_or_default()
            );
        }
    }

    Ok(())
}

fn run_files(
    conn: &KuzuConnection<'_>,
    pattern: Option<&str>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    let rows = if let Some(pattern) = pattern {
        kuzu_rows(
            conn,
            "MATCH (cf:CodeFile) WHERE cf.file_path CONTAINS $pattern RETURN cf.file_path ORDER BY cf.file_path LIMIT $lim",
            vec![
                ("pattern", KuzuValue::String(pattern.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        kuzu_rows(
            conn,
            "MATCH (cf:CodeFile) RETURN cf.file_path ORDER BY cf.file_path LIMIT $lim",
            vec![("lim", KuzuValue::UInt64(u64::from(limit)))],
        )?
    };
    let values: Vec<String> = rows
        .iter()
        .map(|row| kuzu_string(row.first()).unwrap_or_default())
        .collect();
    print_string_list(values, json_output, limit)
}

fn run_functions(
    conn: &KuzuConnection<'_>,
    file: Option<&str>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    let rows = if let Some(file) = file {
        kuzu_rows(
            conn,
            "MATCH (f:CodeFunction)-[:DEFINED_IN]->(cf:CodeFile) WHERE cf.file_path CONTAINS $file AND NOT f.function_name CONTAINS '().(' RETURN f.function_name, cf.file_path ORDER BY cf.file_path, f.function_name LIMIT $lim",
            vec![
                ("file", KuzuValue::String(file.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        kuzu_rows(
            conn,
            "MATCH (f:CodeFunction) WHERE NOT f.function_name CONTAINS '().(' RETURN f.function_name ORDER BY f.function_name LIMIT $lim",
            vec![("lim", KuzuValue::UInt64(u64::from(limit)))],
        )?
    };

    if json_output {
        let payload: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                if row.len() > 1 {
                    json!({
                        "name": kuzu_string(row.first()).unwrap_or_default(),
                        "file": kuzu_string(row.get(1)).unwrap_or_default()
                    })
                } else {
                    json!({"name": kuzu_string(row.first()).unwrap_or_default()})
                }
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        for row in &rows {
            if row.len() > 1 {
                println!(
                    "  {}::{}",
                    kuzu_string(row.get(1))?,
                    kuzu_string(row.first())?
                );
            } else {
                println!("  {}", kuzu_string(row.first())?);
            }
        }
        print_limit_hint(rows.len(), limit);
    }

    Ok(())
}

fn run_classes(
    conn: &KuzuConnection<'_>,
    file: Option<&str>,
    json_output: bool,
    limit: u32,
) -> Result<()> {
    let rows = if let Some(file) = file {
        kuzu_rows(
            conn,
            "MATCH (c:CodeClass)-[:CLASS_DEFINED_IN]->(cf:CodeFile) WHERE cf.file_path CONTAINS $file RETURN c.class_name, cf.file_path ORDER BY cf.file_path, c.class_name LIMIT $lim",
            vec![
                ("file", KuzuValue::String(file.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?
    } else {
        kuzu_rows(
            conn,
            "MATCH (c:CodeClass) RETURN c.class_name ORDER BY c.class_name LIMIT $lim",
            vec![("lim", KuzuValue::UInt64(u64::from(limit)))],
        )?
    };

    if json_output {
        let payload: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                if row.len() > 1 {
                    json!({
                        "name": kuzu_string(row.first()).unwrap_or_default(),
                        "file": kuzu_string(row.get(1)).unwrap_or_default()
                    })
                } else {
                    json!({"name": kuzu_string(row.first()).unwrap_or_default()})
                }
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        for row in &rows {
            if row.len() > 1 {
                println!(
                    "  {}::{}",
                    kuzu_string(row.get(1))?,
                    kuzu_string(row.first())?
                );
            } else {
                println!("  {}", kuzu_string(row.first())?);
            }
        }
        print_limit_hint(rows.len(), limit);
    }

    Ok(())
}

fn run_search(conn: &KuzuConnection<'_>, name: &str, json_output: bool, limit: u32) -> Result<()> {
    let mut payload = Vec::new();
    let searches = [
        (
            "function",
            "MATCH (f:CodeFunction) WHERE f.function_name CONTAINS $name AND NOT f.function_name CONTAINS '().(' RETURN f.function_name LIMIT $lim",
        ),
        (
            "class",
            "MATCH (c:CodeClass) WHERE c.class_name CONTAINS $name RETURN c.class_name LIMIT $lim",
        ),
        (
            "file",
            "MATCH (cf:CodeFile) WHERE cf.file_path CONTAINS $name RETURN cf.file_path LIMIT $lim",
        ),
    ];

    for (kind, query) in searches {
        let rows = kuzu_rows(
            conn,
            query,
            vec![
                ("name", KuzuValue::String(name.to_string())),
                ("lim", KuzuValue::UInt64(u64::from(limit))),
            ],
        )?;
        for row in rows {
            payload.push(json!({
                "type": kind,
                "name": kuzu_string(row.first())?
            }));
        }
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if payload.is_empty() {
        println!("No results for '{name}'");
    } else {
        for item in payload {
            println!(
                "  [{}] {}",
                item["type"].as_str().unwrap_or("unknown"),
                item["name"].as_str().unwrap_or_default()
            );
        }
    }

    Ok(())
}

fn query_context_payload(conn: &KuzuConnection<'_>, memory_id: &str) -> Result<serde_json::Value> {
    let Some((memory_type, file_rel, function_rel)) = resolve_memory_link_tables(conn, memory_id)?
    else {
        return Ok(json!({
            "memory_id": memory_id,
            "files": [],
            "functions": [],
            "classes": [],
        }));
    };

    let files = kuzu_rows(
        conn,
        &format!(
            "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[:{file_rel}]->(cf:CodeFile) RETURN cf.file_path, cf.language, cf.size_bytes ORDER BY cf.file_path"
        ),
        vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
    )?;
    let functions = kuzu_rows(
        conn,
        &format!(
            "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[:{function_rel}]->(f:CodeFunction) RETURN f.function_name, f.signature, f.docstring, f.cyclomatic_complexity ORDER BY f.function_name"
        ),
        vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
    )?;
    let classes = kuzu_rows(
        conn,
        &format!(
            "MATCH (m:{memory_type} {{memory_id: $memory_id}})-[:{function_rel}]->(f:CodeFunction)-[:METHOD_OF]->(c:CodeClass) RETURN DISTINCT c.class_name, c.fully_qualified_name, c.docstring ORDER BY c.class_name"
        ),
        vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
    )?;

    Ok(json!({
        "memory_id": memory_id,
        "files": files.iter().map(|row| json!({
            "type": "file",
            "path": kuzu_string(row.first()).unwrap_or_default(),
            "language": kuzu_string(row.get(1)).unwrap_or_default(),
            "size_bytes": kuzu_i64(row.get(2)).unwrap_or_default(),
        })).collect::<Vec<_>>(),
        "functions": functions.iter().map(|row| json!({
            "type": "function",
            "name": kuzu_string(row.first()).unwrap_or_default(),
            "signature": kuzu_string(row.get(1)).unwrap_or_default(),
            "docstring": kuzu_string(row.get(2)).unwrap_or_default(),
            "complexity": kuzu_i64(row.get(3)).unwrap_or_default(),
        })).collect::<Vec<_>>(),
        "classes": classes.iter().map(|row| json!({
            "type": "class",
            "name": kuzu_string(row.first()).unwrap_or_default(),
            "fully_qualified_name": kuzu_string(row.get(1)).unwrap_or_default(),
            "docstring": kuzu_string(row.get(2)).unwrap_or_default(),
        })).collect::<Vec<_>>(),
    }))
}

fn resolve_memory_link_tables(
    conn: &KuzuConnection<'_>,
    memory_id: &str,
) -> Result<Option<(&'static str, &'static str, &'static str)>> {
    for ((memory_type, file_rel), (_, function_rel)) in MEMORY_FILE_RELATIONSHIPS
        .iter()
        .zip(MEMORY_FUNCTION_RELATIONSHIPS.iter())
    {
        let rows = kuzu_rows(
            conn,
            &format!("MATCH (m:{memory_type} {{memory_id: $memory_id}}) RETURN COUNT(m)"),
            vec![("memory_id", KuzuValue::String(memory_id.to_string()))],
        )?;
        let count = kuzu_i64(rows.first().and_then(|row| row.first()))?;
        if count > 0 {
            return Ok(Some((memory_type, file_rel, function_rel)));
        }
    }

    Ok(None)
}

fn run_callers(conn: &KuzuConnection<'_>, name: &str, json_output: bool, limit: u32) -> Result<()> {
    run_edge_query(
        conn,
        "MATCH (caller:CodeFunction)-[:CALLS]->(callee:CodeFunction) WHERE callee.function_name CONTAINS $name RETURN caller.function_name, callee.function_name LIMIT $lim",
        name,
        json_output,
        limit,
        &format!("Functions calling '{name}':"),
        &format!("No callers found for '{name}'"),
    )
}

fn run_callees(conn: &KuzuConnection<'_>, name: &str, json_output: bool, limit: u32) -> Result<()> {
    run_edge_query(
        conn,
        "MATCH (caller:CodeFunction)-[:CALLS]->(callee:CodeFunction) WHERE caller.function_name CONTAINS $name RETURN caller.function_name, callee.function_name LIMIT $lim",
        name,
        json_output,
        limit,
        &format!("Functions called by '{name}':"),
        &format!("No callees found for '{name}'"),
    )
}

fn run_edge_query(
    conn: &KuzuConnection<'_>,
    query: &str,
    name: &str,
    json_output: bool,
    limit: u32,
    heading: &str,
    empty_message: &str,
) -> Result<()> {
    let rows = kuzu_rows(
        conn,
        query,
        vec![
            ("name", KuzuValue::String(name.to_string())),
            ("lim", KuzuValue::UInt64(u64::from(limit))),
        ],
    )?;

    if json_output {
        let payload: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                json!({
                    "caller": kuzu_string(row.first()).unwrap_or_default(),
                    "callee": kuzu_string(row.get(1)).unwrap_or_default()
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if rows.is_empty() {
        println!("{empty_message}");
    } else {
        println!("{heading}");
        for row in &rows {
            println!(
                "  {} -> {}",
                kuzu_string(row.first())?,
                kuzu_string(row.get(1))?
            );
        }
    }

    Ok(())
}

fn scalar_count(conn: &KuzuConnection<'_>, query: &str) -> Result<i64> {
    let rows = kuzu_rows(conn, query, vec![])?;
    kuzu_i64(rows.first().and_then(|row| row.first()))
}

fn print_string_list(values: Vec<String>, json_output: bool, limit: u32) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        for value in &values {
            println!("{value}");
        }
        print_limit_hint(values.len(), limit);
    }
    Ok(())
}

fn print_limit_hint(actual_len: usize, limit: u32) {
    if actual_len == limit as usize {
        println!("... (showing first {limit}, use --limit to see more)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::memory::{
        code_graph::{CodeGraphImportCounts, import_blarify_json},
        init_kuzu_backend_schema,
    };
    use kuzu::Value as KuzuValue;
    use std::fs;
    use time::OffsetDateTime;

    fn seed_code_graph(db_path: &Path) {
        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::json!({
                "files": [
                    {"path":"src/example/module.py","language":"python","lines_of_code":10},
                    {"path":"src/example/utils.py","language":"python","lines_of_code":5}
                ],
                "classes": [
                    {"id":"class:Example","name":"Example","file_path":"src/example/module.py","line_number":1}
                ],
                "functions": [
                    {"id":"func:Example.process","name":"process","file_path":"src/example/module.py","line_number":2,"class_id":"class:Example"},
                    {"id":"func:helper","name":"helper","file_path":"src/example/utils.py","line_number":1}
                ],
                "imports": [],
                "relationships": [
                    {"type":"CALLS","source_id":"func:Example.process","target_id":"func:helper"}
                ]
            })
            .to_string(),
        )
        .unwrap();
        let counts = import_blarify_json(&json_path, Some(db_path)).unwrap();
        assert_eq!(
            counts,
            CodeGraphImportCounts {
                files: 2,
                classes: 1,
                functions: 2,
                imports: 0,
                relationships: 1
            }
        );
    }

    #[test]
    fn query_code_stats_reads_seeded_graph() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("code-graph.kuzu");
        seed_code_graph(&db_path);
        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();

        let files = scalar_count(&conn, "MATCH (cf:CodeFile) RETURN COUNT(cf)").unwrap();
        let classes = scalar_count(&conn, "MATCH (c:CodeClass) RETURN COUNT(c)").unwrap();
        let functions = scalar_count(&conn, "MATCH (f:CodeFunction) RETURN COUNT(f)").unwrap();
        let (memory_file_links, memory_function_links) = code_memory_link_counts(&conn).unwrap();

        assert_eq!((files, classes, functions), (2, 1, 2));
        assert_eq!((memory_file_links, memory_function_links), (0, 0));
    }

    #[test]
    fn query_code_search_finds_seeded_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("code-graph.kuzu");
        seed_code_graph(&db_path);
        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();

        let rows = kuzu_rows(
            &conn,
            "MATCH (f:CodeFunction) WHERE f.function_name CONTAINS $name RETURN f.function_name LIMIT $lim",
            vec![
                ("name", KuzuValue::String("helper".to_string())),
                ("lim", KuzuValue::UInt64(10)),
            ],
        )
        .unwrap();

        assert_eq!(kuzu_string(rows[0].first()).unwrap(), "helper");
    }

    #[test]
    fn query_code_context_returns_linked_code_entities() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("code-graph.kuzu");
        seed_code_graph(&db_path);
        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();
        init_kuzu_backend_schema(&conn).unwrap();
        let now = OffsetDateTime::now_utc();

        let mut create_memory = conn.prepare(
            "CREATE (m:SemanticMemory {memory_id: $memory_id, concept: $concept, content: $content, category: $category, confidence_score: $confidence_score, last_updated: $last_updated, version: $version, title: $title, metadata: $metadata, tags: $tags, created_at: $created_at, accessed_at: $accessed_at, agent_id: $agent_id})",
        ).unwrap();
        conn.execute(
            &mut create_memory,
            vec![
                ("memory_id", KuzuValue::String("mem-query".to_string())),
                ("concept", KuzuValue::String("Query context".to_string())),
                (
                    "content",
                    KuzuValue::String("helper is relevant here".to_string()),
                ),
                ("category", KuzuValue::String("session_end".to_string())),
                ("confidence_score", KuzuValue::Double(1.0)),
                ("last_updated", KuzuValue::Timestamp(now)),
                ("version", KuzuValue::Int64(1)),
                ("title", KuzuValue::String("Helper context".to_string())),
                (
                    "metadata",
                    KuzuValue::String(r#"{"file":"src/example/module.py"}"#.to_string()),
                ),
                ("tags", KuzuValue::String(r#"["learning"]"#.to_string())),
                ("created_at", KuzuValue::Timestamp(now)),
                ("accessed_at", KuzuValue::Timestamp(now)),
                ("agent_id", KuzuValue::String("agent-1".to_string())),
            ],
        )
        .unwrap();

        let import_dir = tempfile::tempdir().unwrap();
        let json_path = import_dir.path().join("blarify.json");
        fs::write(
            &json_path,
            serde_json::json!({
                "files": [
                    {"path":"src/example/module.py","language":"python","lines_of_code":10},
                    {"path":"src/example/utils.py","language":"python","lines_of_code":5}
                ],
                "classes": [
                    {"id":"class:Example","name":"Example","file_path":"src/example/module.py","line_number":1}
                ],
                "functions": [
                    {"id":"func:Example.process","name":"process","file_path":"src/example/module.py","line_number":2,"class_id":"class:Example"},
                    {"id":"func:helper","name":"helper","file_path":"src/example/utils.py","line_number":1}
                ],
                "imports": [],
                "relationships": []
            }).to_string(),
        ).unwrap();
        import_blarify_json(&json_path, Some(&db_path)).unwrap();

        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();
        let payload = query_context_payload(&conn, "mem-query").unwrap();
        assert_eq!(payload["memory_id"], "mem-query");
        assert_eq!(payload["files"].as_array().unwrap().len(), 1);
        assert_eq!(payload["functions"].as_array().unwrap().len(), 1);
        assert_eq!(payload["functions"][0]["name"], "helper");
        assert_eq!(payload["classes"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn query_code_context_returns_empty_for_unknown_memory() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("code-graph.kuzu");
        seed_code_graph(&db_path);
        let db = open_kuzu_code_graph_db(Some(&db_path)).unwrap();
        let conn = KuzuConnection::new(&db).unwrap();

        let payload = query_context_payload(&conn, "missing-memory").unwrap();
        assert_eq!(payload["memory_id"], "missing-memory");
        assert!(payload["files"].as_array().unwrap().is_empty());
        assert!(payload["functions"].as_array().unwrap().is_empty());
        assert!(payload["classes"].as_array().unwrap().is_empty());
    }
}
