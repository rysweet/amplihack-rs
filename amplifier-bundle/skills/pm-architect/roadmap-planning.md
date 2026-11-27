# Roadmap Planning

The project roadmap lives in `.pm/roadmap.md`. Use this structure.

```markdown
# Project Name Roadmap

## Project Overview
**Type:** cli-tool | **Quality Bar:** balanced | **Initialized:** 2025-11-21

## Primary Goals

### Goal 1: Implement Configuration System
**Status:** In Progress (60%) | **Target:** Q1 2026
**Milestones:** [x] Design schema [x] YAML parser [ ] Validation [ ] Tests
**Supporting Work:** BL-001 (DONE), BL-004 (IN_PROGRESS)

## Current Focus
This quarter: Configuration and testing foundation

## Recent Completions
- Config parser (BL-001), 2025-11-21

## Upcoming Milestones
- Config validation complete, target 2025-11-28
```

## Create or Update the Roadmap

To create the roadmap, read `.pm/config.yaml`, give each primary goal a set of milestones, and record its initial status and target. Write the result to `.pm/roadmap.md`.

To update it, read the current roadmap, mark completed milestones, adjust targets, and add finished work to Recent Completions. Refresh the goal statuses and percentages before saving.

## Track Goal Progress

Read the roadmap and backlog together. An item supports a goal when its title or description contains a word from that goal, using the match in [Goal Alignment Score](REFERENCE.md#goal-alignment-score-10-weight).

For each goal, count each done milestone as one and each in-progress milestone as half. Divide by the total milestones, multiply by 100, cap at 100, and round to a whole percent. Record zero progress for a goal with no milestones. Report each goal's progress, target, risks, and blockers.

## Strategic Recommendations

- For a goal at risk, prioritize the work that closes its near-term milestone gap.
- When a goal is complete, record the achievement on the roadmap and shift focus to the next goal.
- When goals compete, look for independent work that can advance both in parallel.
