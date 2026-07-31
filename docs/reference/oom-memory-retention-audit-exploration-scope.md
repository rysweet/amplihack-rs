# OOM and Memory-Retention Audit Exploration Scope

> [Home](../index.md) > Reference > OOM and Memory-Retention Audit Exploration Scope

This read-only Step 2b artifact defines a standard-depth exploration for a
future reusable repository OOM and memory-retention audit skill under
`amplifier-bundle/skills`. It defines investigation scope only: it does not
implement the skill, profile or benchmark a target, audit defects, or propose
remediation.

## Repository-grounded findings

| Area | Finding | Evidence |
| --- | --- | --- |
| Existing memory scope | Silent-degradation Category G covers unbounded collections, retained allocations, periodic cleanup, health monitoring, artifacts, and subprocess lifecycle. It is a partial precedent, not an exhaustive OOM taxonomy. | `amplifier-bundle/skills/silent-degradation-audit/category_agents/resource-accumulation.md:29-177` |
| Static evidence | Existing checks are source-structural: long-lived fields, recurring mutations, missing shrink operations, cleanup call sites, and resource lifecycle handling. | `amplifier-bundle/skills/silent-degradation-audit/category_agents/resource-accumulation.md:31-170` |
| Runtime precedent | No repository source defines a universal audit duration, warm-up, cadence, sample count, workload, or repetition minimum. | `amplifier-bundle/skills/silent-degradation-audit/category_agents/resource-accumulation.md:22-27`; `amplifier-bundle/skills/dynamic-debugger/SKILL.md:209-214,312-317` |
| Incident duration | Simard's 1 GiB-to-32 GiB growth over 16-24 hours is incident context, not a normative minimum. | `amplifier-bundle/skills/silent-degradation-audit/category_agents/resource-accumulation.md:22-27` |
| Degraded evidence | Existing audit contracts require unavailable tooling and reduced coverage to be disclosed rather than silently treated as clean. | `amplifier-bundle/skills/supply-chain-audit/reference/contracts.md:300-312` |
| Finding dimensions | Existing schemas provide precedent for severity, confidence, evidence, and voting, but do not cleanly separate every required dimension. Independent fields are therefore a new design requirement. | `amplifier-bundle/skills/silent-degradation-audit/validation_panel/voting-rules.md:12-31,58-125`; `amplifier-bundle/skills/quality-audit/reference.md:355-403` |
| Inconclusive handling | Existing validation preserves partial or unparseable results and stops safely when all validation is unusable. | `docs/reference/quality-audit-cycle-recipe.md:96-174` |
| Scope schema | The investigation preparation recipe defines the exact `scope_definition` keys and value domains. | `amplifier-bundle/recipes/investigation-prep.yaml:124-143` |
| Plan schema | The investigation exploration recipe defines the exact `exploration_plan` keys and agent layout. | `amplifier-bundle/recipes/investigation-explore.yaml:56-83` |

Current authoritative sources were sufficient for the scope decisions. Git
history is not required for this exploration; use it only if a future
investigation finds a material requirement that current sources cannot settle.

## Requirements and understanding criteria

The future skill must:

1. Inventory tracked files and give every applicable file a screening result
   or an explicit exclusion rationale.
2. Extend Category G with an explicit taxonomy covering caches, queues,
   listeners, histories, executors, thread-locals, retained graphs, buffering,
   mappings, archives, parsers, sockets, native resources, amplification,
   overflow, eviction, disposal, and concurrent accumulation.
3. Require anchored static evidence and preserve static findings when runtime
   tooling is unavailable.
4. Keep triage, severity, confidence, evidence provenance, evidence status,
   and validation consensus independent.
5. Require target-justified runtime protocols and define no universal
   hours-based minimum.
6. Distinguish a reviewed non-issue from inconclusive runtime evidence.
7. State composition and hand-off contracts explicitly.
8. Synchronize each registration surface according to its actual authority.
9. Validate frontmatter, discovery, coverage accounting, taxonomy coverage,
   schema completeness, evidence anchors, internal references, degraded
   behavior, and independent review.
10. If a recipe is added, use unique step IDs and only supported `bash`,
    `agent`, or `recipe` step types, as required by
    `crates/amplihack-cli/src/commands/recipe/parse.rs:77-94`.

The scope is understood when a future implementer can explain owned failure
classes, distinguish mandatory static evidence from optional runtime
corroboration, account for every applicable file, classify findings without
conflating dimensions, define a safe target-specific runtime protocol, route
findings without duplicate ownership, identify applicable registration
surfaces, and demonstrate clean, degraded, and inconclusive validation paths.

## Evidence expectations and minimum runtime

Static review is mandatory. Runtime evidence is optional corroboration and
must never suppress an anchored static finding.

There is no repository-grounded universal minimum runtime. Optional runtime
evidence is admissible only when its record declares:

- target, environment, workload, and lifecycle exercised;
- baseline and warm-up rule;
- observation window, sample cadence, and sample count;
- lifecycle repetitions or cycles;
- tools, versions, and memory metric;
- resource and time ceilings and stop conditions;
- expected and observed behavior; and
- limitations and reproduction instructions.

The observation window must cover the declared warm-up and enough relevant
lifecycle boundaries to evaluate the suspected retention mechanism. Every
numeric value needs target-specific justification. Unavailable, unsafe,
unjustified, or non-reproducing runtime work is `inconclusive` or `degraded`,
not evidence of absence.

Each finding must preserve these independent dimensions:

| Dimension | Required meaning |
| --- | --- |
| `triage` | `confirmed`, `plausible`, or `reviewed-non-issue` |
| `severity` | Consequence if the condition occurs |
| `confidence` | Certainty that the interpretation is correct |
| `evidence_provenance` | `static`, `runtime`, or `both` |
| `evidence_status` | `complete`, `degraded`, `inconclusive`, or `not-run` |
| `validation_consensus` | Independent reviewer outcome; never a substitute for confidence |

## Composition boundaries

| Component | Boundary |
| --- | --- |
| Future OOM skill | Owns deep taxonomy-driven tracked-file static screening, explicit coverage accounting, triage, and evidence-anchored reporting. Exhaustiveness is a new requirement rather than an existing audit convention. |
| `silent-degradation-audit` | Retains broad wave-based resource-accumulation coverage. Escalate to it when memory growth participates in broader silent masking or degradation. |
| `quality-audit` | May orchestrate or consume findings, but is not required for standalone use. |
| `optimizer` | May receive measured findings for remediation or comparison. It is excluded from this skill's initial discovery, not prohibited from discovery repository-wide. |
| `dynamic-debugger` | Provides optional, explicit runtime corroboration for a specific hypothesis. It is never mandatory; its process-access, sensitive-data, and resource cautions apply. |

These boundaries follow
`amplifier-bundle/skills/silent-degradation-audit/SKILL.md:35-50,58-86`,
`amplifier-bundle/skills/quality-audit/SKILL.md:77-86`,
`amplifier-bundle/skills/quality-audit/reference.md:122,826`,
`amplifier-bundle/agents/core/optimizer.md:4-18`, and
`amplifier-bundle/skills/dynamic-debugger/SKILL.md:1-33,289-310`.

## Registration synchronization

| Surface | Authority and required treatment |
| --- | --- |
| `amplifier-bundle/skills/<name>/SKILL.md` | Functional source for directory-scan discovery; a valid skill file registers automatically. |
| `SkillCatalog` | Scans bundled skill directories; no per-skill Rust registry edit is required (`crates/amplihack-domain-agents/src/skill_catalog.rs:56-93`). |
| Install staging | Copies skills generically; no skill-specific change is required unless new binaries, hooks, or assets are introduced (`crates/amplihack-cli/src/commands/install/verification.rs:66-85`; `crates/amplihack-cli/src/commands/install/mod.rs:531-541`). |
| `docs/skills/SKILL_CATALOG.md` | Human-facing synchronization surface. Update its entry and counts; the current tree provides no general generator enforcement. |
| `amplifier-bundle/bundle.md` | Separate Amplifier-native manifest. Update when the skill supports that path and validate it independently. |
| `docs/claude/skills/` | Partial mirror with no universal synchronization requirement. Add no mirror unless explicitly adopted. |

## Recorded but unasked questions

| Question | Scope decision |
| --- | --- |
| Is runtime profiling mandatory? | No. |
| Is 16-24 hours a minimum? | No; it is incident context only. |
| Can unavailable runtime evidence imply clean? | No. |
| Are severity and confidence interchangeable? | No. |
| Is validator consensus the same as confidence? | No. |
| Are all registration surfaces equally authoritative? | No. |
| Is a `docs/claude/skills` mirror mandatory? | No. |
| Is optimizer prohibited from repository-wide discovery? | No; exclusion applies only to this future skill's initial discovery. |

## Known unknowns

- Current sources define no target-independent runtime, cadence, warm-up, or
  lifecycle-cycle defaults.
- The final confidence scale and calibration thresholds remain to be designed.
- Most of the expanded OOM taxonomy lacks worked in-repository examples.
- Maintenance of `docs/skills/SKILL_CATALOG.md` is undocumented.
- Whether the future skill includes a recipe or companion executable is
  undecided.
- Existing `bundle.md` and catalog claims may drift; implementation must
  validate current state rather than copying it.

## Boundaries and standard depth

**In scope:** future skill contracts, taxonomy ownership, evidence rules,
finding dimensions, adjacent-skill composition, registration paths, and
validation design.

**Out of scope:** implementation, repository defect screening, profiling,
benchmarking, stress workloads, remediation, issues, and correction of
existing registration drift.

Standard depth means tracing each core question through the current
authoritative contract and its direct implementation or validation surface,
checking one relevant precedent where available, recording contradictions and
unknowns, and stopping once every success criterion has repository evidence or
an explicit gap. It does not expand into defect discovery or historical
research unless current sources leave a material requirement unresolved.

## Phase 3 agents

- **Primary: `amplihack:analyzer`** for code, contracts, registration, runtime
  evidence, and composition.
- **Secondary: `amplihack:patterns`** for reusable audit and validation
  precedents.
- **Validation only: `amplihack:reviewer`** for question-by-question
  completeness and evidence review.
- **History:** no dedicated agent by default; use a historical specialist only
  when current authoritative sources cannot resolve a material requirement.

## Exact `scope_definition` schema artifact

```json
{
  "core_questions": [
    "Which OOM and memory-retention failure classes must the future skill own?",
    "Which evidence is mandatory static evidence and which runtime evidence is optional?",
    "How must triage, severity, confidence, evidence provenance, evidence status, and validation consensus remain independent?",
    "What are the composition boundaries with silent-degradation-audit, quality-audit, optimizer, and dynamic-debugger?",
    "Which registration surfaces are authoritative, derived, optional, or independently synchronized?",
    "Which validations prove coverage, schema integrity, registration, and degraded behavior?"
  ],
  "success_criteria": [
    "Responsibility, evidence, composition, registration, and validation contracts are explicitly defined.",
    "No universal runtime duration is asserted without repository evidence.",
    "Every proposed requirement is either repository-supported or clearly identified as a new design requirement.",
    "The future implementation can be evaluated without profiling or auditing repository defects during scope definition."
  ],
  "in_scope": [
    "Repository-root code and contract investigation",
    "amplifier-bundle/skills conventions and related audit skills",
    "OOM and memory-retention taxonomy ownership",
    "Mandatory static and optional runtime evidence rules",
    "Independent finding dimensions",
    "Composition boundaries",
    "Registration and installation paths",
    "Validation requirements"
  ],
  "out_of_scope": [
    "Implementation of the future skill",
    "Profiling, benchmarking, stress testing, or runtime experiments",
    "Auditing actual repository memory defects",
    "Remediation or issue creation",
    "User questions"
  ],
  "known_unknowns": [
    "Target-independent numeric runtime defaults are not defined by current repository sources.",
    "The future confidence scale has no direct repository precedent.",
    "The expanded taxonomy lacks complete in-repository examples.",
    "SKILL_CATALOG generation or maintenance is undocumented.",
    "Recipe and companion-tool inclusion remain future design choices."
  ],
  "estimated_complexity": "high",
  "estimated_depth": "standard",
  "has_ambiguities": false,
  "ambiguities": [],
  "recommended_agents": {
    "primary": "amplihack:analyzer",
    "secondary": "amplihack:patterns",
    "tertiary": "amplihack:reviewer",
    "specialist": null
  }
}
```

## Exact `exploration_plan` schema artifact

```json
{
  "exploration_areas": [
    {
      "area": "OOM and retention responsibility",
      "agent": "amplihack:analyzer",
      "focus": "Map memory-growth failure classes, lifecycle signals, evidence requirements, and ownership boundaries from current skills and conventions.",
      "expected_output": "Responsibility and evidence matrices with repository file-and-line support.",
      "priority": 1
    },
    {
      "area": "Reusable precedents",
      "agent": "amplihack:patterns",
      "focus": "Extract applicable patterns from DISCOVERIES, PATTERNS, resource-accumulation, audit contracts, and validation panels.",
      "expected_output": "Reusable patterns, conflicts, gaps, and cautions for the proposed skill.",
      "priority": 2
    },
    {
      "area": "Runtime evidence contract",
      "agent": "amplihack:analyzer",
      "focus": "Determine defensible duration, sampling, lifecycle, safety, and inconclusive rules while separating mandatory static from optional runtime evidence.",
      "expected_output": "A justified runtime-evidence tier model or a documented unresolved requirement.",
      "priority": 3
    },
    {
      "area": "Composition and registration",
      "agent": "amplihack:analyzer",
      "focus": "Trace adjacent-skill boundaries and authoritative discovery, staging, catalog, manifest, and mirror surfaces.",
      "expected_output": "A composition matrix and authoritative registration synchronization map.",
      "priority": 4
    },
    {
      "area": "Independent validation",
      "agent": "amplihack:reviewer",
      "focus": "Validate completeness, evidence quality, dimension independence, boundary clarity, and validation coverage.",
      "expected_output": "Question-by-question validation with unanswered items and evidence weaknesses.",
      "priority": 5
    }
  ],
  "parallel_deployment": {
    "primary_agent": "amplihack:analyzer",
    "primary_focus": "Code, contracts, registration paths, runtime evidence rules, and composition boundaries.",
    "secondary_agent": "amplihack:patterns",
    "secondary_focus": "Repository discoveries, patterns, audit precedents, and reusable validation conventions.",
    "tertiary_agent": "amplihack:reviewer",
    "tertiary_focus": "Independent completeness and evidence validation against explicit requirements.",
    "specialist_agent": null,
    "specialist_focus": null
  },
  "priority_order": [
    "OOM and retention responsibility",
    "Reusable precedents",
    "Runtime evidence contract",
    "Composition and registration",
    "Independent validation"
  ],
  "potential_dead_ends": [
    "Auditing actual repository memory defects",
    "Running profilers, debuggers, benchmarks, or stress workloads",
    "Treating the 16-24 hour incident duration as a universal minimum",
    "Conflating severity with confidence or validation consensus",
    "Expanding into general performance optimization",
    "Assuming all registration surfaces have identical authority",
    "Reviewing history before exhausting current authoritative sources"
  ],
  "verification_approach": "Trace each requirement to current repository file-and-line evidence; build responsibility, evidence, registration, and validation matrices; require reviewer confirmation that every core question is answered without implementation, profiling, benchmarking, or defect-audit activity.",
  "historical_context_needed": false
}
```
