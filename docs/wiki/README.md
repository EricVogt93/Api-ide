# Forge Wiki

Forge is a local-first IDE for API requests and executable API tests. This wiki is the product manual; it complements the versioned schemas and contributor guide.

## Start here

- [Getting started](getting-started.md) — install, create a project and run the demo.
- [Project model](project-model.md) — folders, inheritance, files and Git.
- [Request and sidecars](request-format.md) — request JSON, assertions and hooks.
- [Catalog](catalog.md) — reusable built-ins and project assets.
- [OpenAPI](openapi.md) — completion, validation, coverage and generators.
- [Authentication](authentication.md) — bearer/basic providers and refresh.
- [AI Advisor](advisor.md) — context assembly, redaction and configuration.
- [CLI and CI](cli.md) — deterministic local and pipeline execution.
- [Architecture](architecture.md) — ports, adapters and dependency direction.

## Design principles

1. A project is ordinary files and folders, so Git remains the source of truth.
2. Behavior is reusable: reference catalog assets instead of copying scripts.
3. The GUI and CLI share the same core execution semantics.
4. Defaults should make a new project runnable without path configuration.
5. Secrets and local state never belong in committed project files.
