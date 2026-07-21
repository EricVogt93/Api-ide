# Development Guide

Forge is a Rust 2021 workspace built with the stable toolchain declared in
`rust-toolchain.toml`. Cargo resolver v2 covers three crates: `forge-core`,
`forge-cli` and `forge-gui`.

## Local setup

Install Rust through rustup; the repository toolchain also installs
`rustfmt` and `clippy`. Linux GUI builds need the same native packages as CI:

```sh
sudo apt-get install cmake pkg-config libgl1-mesa-dev libwayland-dev \
  libx11-dev libxcursor-dev libxi-dev libxkbcommon-dev libxrandr-dev
```

Core and CLI work do not require the desktop windowing headers.

## Build and run

```sh
cargo build --workspace
cargo run -p forge-gui --bin forge-ide
cargo run -p forge-cli -- --help
cargo build --release --locked --workspace
```

Use the demo as an offline end-to-end check. Its request-v1 mocks avoid an
external API dependency:

```sh
cargo run -p forge-cli -- ci requests \
  --root examples/demo-workspace --env demo --mock --allow-project-code
```

The demo also contains a legacy `forge.json` collection for compatibility
testing.

## Required checks

Run the focused test while iterating, then the repository gate before opening
a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo check --release --locked -p forge-gui --bin forge-ide
```

These are the checks in `.github/workflows/ci.yml`. CI runs on pushes to
`development` and `main` and on every pull request. Concurrent runs for the
same ref cancel the older run.

## Focused tests

Most behavior is covered in `crates/forge-core/tests`; unit tests stay beside
small internal helpers. Useful focused commands include:

```sh
cargo test -p forge-core --test reqv1_test
cargo test -p forge-core --test runner_test
cargo test -p forge-core --test auth_exec_test
cargo test -p forge-core --test openapi_test
cargo test -p forge-core --test protocols_test
cargo test -p forge-gui updater::tests
```

Test area ownership:

| Area | Primary tests |
| --- | --- |
| Request-v1, assets, sidecars and sequences | `reqv1_test.rs`, module tests under `reqv1/` |
| Legacy planning, chaining and JUnit | `runner_test.rs` |
| HTTP, TLS and authentication | `exec_test.rs`, `tls_test.rs`, `auth_exec_test.rs` |
| Assertions and variables | `assert_test.rs`, `vars_test.rs` |
| OpenAPI and conversions | `openapi_test.rs`, `convert_test.rs`, `postman_test.rs`, `bruno_test.rs` |
| gRPC, WebSocket, SSE and GraphQL | `grpc_test.rs`, `protocols_test.rs` |
| Demo and shipped documentation contracts | `docs_test.rs` |

Use `#[tokio::test]` for async flows, `wiremock` or a local in-process server
for network behavior, and `tempfile` for persistence. Tests must not rely on a
public service, user home directory or committed secrets. There is currently
no configured coverage threshold.

## Code and format conventions

- Accept `rustfmt` defaults and four-space indentation.
- Use `snake_case` for modules/functions/files, `CamelCase` for types and
  traits, and `SCREAMING_SNAKE_CASE` for constants.
- Keep GUI/CLI adapters thin. Shared behavior belongs in `forge-core`.
- Reuse an existing module or workspace dependency before adding a crate or
  abstraction.
- Preserve deterministic output: stable path ordering, two-space pretty JSON
  and trailing newlines make Git diffs reviewable.
- Do not commit `.forge-local/`, `.forge/`, `.env.local` or
  `*.secrets.json`.

## Changing persisted formats

Request and sidecar files are compatibility boundaries. A format change is
not complete until it considers all of the following:

1. Rust model parsing and `deny_unknown_fields` behavior.
2. The corresponding JSON Schema under `schemas/`.
3. Existing request-v1 specification text in
   `docs/architecture/request-format-v1.md`.
4. Migration/import/export round trips and deterministic serialization.
5. A fixture or test proving both the accepted form and relevant rejection.

Do not silently reinterpret old files. Increment a format version or provide
an explicit migration when compatibility cannot be maintained.

## Reviewing an architectural change

Before adding behavior, trace it through the real entry points:

```text
GUI panel/action ─► Bridge command ─► forge-core API
CLI command       ─────────────────► forge-core API
```

If GUI and CLI need the same rule, implement it once in core. If a change is
only presentation, keep it out of core. Concrete HTTP and filesystem services
already live in core; add a trait only when another implementation or test
boundary requires one.

Finish documentation-only work with `git diff --check`. For code changes,
report the exact focused and workspace commands you actually executed.
