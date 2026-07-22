# ApiWright Wiki

ApiWright is a local-first IDE for API requests and executable API tests. This wiki is the product manual; it complements the versioned schemas and contributor guide.

## Start here

- [Getting started](getting-started.md) — install, create a project and run the demo.
- [GUI reference](gui-reference.md) — shell, explorer, editor, tools, menus and shortcuts.
- [Settings](settings.md) — appearance, HTTP, editor, view and persistence.
- [Project model](project-model.md) — folders, inheritance, files and Git.
- [Request and sidecars](request-format.md) — request JSON, assertions and hooks.
- [Catalog](catalog.md) — reusable built-ins and project assets.
- [OpenAPI](openapi.md) — completion, validation, coverage and generators.
- [Authentication](authentication.md) — bearer/basic providers and refresh.
- [AI Advisor](advisor.md) — context assembly, redaction and configuration.
- [Protocols](protocols.md) — HTTP, GraphQL, WebSocket, SSE and gRPC.
- [Import and export](import-export.md) — lossless bundles, snippets and migrations.
- [Secrets and security](security.md) — local state, masking and project code.
- [Jira integration](jira.md) — ticket links, live details and comments from the project tree.
- [Licensing and billing](licensing.md) — Free, Pro and Enterprise plans and license activation.
- [CLI and CI](cli.md) — deterministic local and pipeline execution.
- [Architecture](architecture.md) — ports, adapters and dependency direction.
- [Development](development.md) — build, test, repository conventions and extension points.
- [Release](release.md) — CI gates, version tags, packages, updater and rollback.
- [Troubleshooting](troubleshooting.md) — common validation, auth and CI failures.

## Design principles

1. A project is ordinary files and folders, so Git remains the source of truth.
2. Behavior is reusable: reference catalog assets instead of copying scripts.
3. The GUI and CLI share the same core execution semantics.
4. Defaults should make a new project runnable without path configuration.
5. Secrets and local state never belong in committed project files.
