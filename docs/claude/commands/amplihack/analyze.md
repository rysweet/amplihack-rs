---
name: amplihack:analyze
disable-model-invocation: true
version: 1.0.0
description: Comprehensive code analysis and philosophy compliance review
triggers:
  - "analyze this code"
  - "check philosophy compliance"
  - "review for simplicity"
  - "assess architecture"
examples:
  - "/analyze ./src/module"
  - "/analyze src/api"
---

# Analyze Command

## Input Validation

@~/.amplihack/.claude/context/AGENT_INPUT_VALIDATION.md

## Usage

`/analyze <PATH>`

## Audit

Run the `code-philosophy` audit on the supplied path. Follow that skill for the audit criteria and report format.
