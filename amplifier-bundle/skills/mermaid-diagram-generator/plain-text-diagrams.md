# Plain-text diagrams

Use plain-text diagrams in terminals, code comments, and commit messages where Mermaid does not render.

## Guidelines

- Use Unicode box-drawing characters such as ┌─┐│└┘├┤┬┴┼.
- Show direction clearly with arrows such as ◄─► ▲▼.
- Choose symbols with a clear meaning, such as 🧱 📦 🔌 🔒 ⚠️ ✓.
- Keep spacing and alignment consistent.

## Levels of detail

1. Overview. Show the main connections, as in `Frontend ◄──► Backend ◄──► Database`.
2. Components. Add technology details and connections.
3. Implementation. Show internal structure and interfaces.

## System overview

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   Frontend      │    │   API Gateway   │    │   Backend       │
│   (React)       │◄──►│   (Express)     │◄──►│   (Python)      │
│                 │    │                 │    │                 │
│  - UI Components│    │  - Route Handler│    │  - Business     │
│  - State Mgmt   │    │  - Auth         │    │    Logic        │
└─────────────────┘    └─────────────────┘    └─────────────────┘
```

## Pipeline

```
Input Data
    │
    ▼
┌─────────────┐
│  Validator  │ ◄─── Raw data cleaning
└─────────────┘
    │
    ▼
┌─────────────┐
│ Transformer │ ◄─── Business logic
└─────────────┘
    │
    ▼
┌─────────────┐
│   Storage   │ ◄─── Persistence
└─────────────┘
    │
    ▼
Output Data
```
