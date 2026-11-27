---
name: pm-architect
description: "Expert project manager for software projects: prioritizes a backlog, prepares delegation packages for coding agents, tracks concurrent workstreams and keeps a roadmap. Activates when managing projects, coordinating work, or tracking overall progress."
explicit_triggers:
  - /top5
---

# PM Architect Skill

## Role

Manage the backlog, prepare work for coding agents, track concurrent workstreams, and keep the roadmap current.

## When to Activate

Activate when the user:

- Mentions managing projects or coordinating work
- Asks about project status or progress
- Wants to organize multiple projects or features
- Needs help with project planning or execution
- Says "I'm losing track" or "What should I work on?"
- Asks "What are the top priorities?" or invokes `/top5`
- Wants a quick daily standup or status overview

## Activities

- To add backlog items and recommend priorities, read the Multi-Criteria Scoring Algorithm, Dependency Analysis, and Backlog Schema in [REFERENCE.md](REFERENCE.md).
- To select an agent and prepare its package, read [delegation.md](delegation.md).
- To track workstream capacity, conflicts, stalls, and status changes, read Coordination Patterns and Workstream Schema in [REFERENCE.md](REFERENCE.md).
- To plan milestones, track goal progress, and update the roadmap, read [roadmap-planning.md](roadmap-planning.md).

## Core Workflow

1. Identify which activities the user's request needs.
2. Read the relevant guidance and inspect the project state.
3. Combine the results into one response with recommended next steps.

## Patterns

### What Should I Work On?

Rank backlog items using [REFERENCE.md](REFERENCE.md) and use [roadmap-planning.md](roadmap-planning.md) to relate the recommendations to current goals.

### Check Overall Status

Combine active workstream health from [REFERENCE.md](REFERENCE.md) with goal progress from [roadmap-planning.md](roadmap-planning.md) in one project status report.

### Start New Work

Prepare the package using [delegation.md](delegation.md), then record the workstream using the Workstream Schema in [REFERENCE.md](REFERENCE.md).

### Initialize PM

Create the `.pm/` structure using State File Schemas in [REFERENCE.md](REFERENCE.md) and create the project roadmap using [roadmap-planning.md](roadmap-planning.md).

### Top 5 Priorities (`/top5`)

Aggregate priorities from GitHub issues, PRs, and the local backlog into a ranked Top 5. Give each item a score breakdown, its source, and a suggested next action.

Weight GitHub issues at 40%, GitHub PRs at 30%, roadmap alignment at 20%, and the local backlog at 10%.

### Daily Standup

Combine git activity, workstream health, backlog changes, and roadmap progress into a cross-project status report.

## Navigation Guide

| File                                       | Read when                                                        |
| ------------------------------------------ | ---------------------------------------------------------------- |
| [REFERENCE.md](REFERENCE.md)               | You need scoring, dependency analysis, coordination, or schemas. |
| [EXAMPLES.md](EXAMPLES.md)                 | You want to see a project management workflow in use.            |
| [delegation.md](delegation.md)             | You are preparing work for a coding agent.                       |
| [roadmap-planning.md](roadmap-planning.md) | You are creating, updating, or reviewing a roadmap.              |

## Success Criteria

Users can manage projects, prioritize work, delegate to agents, track progress, and align work with goals effectively.
