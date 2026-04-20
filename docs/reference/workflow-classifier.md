# Workflow Classifier Reference

The workflow classifier routes user requests into one of four workflow types
based on keyword matching. Implemented in
`crates/amplihack-workflows/src/classifier.rs`.

## Workflow Types

| Type                    | Recipe                 | Purpose                          |
|-------------------------|------------------------|----------------------------------|
| `DEFAULT_WORKFLOW`      | `default-workflow`     | Build, fix, refactor, create     |
| `INVESTIGATION_WORKFLOW`| `investigation-workflow`| Research, analyze, explore       |
| `OPS_WORKFLOW`          | *(none)*               | Infrastructure ops, cleanup      |
| `Q&A_WORKFLOW`          | *(none)*               | Quick questions, explanations    |

## Keyword Tables

### DEFAULT (highest priority)

`implement`, `add`, `fix`, `create`, `refactor`, `update`, `build`,
`develop`, `remove`, `delete`, `modify`

### INVESTIGATION

`investigate`, `understand`, `analyze`, `research`, `explore`, `how does`,
`how it works`

### OPS

`run command`, `disk cleanup`, `repo management`, `git operations`,
`delete files`, `cleanup`, `organize files`, `clean up`,
`manage infrastructure`, `manage deployment`, `manage servers`,
`manage resources`

**Note**: OPS keywords are multi-word phrases to avoid false positives.
Single-word `"manage"` was removed in the #269 fix because it triggered
OPS classification for constructive tasks like "Add a feature to manage users".

### Q&A

`what is`, `explain briefly`, `quick question`, `how do i run`,
`what does`, `can you explain`

## Classification Algorithm

1. Extract all matching keywords from the request (case-insensitive substring match)
2. Check workflows in priority order: DEFAULT > INVESTIGATION > OPS > Q&A
3. First workflow with a matching keyword wins
4. **Constructive-verb override** (#269): If OPS matched but the request
   contains a constructive verb (`add`, `create`, `build`, `implement`,
   `write`, `design`, `develop`, `make`, `introduce`, `extend`, `enhance`,
   `refactor`), override to DEFAULT with confidence 0.75
5. If no keywords match, default to DEFAULT with confidence 0.5

## Confidence Scoring

| Condition                    | Confidence |
|------------------------------|------------|
| Multiple keywords matched    | 0.9        |
| Single keyword matched       | 0.7        |
| Constructive-verb override   | 0.75       |
| No keywords (ambiguous)      | 0.5        |
| Empty request                | 0.0        |

## Examples

| Request                                          | Classification | Reason                     |
|--------------------------------------------------|----------------|----------------------------|
| "implement a new feature for logging"            | DEFAULT        | keyword 'implement'        |
| "investigate why the tests are failing"          | INVESTIGATION  | keyword 'investigate'      |
| "disk cleanup of temp files"                     | OPS            | keyword 'disk cleanup'     |
| "what is the purpose of this module"             | Q&A            | keyword 'what is'          |
| "Add a feature to manage users"                  | DEFAULT        | keyword 'add' (priority)   |
| "manage infrastructure for production"           | OPS            | keyword 'manage infrastructure' |
| "Build a cleanup tool for database records"      | DEFAULT        | keyword 'build' (priority) + constructive override |
| "do something with the system"                   | DEFAULT        | ambiguous (0.5 confidence) |

## Provenance Logging

When a log directory is configured via `WorkflowClassifier::with_log_dir()`,
every classification decision is logged as a `ProvenanceEntry` for audit
and debugging.
