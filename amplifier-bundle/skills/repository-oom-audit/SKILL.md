---
name: repository-oom-audit
description: Performs exhaustive repository-wide OutOfMemoryError and memory-retention audits with tracked-file inventory, systematic risk taxonomy, anchored evidence, severity and confidence, reviewed non-issues, remediation, and validation. Use for OOM investigations, memory leak audits, unbounded growth reviews, resource-lifecycle audits, allocation-amplification analysis, or JVM/native memory risk assessments.
---

# Repository OOM Audit

## Purpose

Finds heap, native-memory, thread, file-descriptor, and retained-graph conditions
that can exhaust a process. Produces an evidence-backed report without confusing
lexical matches, plausible risks, and confirmed defects.

This is an audit method, not a pattern dump. Every tracked file receives a
screening disposition, every positive finding is traced through ownership and
cleanup, and important non-issues remain visible as coverage evidence.

## Safety Invariants

1. Record the exact commit before analysis.
2. Keep the target repository read-only unless the user separately authorizes fixes.
3. Do not commit, push, open issues, or create pull requests unless explicitly asked.
4. Preserve unrelated work in dirty checkouts.
5. Store generated inventories and reports outside the target when repository policy
   forbids audit artifacts.
6. Do not copy substantial source into reports; cite and paraphrase.
7. Treat runtime profiling as optional and non-invasive. Never collect or expose
   sensitive heap contents without explicit authorization.

## Completion Standard

An audit is complete only when:

- the commit and repository state are recorded;
- every tracked file has a disposition or explicit assessment limitation;
- every taxonomy category has a coverage result, including zero-result categories;
- heap, requested-array, native/direct, thread-stack, metaspace/class-loader,
  finalization/reference, and container/process memory domains are addressed;
- every candidate is classified as confirmed, plausible, or reviewed non-issue;
- every actionable finding has anchored evidence, severity, confidence, trigger,
  retained-memory analysis, remediation, and test guidance;
- exclusions, unavailable sources, and runtime limitations are explicit; and
- inventory and report consistency checks pass.

## Workflow

### 1. Freeze identity and constraints

Record:

```bash
git rev-parse HEAD
git status --short
git remote -v
git submodule status
```

Capture repository guardrails, generated/vendor policy, authorized output location,
build system, runtime versions, and whether source modifications are forbidden.
Line citations refer to this commit even if the working tree later changes.

### 2. Build the exhaustive inventory

Use Git as the scope authority:

```bash
git ls-files -z
```

Classify every path by:

- language and file type;
- production, test, tooling, generated, vendored, configuration, or binary;
- module/component;
- screening disposition; and
- limitation or exclusion reason.

Required dispositions:

| Disposition | Meaning |
|---|---|
| `screened-no-candidate` | Taxonomy screening found no relevant construct |
| `candidate-traced` | One or more matches were manually traced |
| `reviewed-non-issue` | A bound, cleanup path, finite domain, or unreachable path was proven |
| `generated-boundary-reviewed` | Generated file was represented by its generator/parser boundary |
| `vendored-reviewed` | Vendored code was screened and ownership was recorded |
| `assessment-limited` | Access, format, size, tooling, or policy prevented assessment |

Do not silently exclude tests, tooling, generated files, or vendored code. They can
cause test-runner OOMs, ship into production, or reveal unsafe producer/consumer
contracts. Grouping is allowed only when each path remains present in the inventory.

Validate inventory completeness by comparing the sorted inventory path column with a
fresh `git ls-files` result. The symmetric difference must be empty.

### 3. Map memory ownership and input boundaries

Identify long-lived roots:

- process and class statics;
- application/session/request objects;
- event loops, UI trees, actors, and service containers;
- executor workers, timers, and thread locals;
- native handles and callback registries;
- class loaders, plugins, reflection metadata, and generated-code caches.

Identify growth inputs:

- user files, uploads, clipboard, drag-and-drop, and command arguments;
- network responses, sockets, messages, subprocess output, and logs;
- archive entries, serialized graphs, parser nodes, media dimensions, and metadata;
- internal event rates, edit histories, retries, and concurrent task submission.

Trace: **source -> allocation/growth -> owner -> retention root -> cleanup/bound -> concurrency multiplier**.

### 4. Screen the complete taxonomy

Use language-aware semantic/code-intelligence tools first, then repository search.
Search terms are candidate generators, never proof.

| Category | Required questions |
|---|---|
| Collections and graphs | Can lists, maps, sets, trees, registries, indexes, pools, dedup tables, or parent/child graphs grow for process/session lifetime? |
| Queues and backpressure | Are queues bounded? What happens when producers outpace consumers or consumers block? Are retry/dead-letter queues bounded? |
| Caches | Is cardinality finite? Are size/weight, TTL, eviction, weak references, invalidation, and class-loader lifecycles correct? |
| Listeners and callbacks | Does every registration have an owner and removal path? Can repeated activation register duplicates? Do publishers outlive subscribers? |
| Histories and logs | Are undo, audit, telemetry, diagnostics, console capture, event journals, snapshots, and recent-value lists bounded by count and weight? |
| Executors and threads | Are pools/queues bounded and shut down? Can platform threads, scheduled tasks, futures, timers, subprocess drainers, or per-request executors accumulate? |
| Thread locals | Are values removed on pooled threads? Can values retain requests, class loaders, buffers, security context, or graphs? |
| Metaspace and code generation | Can class loaders, generated classes, proxies, scripts, plugins, hot reload, reflection metadata, or compiler outputs accumulate? Are unload boundaries real and observable? |
| Finalization and references | Can finalizers, cleaners, phantom/reference queues, or deferred native cleanup fall behind allocation? Are reference-processing threads starved? |
| Whole-input reads | Are file, user, network, process, and archive reads bounded before `readAll`, string conversion, or in-memory materialization? |
| Buffers and chunking | Does "streaming" retain all chunks? Are builders, byte buffers, collectors, joins, copies, decompression, or encoding pipelines bounded? |
| Memory mapping | What maps files, who unmaps/closes channels, and can address-space/native mappings accumulate? |
| Images and graphics | Are dimensions/pixel counts checked before decode? Are encoded and decoded copies concurrent? Are graphics, rasters, textures, and image caches disposed? |
| Audio and video | Are duration, channels, sample rate, frame size, tracks, and decoded buffers bounded? Are players/codecs/streams released? |
| Archives | Are entry count, per-entry bytes, total expanded bytes, nesting, path uniqueness, and compression ratio limited? |
| Serialization | Are graph depth, references, array lengths, object types, and total bytes filtered before object allocation? |
| XML and JSON | Are document bytes, depth, nodes, strings, arrays, names, entity processing, and numeric declarations limited? Streaming APIs can still build unbounded models. |
| Sockets and streams | Are timeouts, message/frame sizes, pending writes, connection counts, and close paths present on success, failure, cancellation, and timeout? |
| Native resources | Are direct buffers, JNI allocations, GPU resources, file descriptors, font handles, codecs, and OS objects explicitly released on the owning thread/context? |
| Amplification | How many simultaneous representations exist: compressed, encoded, bytes, chars, tokens, DOM, model, decoded media, copies, and output? |
| Integer arithmetic | Can count, dimension, stride, frame, or byte-size arithmetic overflow before allocation? Is checked wide arithmetic used? |
| Cleanup and eviction | Do `close`, `dispose`, `shutdown`, `cancel`, `remove`, `clear`, and eviction run on all paths and actually sever strong references? |
| Concurrency accumulation | Multiply per-item memory by workers, requests, threads, retries, listeners, and in-flight stages. Are check-then-act races able to bypass limits? |
| Process and container budget | Does heap plus metaspace, code cache, thread stacks, direct/native buffers, mapped files, GPU/JNI memory, and safety margin fit the actual OS/container limit? |

Record zero matches explicitly. Also search aliases and framework-specific equivalents
for each detected language and dependency.

### 5. Trace and classify every candidate

Read declarations, constructors, mutators, owners, call sites, error paths, cancellation,
shutdown, tests, and configuration. Verify whether limits are hard, configurable,
attacker/user controlled, count-only, or weight-aware.

Classification:

- **Confirmed defect:** Static evidence proves a reachable unbounded/missing-lifecycle
  mechanism under stated conditions. Runtime reproduction strengthens but is not required.
- **Plausible risk:** The mechanism exists, but reachability, cardinality, ownership,
  lifetime, input trust, or practical trigger needs confirmation.
- **Reviewed non-issue:** Evidence proves a finite domain, effective bound/eviction,
  complete lifecycle, weak ownership, streaming behavior, or non-production isolation.

Never promote a regex match to a finding. Never omit a significant reviewed non-issue
merely because it is not actionable.

### 6. Score severity and confidence separately

Severity:

| Level | Meaning |
|---|---|
| Critical | Low-effort remote/untrusted trigger can reliably exhaust shared production capacity |
| High | User-controlled or routine workload can exhaust heap/native capacity or permanently retain large graphs |
| Medium | Sustained, unusual, privileged, or lifecycle-specific use can cause material growth |
| Low | Small fixed leak, test/tool-only impact, or difficult trigger with limited scope |
| Info | Hardening or observability recommendation without demonstrated exhaustion |

Confidence:

- **High:** Direct code and ownership evidence; trigger and missing bound/cleanup are clear.
- **Medium:** Strong mechanism evidence with one unresolved lifecycle/input assumption.
- **Low:** Indirect signal requiring call-path or runtime confirmation.

### 7. Quantify retention and amplification

For each finding, estimate:

- retained objects and their root;
- per-item or per-task weight;
- growth variable and practical maximum;
- simultaneous representations/copies;
- native stack/direct/GPU/file-handle contribution;
- concurrency multiplier; and
- whether GC can reclaim the graph.

Use checked arithmetic. Distinguish retained leaks from transient peak amplification:
both can produce OOM, but remediation differs.

Derive a provisional concurrency ceiling from measured peak retained/native bytes per
task and the memory budget remaining after heap baseline, JVM/native overhead, and a
safety margin. Do not select pool sizes from CPU count alone when tasks retain large
images, documents, archives, or response graphs.

### 8. Use authoritative evidence

Prioritize current primary sources:

1. language and runtime specifications/API documentation;
2. framework/library/vendor documentation;
3. operating-system and file-format specifications;
4. maintained security/performance guidance from authoritative organizations;
5. reputable secondary sources only when primary guidance is unavailable.

Record source title, version, URL, and the claim it supports. If user-provided sources
or runtime artifacts are unavailable, say so without inventing their contents.

### 9. Recommend behavior-compatible remediation

Recommendations must:

- preserve documented behavior or identify the intentional behavior change;
- add characterization tests before changing ambiguous behavior;
- bound by memory weight as well as count where item sizes vary;
- define overflow, rejection, coalescing, spill, or eviction semantics;
- close resources on success, exception, cancellation, timeout, and shutdown;
- avoid silent drops and broad exception swallowing; and
- include observability for limits, queue depth, evictions, rejections, and peak usage.

### 10. Validate without destabilizing the target

Static validation:

- inventory symmetric difference is empty;
- every inventory row has a disposition;
- every taxonomy row has a result;
- every citation resolves at the audited commit;
- every positive finding has all required fields;
- confirmed/plausible/non-issue counts match the report;
- exclusions and limitations are explicit; and
- the target Git status has not changed.

Optional runtime validation:

- reproduce under a deliberately small heap or memory limit;
- use JFR, heap histograms/dumps, native-memory tracking, descriptor counts, and
  thread dumps when safe;
- compare at least two equivalent post-GC snapshots or recordings across repeated
  workload cycles, including dominator and GC-root-path differences;
- correlate live-set growth with thread, class-loader, queue, cache, direct-buffer,
  file-descriptor, and process-RSS trends rather than assuming every OOM is a heap leak;
- compare dominators, retained sizes, queue/cache cardinality, and resource counts
  before and after repeated operations; and
- clean up all temporary data and processes.

Runtime failure to reproduce does not disprove a statically confirmed unbounded
mechanism; document environment, workload, duration, and observed ceiling.

## Required Report Format

```markdown
# OOM and Memory-Retention Audit

## Audit identity
- Repository, commit, branch, dirty state, runtime/tool versions

## Scope and inventory
- Counts by production/test/tooling/generated/vendored
- Inventory artifact path/hash
- Per-file dispositions and limitations

## Method and taxonomy coverage
- Search/analysis tools
- Category result table, including zero matches

## Confirmed defects
### [ID] [Title]
- Classification
- Severity / confidence
- Evidence: repository-relative path:line-range, anchored to commit
- Trigger
- Retained memory and root
- Growth/amplification/concurrency analysis
- Existing bound/cleanup and why it fails
- Behavior-compatible remediation
- Test and runtime validation
- Authoritative sources

## Plausible risks
- Same fields, with unresolved assumptions stated

## Reviewed non-issues
- Candidate, evidence, and proven bound/cleanup/finite scope

## Coverage gaps and limitations
- Inaccessible files, generated/vendor treatment, unavailable sources,
  omitted runtime work, environment constraints

## Prioritized remediation plan
- Order by severity, confidence, effort, and behavior risk
```

## Evidence Quality Gate

Reject or downgrade a finding when:

- the citation is only an import, declaration, or lexical match;
- no retention root or peak-allocation path is identified;
- a cleanup/eviction path was not inspected;
- the trigger is impossible under the input contract;
- the collection domain is demonstrably finite;
- test-only code is presented as production reachable;
- severity conflates impact with confidence; or
- claims rely on unavailable sources.

The final report should be reproducible by another reviewer at the recorded commit.
