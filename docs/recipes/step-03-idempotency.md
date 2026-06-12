# step-03-create-issue: Host-Aware Tracking Idempotency

`step-03-create-issue` is documented in the reference section:

- [Step 03 Host-Aware Tracking Idempotency](../reference/recipe-step-03-idempotency.md)
- [Provider-Aware Workflow Prep Reference](../reference/dual-provider-workflow.md)

The current contract is host-aware:

| Host type | Tracking path |
| --------- | ------------- |
| `github` | GitHub issue reuse, search, create, and label setup through `gh` |
| `azdo` | Azure Boards reuse/create, with structured local metadata fallback |
| `other` | Structured local metadata only |

GitHub issue and label commands are valid only in the `github` branch. Azure
DevOps and unsupported remotes never fall through to `gh issue` or `gh label`
commands.
