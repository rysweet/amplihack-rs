---
name: gherkin-expert
version: 1.0.0
description: Gherkin/BDD expert for writing, reviewing, and refactoring Cucumber feature files and step definitions.
auto_activates:
  - "write feature file"
  - "write gherkin"
  - "BDD scenarios"
  - "cucumber feature"
  - "acceptance criteria as gherkin"
  - "review feature files"
explicit_triggers:
  - /gherkin
  - /amplihack:gherkin-expert
confirmation_required: false
skip_confirmation_if_explicit: true
token_budget: 3000
---

# Gherkin Expert Skill

Expert guidance for writing, reviewing, and refactoring Gherkin feature files
and Cucumber step definitions following BDD best practices.

## Capabilities

- **Author** well-structured `.feature` files with proper Given/When/Then flow
- **Review** existing feature files for anti-patterns and readability issues
- **Refactor** imperative scenarios into declarative specifications
- **Design** step definitions with appropriate parameterization
- **Organize** feature files with tags, backgrounds, and scenario outlines

## Usage

Activate explicitly with `/gherkin` or automatically when working with `.feature`
files, BDD scenarios, or acceptance criteria.

## Style Guide

Structure features with Feature, Background, Scenario and Scenario Outline,
and steps with Given, When, Then, And and But.

### Preferred: Declarative

```gherkin
Scenario: Registered user logs in
  Given a registered user "alice"
  When the user logs in with valid credentials
  Then the user should see the dashboard
```

### Avoid: Imperative

```gherkin
Scenario: Login
  Given I am on the login page
  When I type "alice" into the username field
  And I type "password123" into the password field
  And I click the "Login" button
  Then I should see the text "Dashboard"
```

## Anti-Pattern Checklist

1. **Multiple behaviors per scenario** — split into focused scenarios
2. **Missing assertions** — every scenario needs a meaningful Then
3. **UI-coupled steps** — describe behavior, not button clicks
4. **Giant Background** — keep shared setup minimal
5. **Scenario Outline abuse** — use only when examples vary by data
6. **Inconsistent domain language** — align with ubiquitous language
7. **Test scripts** — feature files used as test scripts rather than specifications

## Behavioural Rules

- Move steps shared by every scenario into Background.
- Keep Given, When and Then in strict order: no When before Given.

## Step Definition Design

- Make step definitions reusable, with parameters where steps differ only by value.
- Use Cucumber expressions, or regular expressions where they are needed.
- Keep step definitions loosely coupled to each other.
- Use data tables and doc strings for structured or long inputs.

## Organisation and Structure

- Name feature files consistently and lay out directories by domain.
- Tag suites, for example `@smoke`, `@regression` and `@wip`.
- Choose between feature-level and scenario-level Background with care.
- Manage the state that steps share explicitly.

## Living Documentation

- Write features that non-technical stakeholders can read.
- Balance precision with readability.
- Use the domain's ubiquitous language consistently.
- Structure features around user stories and acceptance criteria.

## Related Skills

- **property-based-testing**: Generalizes concrete Gherkin examples into
  universally-quantified properties (invariant, round-trip, metamorphic).
  Reach for it when a `Scenario Outline`'s example table is really trying to
  express "for all inputs" rather than a handful of cases.
- **tla-plus-expert**: Formal design-level verification; together these form
  the specification-and-verification triad.
