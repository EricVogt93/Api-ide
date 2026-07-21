# Architecture

Forge has a pragmatic hexagonal structure: native UI and command-line entry
points depend inward on a GUI-free core. The crate boundary is strict; inside
`forge-core`, application logic and concrete infrastructure live in focused
modules rather than behind a trait for every dependency.

```text
                    inbound adapters
             ┌─────────────┴─────────────┐
             │                           │
      forge-gui (egui)            forge-cli (clap)
             │                           │
             └──────────► forge-core ◄───┘
                              │
        ┌─────────────────────┼──────────────────────┐
        ▼                     ▼                      ▼
   filesystem/SQLite     HTTP + protocols      script runtimes
```

No module in `forge-core` imports either binary crate. Both adapters use the
same public request models, resolution rules, execution engine and result
types, which prevents GUI-only execution semantics.

## Workspace crates

| Crate | Responsibility | Must not own |
| --- | --- | --- |
| `forge-core` | Formats, domain models, validation, orchestration, transport, persistence, assertions, scripting and OpenAPI | egui widgets, dialogs, CLI parsing or terminal presentation |
| `forge-cli` | clap commands, root/target selection, output formatting and process exit codes | alternate request semantics |
| `forge-gui` | egui state, panels, editors, dialogs, background bridge and desktop update UX | duplicated validation or runner rules |

The dependency graph is enforced naturally by Cargo: `forge-cli` and
`forge-gui` depend on `forge-core`; the core manifest has no reverse
dependency on either adapter.

## Core module map

`forge-core/src` is organized by capability:

| Module | Role |
| --- | --- |
| `reqv1` | Current `project.json` request format, sidecars, assets, canonical IR, matrices, sequences, auth sessions, mocks, bundles and migration |
| `model`, `store`, `runner` | Legacy `forge.json` workspace model, file tree and sequential suite runner |
| `exec` | Shared concrete HTTP engine, request/response types, cookies and OAuth helpers |
| `assert` | Legacy assertion evaluation, extraction and JSON Schema validation reused by other capabilities |
| `script` | Resource-limited Rhai and JavaScript hosts for hooks and request scripts |
| `vars` | Legacy variable scopes, interpolation and dynamic values |
| `openapi` | OpenAPI parsing, request skeletons, bindings and contract assertions |
| `protocols` | GraphQL introspection, unary gRPC, WebSocket and SSE sessions |
| `convert` | cURL, Postman and Bruno conversion plus language snippets |
| `history` | SQLite-backed execution history, search and response diffing |

The two request generations deliberately coexist. Current projects use
`reqv1`; legacy workspaces remain loadable through `store`/`runner` and can be
converted through `reqv1::migrate`. They share lower-level HTTP and selected
assertion/script facilities but have different persisted models.

## Request-v1 execution path

The current engine turns Git-friendly files into a canonical request before
any network activity:

```text
*.request.json
  + sibling *.hooks.json / *.assertions.json
  + project.json / inherited environment / secret provider
          │
          ▼
parse → reference resolution → bindings/generators → interpolation
      → canonical ResolvedRequest IR
      → beforeRequest pipeline
      → HttpEngine.execute OR in-process mock
      → afterResponse → onError? → finally
      → assertions + runtime writes + diagnostics + masked result
```

1. `load_request_document` parses the request and merges optional sibling
   hook and assertion documents into one effective pipeline without changing
   the persisted request body.
2. `load_request_environment` applies the explicit environment or the nearest
   inherited folder/request selection. The CLI supplies secrets through a
   closure backed by `.env.local` and then process environment variables.
3. `RefResolver` resolves built-ins, aliases and relative assets, rejects path
   escape, and applies exact-before-longest-prefix alias precedence.
4. `DataStore` caches JSON data per build, clones before request-local JSON
   Patch, detects reference cycles and validates sibling schemas.
5. `build_ir` topologically resolves binding dependencies, runs generators,
   interpolates the `env`, `bindings`, `matrix`, `runtime` and `secret` scopes,
   and collects independent diagnostics. This stage is pure with respect to
   network I/O and backs `forge validate`.
6. The runner optionally obtains a cached project-auth token, executes
   `beforeRequest`, sends through `HttpEngine` or renders the request mock,
   executes `afterResponse`, conditionally executes `onError`, and always
   executes `finally`.
7. Assertion failures produce `RunStatus::Failed`; transport, resolution or
   asset errors produce `RunStatus::Error`. Secret values captured while
   building IR are masked from public assertions, runtime values and
   diagnostics.

A standalone request expands its matrix. A sequence carries extractor output
into the next request as `${runtime.*}`. A batch executes files independently,
but can share the HTTP cookie jar and auth session supplied by the adapter.

The complete compatibility contract lives in
[Request Format v1](../architecture/request-format-v1.md) and the versioned
JSON Schemas under `schemas/`.

## Legacy runner path

`Workspace::load` reads `forge.json`, environments and the ordered collection
tree. `runner::run` then plans a request/folder/collection/workspace scope,
loads CSV or JSON iterations, and executes sequentially. For each iteration it
creates a fresh runtime-variable map, applies inherited variables and auth,
runs suite and request scripts, executes HTTP, evaluates assertions, applies
extractors and streams `RunEvent` values. The CLI consumes those events for
terminal output and optional JUnit; the GUI consumes the same events for its
test-results panels.

## HTTP and protocol boundary

`exec::HttpEngine` is a concrete core service built on `reqwest`/Tokio. It
owns a cookie jar and caches clients by TLS, proxy and authentication-relevant
settings. It handles cancellation, timing, redirects, bodies, cookies, client
certificates, custom roots, Digest, NTLM and SigV4. GraphQL, gRPC, WebSocket
and SSE live in `protocols`, sharing TLS-material rules where appropriate.

This is the main deliberate deviation from strict textbook hexagonal
architecture: there is no `HttpPort` or `WorkspaceRepository` trait. File I/O,
SQLite and HTTP implementations are inside the core crate. Local test servers
and temporary directories test those real boundaries. Introduce a port only
when a second implementation or an otherwise untestable boundary justifies
it; a one-implementation interface would add ceremony without isolation.

The secret-provider function is already a narrow injected boundary, and
`CancellationToken` plus event channels let adapters control long-running
work without importing presentation concerns.

## GUI adapter and concurrency

`ForgeApp` owns synchronous egui state. It never performs request network I/O
on the render thread. `Bridge` starts one dedicated thread with a Tokio
runtime and a shared `HttpEngine`/request-v1 `AuthSession`:

```text
egui action → Cmd channel → bridge task → forge-core
    ▲                                      │
    └── repaint + Evt channel ◄────────────┘
```

Commands cover runs, cancellation, protocol sessions, OpenAPI fetches,
catalog previews, Advisor calls, cookies and update checks. Events carry
typed results back; the app drains them once per frame and ignores stale run
IDs. Cookie state is restored per workspace and persisted after execution.

The Advisor is a GUI-side outbound adapter because it is optional desktop
integration, not request execution. The GUI assembles bounded, redacted
context and `advisor.rs` talks to an OpenAI-compatible provider. Core remains
the authority for parsing, OpenAPI matching and execution data included in
that context.

## Persistence boundaries

- Versioned project inputs are normal JSON/YAML/JS files intended for Git.
- Request behavior split into `*.request.json`, `*.assertions.json` and
  `*.hooks.json` is merged only at load time.
- `.env.local`, `.forge-local/`, `.forge/` and `*.secrets.json` are local or
  generated state and ignored by default.
- `history` uses SQLite; cookies, UI state and Advisor configuration stay
  outside committed request files.
- Bundle import validates relative paths and all collisions before writing;
  reference resolution canonicalizes paths and rejects project escape.

## Change placement rules

Use the narrowest existing layer:

- Execution semantics, persisted format behavior, diagnostics or reusable
  generation belong in `forge-core` with a core test.
- CLI target selection, text output and exit mapping belong in `forge-cli`.
- Layout, interaction and transient state belong in `forge-gui`; background
  I/O is routed through `bridge.rs`.
- A format change requires schema and migration consideration, stable
  serialization, fixture coverage and documentation.
- Do not make the CLI call GUI code or let panels parse an alternative model
  of a request.

See [Development](development.md) for focused tests and repository checks.
