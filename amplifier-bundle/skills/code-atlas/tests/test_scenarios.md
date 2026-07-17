# Code Atlas — End-to-End Test Scenarios

**Purpose:** Validate the full atlas workflow against concrete codebases before considering the skill complete.

**Approach:** Each scenario specifies a codebase fixture, the command to run, and the exact outputs expected. These are acceptance tests — they must pass for the skill to ship.

---

## Scenario 1: Minimal Go API (Single Service)

**Fixture:** A Go HTTP service with:

- `cmd/server/main.go` — entry point, listens on `:8080`
- `internal/handlers/user_handler.go` — `GET /users`, `POST /users`
- `internal/models/user_model.go` — `User` struct
- `docker-compose.yml` — one service `api`, port `8080:8080`
- `.env.example` — `DATABASE_URL`, `PORT`
- `go.mod` — one external dep `github.com/gin-gonic/gin v1.9.1`

**Command:** `/code-atlas`

**Expected outputs:**

| File                                    | Must contain                            |
| --------------------------------------- | --------------------------------------- |
| `docs/atlas/repo-surface/topology.mmd`  | Node for `api` service, port `8080`     |
| `docs/atlas/compile-deps/deps.mmd`      | Node for `github.com/gin-gonic/gin`     |
| `docs/atlas/api-contracts/inventory.md` | Rows for `GET /users` and `POST /users` |
| `docs/atlas/data-flow/dataflow.mmd`     | `User` struct referenced                |
| `docs/atlas/inventory/env-vars.md`      | `DATABASE_URL` row, no value shown      |
| `docs/atlas/inventory/services.md`      | `api-service` row with port `8080`      |

**Must NOT contain:**

- Any value from `.env.example` (only key names)
- Nodes from test files (`*_test.go`)

---

## Scenario 2: TypeScript + Go Microservices (Multi-Service)

**Fixture:** Two services:

- `services/api/` — Express TypeScript API with `routes/user.routes.ts`, `dtos/user.dto.ts`
- `services/auth/` — Go auth service with `handler_login.go`, `login_request.go`
- `docker-compose.yml` — both services + postgres + redis

**Command:** `/code-atlas`

**Expected outputs:**

| Layer   | Expected content                                             |
| ------- | ------------------------------------------------------------ |
| Layer 1 | Both services, postgres, redis as nodes; inter-service edges |
| Layer 2 | Separate dependency graphs for TypeScript and Go services    |
| Layer 3 | Routes from TypeScript service + Go handler routes           |
| Layer 4 | `user.dto.ts` types + Go request struct traced               |
| Layer 5 | At least 2 user journeys derived from routes                 |
| Layer 6 | Both services in service inventory                           |

**Staleness check:** Change `services/api/routes/user.routes.ts`. Run `check-atlas-staleness.sh`.
Expected output: `Layer 3 STALE` (and not Layer 1 or Layer 2).

---

## Scenario 3: Python FastAPI (Single Service, Bug Hunt Focus)

**Fixture:** FastAPI service with a deliberate bug:

- Route `POST /api/orders` declares `CreateOrderRequest` with fields `{items, user_id}`
- Handler `order_handler.py` accesses `request.customer_id` (field not in DTO)
- `.env.example` declares `DATABASE_URL` and `STRIPE_KEY`
- Service only uses `DATABASE_URL` in code (STRIPE_KEY is orphaned)

**Command:** `/code-atlas` (with bug hunt enabled)

**Expected bug reports:**

| Bug                | Expected report                                                       |
| ------------------ | --------------------------------------------------------------------- |
| Route/DTO mismatch | `BUG-001`: handler accesses `customer_id` not in `CreateOrderRequest` |
| Orphaned env var   | `BUG-002`: `STRIPE_KEY` declared in `.env.example` but never used     |

**Pass criteria:**

- `docs/atlas/bug-reports/` contains at least 2 files
- Each file contains code evidence with file path and line number
- Neither bug report contains the actual value of `DATABASE_URL`

---

## Scenario 4: Staleness Detection — All 8 Layer Triggers

**Purpose:** Verify that each file pattern in the staleness trigger table correctly identifies the right layer.

**Test matrix:** For each row, change exactly one file and verify exactly one layer is reported stale.

| File changed                  | Expected stale layer | Must NOT report stale |
| ----------------------------- | -------------------- | --------------------- |
| `docker-compose.yml`          | Layer 1              | Layers 2–6            |
| `k8s/deployment.yaml`         | Layer 1              | Layers 2–6            |
| `helm/templates/service.yaml` | Layer 1              | Layers 2–6            |
| `go.mod`                      | Layer 2              | Layers 1, 3–6         |
| `services/web/package.json`   | Layer 2              | Layers 1, 3–6         |
| `internal/user_handler.go`    | Layer 3              | Layers 1–2, 4–6       |
| `src/api/routes.ts`           | Layer 3              | Layers 1–2, 4–6       |
| `src/dtos/user.dto.ts`        | Layer 4              | Layers 1–3, 5–6       |
| `internal/order_model.go`     | Layer 4              | Layers 1–3, 5–6       |
| `src/pages/checkout.page.tsx` | Layer 5              | Layers 1–4, 6         |
| `.env.example`                | Layer 6              | Layers 1–5            |

---

## Scenario 5: Publication Workflow

**Fixture:** Any multi-service codebase with a complete atlas already built.

**Command:** `/code-atlas publish=true`

**Expected:**

- `docs/atlas/` contains all 8 layer directories
- Each directory has at least one `.mmd` or `.dot` file and one `.svg` file
- `docs/atlas/README.md` exists and links to all 8 layers
- `docs/atlas/staleness-map.yaml` exists and contains at least 6 glob entries
- `docs/atlas/staleness-map.yaml` records a top-level `graph_backend:` field
  (one of `kuzu | lbug | neo4j | portable-cypher-only`)
- `docs/atlas/cypher/` exists and always contains `schema.cypher`, `atlas-layers.cypher`,
  `atlas-relationships.cypher`, and `queries.cypher` (portable graph — emitted regardless of backend)
- `docs/atlas/index.md` records the resolved `graph_backend` and `analyzer_mode`

**CI validation:**

```bash
# Run in CI after publish
for layer in repo-surface compile-deps api-contracts data-flow user-journeys inventory; do
    if [[ ! -d "docs/atlas/$layer" ]]; then
        echo "FAIL: Missing $layer" && exit 1
    fi
done
echo "All layer directories present."
```

---

## Scenario 6: Error Resilience (Partial Codebase)

**Fixture:** A repository with:

- No `docker-compose.yml` or Kubernetes manifests (Layer 1 source missing)
- No Python files (`code-visualizer`/`python-ast` adapter NOT selected — must not be required)
- Valid TypeScript routes (Layer 3 should succeed)

**Command:** `/code-atlas`

**Expected:**

- Layer 1: Skipped with `SkillError { code: "LAYER_SOURCE_NOT_FOUND", layer: 1 }`
- Layers 2, 3, 4, 5, 6: Completed normally
- compile-deps analyzer runs in `static-approximation` mode (no Python required); mode is recorded
- `completion_summary.errors` contains exactly one error for Layer 1
- Build does NOT halt on the Layer 1 error
- Portable graph still emitted under `docs/atlas/cypher/`; `graph_backend` recorded

---

## Scenario 7: Backend-Agnostic Graph — No Kuzu, No Python (Native Rust)

**Purpose:** Prove the graph representation is backend-agnostic. A build with NO kuzu and NO Python
available still succeeds, always emits the portable cypher graph with cross-layer links, and records
the selected backend (fail-visible, never a silent skip).

**Fixture:** A native Rust service (Simard-style, hard NO-kuzu / NO-Python policy):

- `Cargo.toml` + `src/main.rs` (axum), `src/routes.rs`, `src/dto.rs`
- No `kuzu` binary/package on PATH; no Python interpreter on PATH
- `lbug`/ladybug embedded store available in-process (optional)

**Command:** `/code-atlas`

**Expected:**

- Build **succeeds** (absence of kuzu/Python does NOT hard-fail)
- compile-deps analyzer runs in `rust-cargo-metadata` mode; label recorded (never `python-ast`)
- `graph_backend` resolves to `lbug` (if the embedded store is populated) or `portable-cypher-only`
  (if not) — and is recorded in BOTH `docs/atlas/index.md` and `docs/atlas/staleness-map.yaml`
- `docs/atlas/cypher/` is present with `schema.cypher`, `atlas-layers.cypher`,
  `atlas-relationships.cypher`, and `queries.cypher`
- `atlas-relationships.cypher` contains the inter-layer link relationships (`EXPOSES`, `USES_DTO`,
  `USES_ENV`, `TRAVERSES`, etc.) — cross-layer links are present in every backend
- `schema.cypher` uses the `lbug`/OpenCypher adapter (no `CREATE NODE TABLE` kuzu-only DDL required)

**Pass criteria:**

- No error asserts "Kuzu is required" or halts the build when kuzu is absent
- `grep graph_backend docs/atlas/staleness-map.yaml` returns a value from the allowed enum
- The portable graph is never silently dropped

---

## Acceptance Criteria

The skill is considered ready to ship when all 7 scenarios produce the described outputs without manual intervention. Automated scenarios (1–5, 7) must be run against fixture codebases in CI.

Run order: Scenario 1 → 7 (simpler to more complex). Scenario 4 (`test_staleness_triggers.sh`) must pass before Scenarios 2 and 3. Scenario 7 (backend-agnostic graph) must pass with neither kuzu nor Python installed.
