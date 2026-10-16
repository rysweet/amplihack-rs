# default-workflow `max_total_steps`

`amplifier-bundle/recipes/default-workflow.yaml` sets
`recursion.max_total_steps` to the recursive step total (the workflow plus every
referenced sub-recipe, counted per call, including condition-skipped steps) plus
roughly 25% headroom, rounded to a clean number. Currently the total is 92 and
the cap is 120.

A cap below the real total aborts the run with a step-limit error, which made
`step-22-ensure-mergeable` fail even though earlier phases completed (#1553).
`max_depth` and other limits are unchanged.

## Regression guard

```bash
cargo test -p amplihack-cli --test issue_1553_max_total_steps_cap
```

The test sums steps recursively (resolving sub-recipes only inside the recipes
directory, rejecting `..` and path separators, failing on cycles) and asserts
`max_total_steps >= total` and that the cap stays within 2x the total. When you
add steps, raise the cap if this test fails.
