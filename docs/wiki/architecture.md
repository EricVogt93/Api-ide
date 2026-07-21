# Architecture

Forge follows a hexagonal split:

```text
GUI (egui) ─┐
CLI         ├── ports/adapters ── forge-core ── domain + execution
Advisor     ┘
```

`forge-core` owns request models, schemas, OpenAPI matching, environments, assets, scripting, transport orchestration, history and import/export. `forge-gui` adapts those capabilities to egui and keeps transient view state. `forge-cli` adapts them to deterministic terminal execution. The Advisor transport is isolated behind its own provider adapter.

Keep dependency direction inward: core must not import GUI or CLI code. Add behavior to core when it affects execution semantics; add UI or CLI code only for presentation and orchestration. File-backed formats and schemas are compatibility boundaries, so version changes require migration and tests.
