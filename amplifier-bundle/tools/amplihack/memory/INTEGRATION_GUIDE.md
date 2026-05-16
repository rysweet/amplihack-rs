# Agent Memory System - Integration Guide

This guide provides comprehensive documentation for integrating the Agent Memory System into Claude workflows and agent operations.

## Overview

The Agent Memory System provides persistent memory capabilities for AI agents with:

- **Session-based isolation**: Each conversation session has isolated memory space
- **Agent namespacing**: Memories organized by agent identifiers
- **Thread-safe operations**: Concurrent access with proper locking
- **High performance**: <50ms operations with efficient indexing
- **Secure storage**: 600-permission SQLite files with ACID compliance
- **Graceful degradation**: Optional activation for zero-impact integration

## Quick Start

### Basic Memory Operations

```rust
use amplihack_memory::{get_memory_manager, MemoryType};

// Get memory manager for current session
let memory = get_memory_manager(None)?;

// Store a memory
let memory_id = memory.store(
    "architect",
    "API Design Decision",
    "Decided to use REST API with JSON responses",
    MemoryType::Decision,
    8,
    &["api", "architecture"],
)?;

// Retrieve memories
let decisions = memory.retrieve(
    Some("architect"),
    Some(MemoryType::Decision),
    Some(7),
    None,
)?;

// Search memories
let results = memory.search("API design")?;
```

### Context Preservation

```rust
use amplihack_memory::context_preservation::{
    ContextPreserver, preserve_current_context, restore_latest_context,
};

// Preserve conversation context
let memory_id = preserve_current_context(
    "orchestrator",
    "Working on user authentication system",
    &["Using JWT tokens", "REST API pattern"],
    &["Design user model", "Create auth endpoints"],
)?;

// Restore context later
if let Some(context) = restore_latest_context("orchestrator")? {
    println!("Summary: {}", context.conversation_summary);
    println!("Tasks: {:?}", context.active_tasks);
}
```

## Integration Patterns

### Pattern 1: Agent Memory Integration

Each agent can maintain its own memory namespace for decisions, learnings, and context.

```rust
use amplihack_memory::{get_memory_manager, MemoryManager, MemoryType};
use chrono::Utc;
use serde_json::json;

pub struct ArchitectAgent {
    memory: Option<MemoryManager>,
    agent_id: String,
}

impl ArchitectAgent {
    pub fn new(session_id: Option<&str>) -> Self {
        let memory = get_memory_manager(session_id).ok();
        Self {
            memory,
            agent_id: "architect".to_string(),
        }
    }

    /// Store architectural decisions with full context.
    pub fn make_decision(&self, context: &str, decision: &str) -> Option<String> {
        let memory = self.memory.as_ref()?; // Graceful degradation
        memory.store(
            &self.agent_id,
            &format!("Decision: {context}"),
            decision,
            MemoryType::Decision,
            8,
            &["architecture", "decision"],
        ).ok()
    }

    /// Retrieve previous architectural decisions.
    pub fn recall_decisions(&self, context_search: Option<&str>) -> Vec<MemoryEntry> {
        let Some(memory) = self.memory.as_ref() else { return vec![] };
        memory.retrieve(
            Some(&self.agent_id),
            Some(MemoryType::Decision),
            Some(7),
            context_search,
        ).unwrap_or_default()
    }

    /// Store reusable patterns for future reference.
    pub fn learn_pattern(
        &self,
        pattern_name: &str,
        pattern_description: &str,
        usage_examples: &[String],
    ) -> Option<String> {
        let memory = self.memory.as_ref()?;
        let pattern_data = json!({
            "pattern_name": pattern_name,
            "description": pattern_description,
            "usage_examples": usage_examples,
            "learned_at": Utc::now().to_rfc3339(),
        });
        memory.store(
            &self.agent_id,
            &format!("Pattern: {pattern_name}"),
            &pattern_data.to_string(),
            MemoryType::Pattern,
            7,
            &["pattern", "reusable", &pattern_name.to_lowercase().replace(' ', "_")],
        ).ok()
    }
}
```

### Pattern 2: Workflow State Management

Track multi-step workflows across agent collaborations.

```rust
use amplihack_memory::context_preservation::ContextPreserver;
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct WorkflowManager {
    workflow_name: String,
    preserver: ContextPreserver,
}

impl WorkflowManager {
    pub fn new(workflow_name: &str, session_id: Option<&str>) -> Self {
        Self {
            workflow_name: workflow_name.to_string(),
            preserver: ContextPreserver::new(session_id),
        }
    }

    /// Initialize workflow state.
    pub fn start_workflow(
        &self,
        steps: &[&str],
        initial_context: Option<&str>,
    ) -> Result<String, MemoryError> {
        let current_step = steps.first().copied().unwrap_or("init");
        let pending: Vec<String> = steps.iter().skip(1).map(|s| s.to_string()).collect();
        self.preserver.preserve_workflow_state(
            &self.workflow_name,
            current_step,
            &[],
            &pending,
            &HashMap::new(),
            Some(json!({
                "started_at": Utc::now().to_rfc3339(),
                "initial_context": initial_context,
            })),
        )
    }

    /// Mark step as completed and advance workflow.
    pub fn complete_step(
        &self,
        step_name: &str,
        results: Value,
        next_step: Option<&str>,
    ) -> Option<String> {
        let state = self.preserver.restore_workflow_state(&self.workflow_name)?;

        let mut completed_steps = state.completed_steps.clone();
        completed_steps.push(step_name.to_string());
        let mut pending_steps = state.pending_steps.clone();

        let current_step = if let Some(ns) = next_step {
            if let Some(pos) = pending_steps.iter().position(|s| s == ns) {
                pending_steps.remove(pos);
            }
            ns.to_string()
        } else if !pending_steps.is_empty() {
            pending_steps.remove(0)
        } else {
            "completed".to_string()
        };

        let mut step_results = state.step_results.clone();
        step_results.insert(step_name.to_string(), results);

        self.preserver.preserve_workflow_state(
            &self.workflow_name,
            &current_step,
            &completed_steps,
            &pending_steps,
            &step_results,
            state.workflow_metadata,
        ).ok()
    }

    /// Get current workflow status.
    pub fn get_workflow_status(&self) -> Option<WorkflowStatus> {
        let state = self.preserver.restore_workflow_state(&self.workflow_name)?;
        let total = state.completed_steps.len() + state.pending_steps.len() + 1;
        let progress = state.completed_steps.len() as f64 / total as f64 * 100.0;

        Some(WorkflowStatus {
            workflow_name: self.workflow_name.clone(),
            current_step: state.current_step,
            progress_percentage: progress,
            completed_count: state.completed_steps.len(),
            pending_count: state.pending_steps.len(),
        })
    }
}
```

### Pattern 3: Session Context Preservation

Maintain conversation context across session boundaries.

```rust
use amplihack_memory::context_preservation::ContextPreserver;
use amplihack_memory::{MemoryType, MemoryError};
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Preserve comprehensive session context.
pub fn preserve_session_context(
    session_id: &str,
    context_data: &ContextData,
) -> Result<SessionContextResult, MemoryError> {
    let preserver = ContextPreserver::new(Some(session_id));

    // Store conversation context
    let context_id = preserver.preserve_conversation_context(
        "session_manager",
        &context_data.summary,
        &context_data.decisions,
        &context_data.tasks,
        Some(json!({
            "preserved_at": Utc::now().to_rfc3339(),
            "agent_count": context_data.agent_states.len(),
            "context_version": "1.0",
        })),
    )?;

    // Store individual agent states
    let mut agent_memory_ids = HashMap::new();
    for (agent_id, state) in &context_data.agent_states {
        let timestamp = Utc::now().format("%Y-%m-%d %H:%M").to_string();
        let mid = preserver.memory().store(
            agent_id,
            &format!("Agent State Snapshot - {timestamp}"),
            &serde_json::to_string_pretty(state)?,
            MemoryType::Context,
            7,
            &["agent_state", "snapshot", session_id],
        )?;
        agent_memory_ids.insert(agent_id.clone(), mid);
    }

    Ok(SessionContextResult {
        context_id,
        agent_memory_ids,
        session_id: session_id.to_string(),
    })
}

/// Restore complete session context.
pub fn restore_session_context(session_id: &str) -> Option<RestoredSessionContext> {
    let preserver = ContextPreserver::new(Some(session_id));

    // Restore conversation context
    let context = preserver.restore_conversation_context("session_manager")?;

    // Restore agent states
    let agent_snapshots = preserver.memory().retrieve(
        None,
        Some(MemoryType::Context),
        None,
        None,
    ).unwrap_or_default();

    let mut agent_states = HashMap::new();
    for snapshot in &agent_snapshots {
        if let Ok(state_data) = serde_json::from_str::<Value>(&snapshot.content) {
            agent_states.insert(snapshot.agent_id.clone(), state_data);
        }
    }

    Some(RestoredSessionContext {
        conversation_summary: context.conversation_summary,
        key_decisions: context.key_decisions,
        active_tasks: context.active_tasks,
        agent_states,
        preserved_at: context.preserved_at,
        session_id: session_id.to_string(),
    })
}
```

### Pattern 4: Agent Collaboration Memory

Enable agents to share context and build on each other's work.

```rust
use amplihack_memory::{get_memory_manager, MemoryManager, MemoryType};
use chrono::Utc;
use serde_json::{json, Value};

pub struct CollaborativeMemory {
    memory: Option<MemoryManager>,
}

impl CollaborativeMemory {
    pub fn new(session_id: Option<&str>) -> Self {
        let memory = get_memory_manager(session_id).ok();
        Self { memory }
    }

    /// Share insights between agents.
    pub fn share_insight(
        &self,
        from_agent: &str,
        to_agent: &str,
        insight_title: &str,
        insight_content: &str,
        context: Option<&str>,
    ) -> Option<String> {
        let memory = self.memory.as_ref()?;
        let insight_data = json!({
            "from_agent": from_agent,
            "to_agent": to_agent,
            "insight_title": insight_title,
            "insight_content": insight_content,
            "context": context,
            "shared_at": Utc::now().to_rfc3339(),
        });
        memory.store(
            from_agent,
            &format!("Insight for {to_agent}: {insight_title}"),
            &insight_data.to_string(),
            MemoryType::Context,
            6,
            &["collaboration", "insight", to_agent, from_agent],
        ).ok()
    }

    /// Get insights shared with a specific agent.
    pub fn get_insights_for_agent(&self, agent_id: &str, limit: usize) -> Vec<Value> {
        let Some(memory) = self.memory.as_ref() else { return vec![] };
        let insights = memory.retrieve(
            None,
            Some(MemoryType::Context),
            None,
            None,
        ).unwrap_or_default();

        insights.iter().filter_map(|insight| {
            let data: Value = serde_json::from_str(&insight.content).ok()?;
            if data.get("to_agent")?.as_str()? == agent_id {
                Some(data)
            } else {
                None
            }
        }).take(limit).collect()
    }

    /// Record collaborative work between agents.
    pub fn record_collaboration(
        &self,
        agents: &[&str],
        collaboration_type: &str,
        outcome: &str,
        artifacts: Option<&[String]>,
    ) -> Option<Vec<String>> {
        let memory = self.memory.as_ref()?;
        let collaboration_data = json!({
            "participating_agents": agents,
            "collaboration_type": collaboration_type,
            "outcome": outcome,
            "artifacts": artifacts.unwrap_or(&[]),
            "collaborated_at": Utc::now().to_rfc3339(),
        });

        let mut memory_ids = Vec::new();
        let mut tags: Vec<String> = vec![
            "collaboration".into(),
            collaboration_type.to_lowercase().replace(' ', "_"),
        ];
        tags.extend(agents.iter().map(|a| a.to_string()));

        for agent_id in agents {
            if let Ok(mid) = memory.store(
                agent_id,
                &format!("Collaboration: {collaboration_type}"),
                &collaboration_data.to_string(),
                MemoryType::Context,
                7,
                &tags.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            ) {
                memory_ids.push(mid);
            }
        }
        Some(memory_ids)
    }
}
```

## Performance Optimization

### Memory Manager Configuration

```rust
use amplihack_memory::{activate_memory, get_memory_manager, MemoryType};

// Disable memory for performance-critical operations
activate_memory(false);
// ... perform critical operations ...
activate_memory(true);

// Use batch operations for efficiency
let memory = get_memory_manager(None)?;
let batch_memories = vec![
    BatchEntry {
        agent_id: "agent1",
        title: "Batch Memory 1",
        content: "Content 1",
        memory_type: MemoryType::Context,
    },
    BatchEntry {
        agent_id: "agent2",
        title: "Batch Memory 2",
        content: "Content 2",
        memory_type: MemoryType::Decision,
    },
];
let memory_ids = memory.store_batch(&batch_memories)?;
```

### Query Optimization

```rust
use amplihack_memory::get_memory_manager;
use chrono::{Duration, Utc};

// Efficient memory retrieval patterns
let memory = get_memory_manager(None)?;

// Use specific filters to reduce result sets
let recent_decisions = memory.retrieve(
    Some("architect"),
    Some(MemoryType::Decision),
    Some(8),
    Some(10), // Always use limits for large datasets
)?;

// Use tags for fast categorization
let api_memories = memory.retrieve_by_tags(
    &["api", "design"],
    Some(20),
)?;

// Combine filters for precise queries
let critical_recent = memory.retrieve_filtered(
    Some(MemoryType::Decision),
    Some(9),
    Some(Utc::now() - Duration::hours(24)),
    Some(5),
)?;
```

## Error Handling and Graceful Degradation

### Robust Memory Operations

```rust
use amplihack_memory::{get_memory_manager, MemoryType};

/// Wrapper for safe memory operations with fallbacks.
fn safe_memory_operation<F, T>(memory_func: F) -> Option<T>
where
    F: FnOnce() -> Result<T, MemoryError>,
{
    match memory_func() {
        Ok(result) => Some(result),
        Err(e) => {
            eprintln!("Memory operation failed: {e}");
            None // or appropriate fallback
        }
    }
}

/// Agent pattern with memory fallback.
fn agent_with_memory_fallback(agent_id: &str, operation_data: &str) -> OperationResult {
    let memory = get_memory_manager(None).ok();

    // Primary operation with memory
    if let Some(mem) = &memory {
        match mem.store(
            agent_id,
            "Operation Result",
            operation_data,
            MemoryType::Context,
            5,
            &[],
        ) {
            Ok(memory_id) => return OperationResult { success: true, memory_id: Some(memory_id), fallback: false },
            Err(e) => eprintln!("Memory storage failed, continuing without: {e}"),
        }
    }

    // Fallback operation without memory
    OperationResult { success: true, memory_id: None, fallback: true }
}
```

### Environment-Specific Configuration

```rust
use std::env;
use amplihack_memory::activate_memory;

// Check if memory should be enabled
let memory_enabled = env::var("CLAUDE_MEMORY_ENABLED")
    .unwrap_or_else(|_| "true".to_string())
    .to_lowercase() == "true";

if memory_enabled {
    activate_memory(true);
} else {
    // Disable memory for this environment
    activate_memory(false);
}
```

## Integration with Claude Tools

### Hook Integration

```rust
use amplihack_memory::context_preservation::preserve_current_context;
use amplihack_memory::{get_memory_manager, MemoryType};
use chrono::Utc;
use serde_json::json;

/// Hook to preserve context before workflow execution.
fn pre_workflow_hook(workflow_context: &WorkflowContext) -> PreHookResult {
    let memory_id = preserve_current_context(
        "workflow_orchestrator",
        workflow_context.summary.as_deref().unwrap_or(""),
        &workflow_context.decisions,
        &workflow_context.tasks,
    ).ok();

    PreHookResult {
        context_preserved: memory_id.is_some(),
        memory_id,
    }
}

/// Hook to store workflow results.
fn post_workflow_hook(workflow_results: &WorkflowResults) {
    let Ok(memory) = get_memory_manager(None) else { return };
    if workflow_results.success {
        let _ = memory.store(
            "workflow_orchestrator",
            &format!("Workflow Completed: {}", workflow_results.workflow_name),
            &serde_json::to_string_pretty(workflow_results).unwrap_or_default(),
            MemoryType::Artifact,
            8,
            &["workflow", "completed", "results"],
        );
    }
}
```

### Tool Integration

```rust
use amplihack_memory::{get_memory_manager, MemoryType};
use chrono::Utc;
use serde_json::json;

/// Wrapper to add memory capabilities to existing tools.
fn enhanced_tool_with_memory(
    tool_name: &str,
    tool_args: &ToolArgs,
    agent_id: &str,
) -> ToolResult {
    // Execute original tool
    let tool_result = original_tool_function(tool_name, tool_args);

    // Store tool usage in memory
    if let Ok(memory) = get_memory_manager(None) {
        let _ = memory.store(
            agent_id,
            &format!("Tool Usage: {tool_name}"),
            &json!({
                "tool_name": tool_name,
                "arguments": tool_args,
                "result": &tool_result,
                "executed_at": Utc::now().to_rfc3339(),
            }).to_string(),
            MemoryType::Artifact,
            5,
            &["tool_usage", &tool_name.to_lowercase().replace(' ', "_")],
        );
    }

    tool_result
}
```

## Best Practices

### Memory Organization

1. **Use descriptive titles**: Make memories easily identifiable
2. **Tag consistently**: Develop a tagging taxonomy for your domain
3. **Set appropriate importance**: Use 1-10 scale meaningfully
4. **Organize hierarchically**: Use parent_id for related memories

### Performance Guidelines

1. **Use limits**: Always specify limits for large queries
2. **Filter aggressively**: Use multiple filters to reduce result sets
3. **Batch operations**: Use store_batch for multiple memories
4. **Clean up regularly**: Remove expired and unimportant memories

### Security Considerations

1. **Sensitive data expiration**: Set expires_in for sensitive memories
2. **Agent isolation**: Use agent namespacing appropriately
3. **Session boundaries**: Leverage session isolation for security
4. **Access patterns**: Monitor and audit memory access patterns

### Maintenance

```rust
use amplihack_memory::maintenance::MemoryMaintenance;

/// Perform weekly memory system maintenance.
fn weekly_maintenance() -> Result<(), MemoryError> {
    let maintenance = MemoryMaintenance::new()?;

    // Clean up expired memories
    let expired_count = maintenance.cleanup_expired()?;
    println!("Cleaned up {expired_count} expired memories");

    // Remove old sessions (older than 30 days)
    let old_sessions = maintenance.cleanup_old_sessions(30)?;
    println!("Removed {old_sessions} old sessions");

    // Optimize database
    maintenance.vacuum_database()?;
    maintenance.optimize_indexes()?;

    // Generate usage analysis
    let analysis = maintenance.analyze_memory_usage()?;
    println!("Memory usage analysis: {analysis:?}");

    Ok(())
}
```

## Troubleshooting

### Common Issues

**Import Errors**:

```rust
// Ensure correct Rust module path
use amplihack_memory::MemoryManager;
```

**Performance Issues**:

```rust
// Check database size and optimize
use amplihack_memory::maintenance::MemoryMaintenance;
let maintenance = MemoryMaintenance::new()?;
let stats = maintenance.analyze_memory_usage()?;
println!("Database size: {} bytes", stats.db_size_bytes);
```

**Memory Conflicts**:

```rust
// Use session isolation
let memory1 = get_memory_manager(Some("session_1"))?;
let memory2 = get_memory_manager(Some("session_2"))?;
// These operate in isolated namespaces
```

### Debug Mode

```rust
// Enable detailed logging
use tracing_subscriber;
tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).init();

// Check memory system status
use amplihack_memory::get_memory_manager;
match get_memory_manager(None) {
    Ok(memory) => {
        let stats = memory.get_session_summary()?;
        println!("Session stats: {stats:?}");
    }
    Err(_) => println!("Memory system not available"),
}
```

## Examples

See the `examples/` directory for complete working examples:

- `examples/agent_collaboration.rs` - Multi-agent memory sharing
- `examples/workflow_management.rs` - Complex workflow state tracking
- `examples/session_preservation.rs` - Cross-session context management
- `examples/performance_optimization.rs` - High-performance memory usage

## Support

For issues, questions, or contributions:

1. Check existing memories for similar issues
2. Review performance characteristics
3. Verify thread safety requirements
4. Consider graceful degradation patterns

The Agent Memory System is designed to enhance Claude agent capabilities while maintaining system reliability and performance.
