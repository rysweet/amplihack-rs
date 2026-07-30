# Step 2b Historical-Context Investigation Scope

> [Home](../index.md) > Reference > Step 2b Historical-Context Investigation Scope

Defines a future read-only Step 2b investigation that reconstructs relevant
workflow history without inspecting current repository content or changing
state.

## Clarified Requirements

```json
{
  "task_summary": "Define the scope for a future bounded, read-only Step 2b investigation of historical workflow context using only approved historical sources.",
  "explicit_requirements": [
    "Treat every requirement in this contract as non-optimizable.",
    "Use only the approved historical sources listed in this contract.",
    "Do not inspect current repository content during the investigation.",
    "Do not mutate repository, provider, workflow, or local state.",
    "Preserve unresolved unknowns and ambiguities instead of resolving them without evidence.",
    "Record verifiable evidence for every answered historical question.",
    "Stop before Phase 3 and do not invoke any recommended Phase 3 agent.",
    "Do not implement code, edit documentation, run tests, builds, benchmarks, services, or other workloads."
  ],
  "acceptance_criteria": [
    "Every historical question has either the required evidence and completion statement or an explicit unresolved status.",
    "Every cited source is an approved historical source and includes a stable identifier.",
    "The report distinguishes evidence, inference, assumption, ambiguity, and unknown.",
    "The activity performs no repository-content inspection outside approved historical sources.",
    "The activity leaves Git, files, provider objects, workflow state, processes, and external systems unchanged.",
    "The activity ends before Phase 3 with no implementation or workload execution."
  ],
  "out_of_scope": [
    "Inspection of the current working tree, source tree, configuration, generated files, dependencies, or documentation.",
    "Phase 3 execution or invocation of Phase 3 agents.",
    "Implementation, remediation, design changes, testing, building, benchmarking, deployment, or service execution.",
    "Mutation of local, repository, workflow, provider, or external state.",
    "Resolution of unknowns or ambiguities that approved historical evidence does not settle."
  ],
  "assumptions": [
    "The future investigator receives a specific task or decision whose relevant history must be reconstructed.",
    "Approved historical sources are available through read-only interfaces.",
    "Stable evidence identifiers can be recorded without checking out revisions or writing artifacts.",
    "Missing, inaccessible, conflicting, or inconclusive evidence is a valid outcome."
  ],
  "questions_resolved": [
    "Historical context means prior intent, decisions, changes, regressions, and outcomes relevant to the supplied task.",
    "Repository inspection excludes current-tree reads and filesystem searches but permits read-only Git object queries that do not check out or materialize files.",
    "No state mutation includes no commits, branches, tags, stashes, worktrees, files, caches, comments, labels, issues, pull requests, workflow dispatches, or persistent background processes.",
    "Completion requires bounded evidence collection, not a definitive answer when the historical record is incomplete.",
    "Recommended Phase 3 roles are planning output only and must not be invoked."
  ],
  "estimated_complexity": "medium",
  "classification": "other"
}
```

## Non-Optimizable Requirements

The future activity must preserve all requirements in the JSON contract. It
may reduce neither the evidence standard nor any safety boundary to save time,
tokens, or tool calls.

## Approved Historical Sources

Only these read-only sources are permitted:

- Existing Git objects and refs queried without checkout or mutation, using
  read-only operations equivalent to `git log`, `git show`, `git diff`,
  `git blame`, `git tag --list`, and `git branch --list`.
- Existing GitHub issues, pull requests, reviews, comments, commits, releases,
  and workflow-run metadata queried through read-only `gh` or API operations.
- Existing immutable workflow logs or artifacts already identified by the
  task input or by approved GitHub historical metadata.

Each citation must include a stable identifier such as a commit SHA, tag,
issue or pull-request number, review URL, workflow-run ID, artifact ID, or
timestamped log entry. An approved source may expose historical file content
only through an existing Git object; it does not authorize reading the current
working tree.

## Explicit Exclusions

The future investigator must not:

- Read, list, search, index, hash, or otherwise inspect current repository
  files or directories, including source, tests, configuration, documentation,
  generated output, and dependency metadata.
- Use repository search, code intelligence, language servers, filesystem
  globbing, or commands such as `find`, `ls`, `rg`, `grep`, or `cat` against
  repository content.
- Check out, restore, reset, cherry-pick, rebase, merge, fetch, pull, clone, or
  materialize a historical revision.
- Create or modify files, refs, branches, tags, commits, stashes, worktrees,
  indexes, caches, issues, pull requests, comments, labels, reviews, releases,
  workflow runs, or external records.
- Run code, hooks, tests, linters, formatters, builds, package managers,
  benchmarks, servers, containers, migrations, deployment tools, or any other
  workload.
- Enter Phase 3, perform implementation, or invoke an agent.

## Historical Questions and Completion Evidence

| Historical question | Required verifiable evidence | Completion criterion |
| --- | --- | --- |
| What original problem or goal introduced the relevant behavior? | The earliest relevant issue, pull request, commit, or release reference, including stable identifiers and quoted or summarized rationale. | The origin is evidenced, or the earliest reachable record is identified and the true origin is marked unknown. |
| Which decisions and constraints shaped the behavior? | Decision-bearing issue or PR discussion, reviews, commit messages, or release notes with stable identifiers. | Each claimed decision maps to evidence; conflicts and gaps remain explicit. |
| How did the behavior evolve over time? | An ordered sequence of relevant commits, pull requests, releases, or workflow runs. | The sequence identifies material transitions and their evidence without claiming unobserved causality. |
| Which regressions, failures, or reversals occurred? | Historical issue reports, failed workflow-run metadata, revert commits, follow-up fixes, or review findings. | Each event records impact and disposition when evidenced; missing outcomes remain unknown. |
| What prior approaches were attempted, accepted, rejected, or superseded? | Competing pull requests, commits, review threads, or decision records. | Each approach has an evidence-backed status; silence is not interpreted as rejection. |
| Which constraints still appear applicable to the supplied task? | Historical statements linked to later confirming or superseding records. | Constraints are labeled current-looking, superseded, conflicting, or unknown; current repository inspection is not used to decide. |
| What unresolved historical ambiguities affect a later Phase 3? | Missing links, conflicting accounts, inaccessible records, or evidence gaps with stable references where available. | Every unresolved item states what is unknown, why the approved evidence cannot resolve it, and what later evidence would be needed. |

## Investigation Depth

Use **standard depth**:

1. Trace the earliest reachable origin.
2. Follow material decision and change points.
3. Check linked regressions, reversals, and superseding records.
4. Triangulate consequential claims with two independent historical records
   when available.
5. Do not broaden into unrelated history or inspect current repository content.

## Assumptions, Unknowns, and Ambiguities

Reasonable assumptions may guide search order but may not become findings
without evidence. Label each conclusion as **evidenced**, **inferred**,
**assumed**, **ambiguous**, or **unknown**.

Preserve these unresolved variables until the future task supplies or evidence
settles them:

- The exact feature, defect, component, decision, or time range of interest.
- Which historical records exist, are accessible, or are authoritative.
- Whether historical intent still matches current behavior.
- Whether a missing record means no decision occurred.
- Whether correlation between historical events proves causation.

## Stopping Conditions

Stop the investigation when the first applicable condition is met:

- Every historical question satisfies its completion criterion.
- Remaining questions require a prohibited source or state mutation.
- Approved sources are exhausted and remaining gaps are recorded as unknown or
  ambiguous.
- Evidence conflicts cannot be resolved from approved sources.
- The next action would enter Phase 3, invoke an agent, inspect current
  repository content, implement a change, or run a workload.

The final output is a read-only evidence report. It must contain citations,
findings, preserved unknowns, and the stopping condition reached. It must not
contain implementation output or claim that Phase 3 ran.

## Recommended Phase 3 Roles

These roles are recommendations only. Do not invoke them during Step 2b:

- **Primary: `knowledge-archaeologist`** — validate historical intent and
  evolution against the evidence report.
- **Secondary: `architect`** — map evidenced constraints into a future design
  scope after repository inspection is separately authorized.
- **Tertiary: `patterns`** — compare evidenced decisions with established
  project patterns in a later phase.
- **Conditional specialist** — select only after Step 2b identifies a domain
  need; examples include `security`, `database`, or `integration`.

Phase 3 must receive the complete non-optimizable requirements, evidence
citations, unknowns, ambiguities, assumptions, exclusions, and stopping
condition unchanged.
