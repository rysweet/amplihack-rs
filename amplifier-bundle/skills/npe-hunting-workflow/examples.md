# NPE Hunting Examples

## Example 1: Activate from an observed stack trace

Prompt:

```text
Start from this NullPointerException stack trace, identify the exact null
producer, and then search for structurally similar bugs.
```

Expected behavior:

1. Anchor source lines to the reported revision.
2. Confirm the immediate null path before searching broadly.
3. Create the journal and candidate ledger.
4. Derive a lifecycle signature from the seed.
5. Keep causal hypotheses separate from confirmed facts.

The skill must not begin with a repository-wide search for `null`.

## Example 2: Reject plausible static matches

Prompt:

```text
I have twenty possible NPE sites. Review them before we fix anything.
```

Expected behavior:

1. Require producer, reachability, dereference, transition, and safe condition
   for every candidate.
2. Invoke `crusty-old-engineer` for every ledger entry.
3. Reject lazy recreation, short-circuited access, wrong bug classes, and
   teardown paths with no production caller.
4. Route source-reachable but unproven entries to targeted reproduction.
5. Start no fix workstream from static evidence alone.

## Example 3: Model lifecycle ordering and fix in parallel

Prompt:

```text
The validated NPE depends on a resource being replaced between animation open
and close. Prove the fix, then repair all validated bugs in parallel.
```

Expected behavior:

1. Invoke `tla-plus-expert` because resource identity and ordering are finite
   and modelable.
2. Produce a current-design counterexample.
3. Check whether a null-only guard leaves an acquisition/cleanup asymmetry.
4. Prove the proposed safety invariants within explicit model bounds.
5. Add characterization tests before production edits.
6. Partition validated bugs by files and shared interfaces.
7. Launch parallel `default-workflow` workstreams only for disjoint groups.
