# CLI and CI

The `forge` binary is the headless adapter over `forge-core`. Build it with
`cargo build --release -p forge-cli`; examples below then use
`target/release/forge`. During development, replace `forge` with
`cargo run -p forge-cli --`.

## Choose the correct runner

ApiWright currently supports two on-disk generations:

| Project marker | Commands | Purpose |
| --- | --- | --- |
| `project.json` | `validate`, `run-v1`, `ci`, `run-sequence`, `assets`, `lock`, `mock` | Current request-v1 projects, sidecars and catalog assets |
| `forge.json` | `run`, `list`, `envs` | Legacy workspaces kept for compatibility and migration |

Use `ci` for current projects in automation. It always treats the selected
requests as independent tests. `run-v1` is the interactive equivalent: one
file expands its matrix, multiple explicit files form an ordered sequence,
and a folder runs independently in stable path order.

## Project roots and targets

`--root` must point at the directory containing `project.json`. If omitted,
ApiWright walks upward from the first target until it finds that file. Relative
targets are resolved **below the selected root**; absolute targets are allowed
only when they remain inside the project. Folder scans are recursive, skip
symbolic links, sort by path, and de-duplicate files.

```sh
# From the repository root
apiwright ci requests/checkout --root . --env staging

# The complete offline demo
apiwright ci requests --root examples/demo-workspace \
  --env demo --mock --allow-project-code

# No target is required for the regression selection
apiwright ci --root . --regression
```

`--regression` keeps only request documents marked as regression tests in
Properties. With no target it scans `<root>/requests`. An empty selection is
an error, not a successful no-op.

## Current project commands

### Validate and execute

```sh
apiwright validate requests/users/get.request.json --root . --env staging
apiwright run-v1 requests/login.request.json requests/profile.request.json --root .
apiwright ci requests/users requests/orders/create.request.json --root .
apiwright run-sequence smoke.sequence.json --root .
```

- `validate <request>` parses the request and sidecars, resolves references,
  bindings, inherited environment and canonical IR, but performs no network
  request. Secret references receive placeholders, so validation does not
  require real secret values.
- `run-v1 [targets...]` accepts request files or folders. Multiple explicit
  files thread `${runtime.*}` extractor output forward. A matrix request must
  be run alone or through batch/folder mode; matrix-by-sequence semantics are
  intentionally rejected.
- `ci [targets...]` uses the same engine but forces batch semantics: every
  request is isolated from other requests' runtime values and each matrix case
  is expanded independently.
- `run-sequence <sequence>` resolves a persisted `*.sequence.json` below the
  project and executes its declared order with runtime chaining.

All four execution commands accept `--env <name>` where applicable.
`run-v1`, `ci`, and `run-sequence` also accept:

- `--mock` renders each request's configured mock in-process instead of
  sending HTTP. Missing mocks fail the request.
- `--frozen` verifies assets against `.forge/lock.json` before execution and
  aborts on missing lock data or drift.
- `--allow-project-code` permits repository-owned JavaScript after it has been
  reviewed. Project code is denied by default, including code used by the
  configured auth request.

Secrets resolve from `<root>/.env.local` first and then process environment
variables. Do not commit either resolved values or `.env.local`.

### Inspect, lock and mock

```sh
apiwright assets .
apiwright assets . --json
apiwright lock .
apiwright lock . --check
apiwright mock . --port 9090 --env staging
```

`assets` reports assets, request usage, environments and broken references;
`--json` emits the complete index. `lock` writes `.forge/lock.json`, while
`--check` only verifies it. Because `.forge/` is ignored by default, a CI job
using `--frozen` must restore a trusted lockfile as an artifact or explicitly
version it according to the team's policy.

`mock` scans the project and serves all configured routes on
`0.0.0.0:<port>` (default `8080`) until the process is stopped. Treat it as a
development server: binding to all interfaces can expose mocks on the local
network.

### Import, export and migration

```sh
apiwright export requests/orders --root . --format json -o orders.forge.json
apiwright export requests/orders/get.request.json --root . \
  --format curl -o get-order.forge.sh
apiwright import orders.forge.json requests/imported

apiwright migrate legacy.request.json -o requests/legacy.request.json
apiwright migrate-all legacy/ requests/migrated --dry-run
```

`export` never overwrites an existing output. JSON bundles retain readable
UTF-8 and base64-encode only binary files. A cURL export is executable and
embeds the lossless bundle needed to restore assertions, hooks and properties.
`import` validates every path and collision before writing. See
[Import and export](import-export.md) for the bundle contract.

`migrate` writes one converted request or prints it to stdout. `migrate-all`
preserves relative paths; run `--dry-run` first. Existing destinations and
unsupported lossless conversions are reported instead of overwritten.

## Legacy workspace commands

```sh
apiwright list examples/demo-workspace
apiwright envs examples/demo-workspace
apiwright run examples/demo-workspace --scope collections/httpbin \
  --env httpbin --bail --delay-ms 100 --report target/httpbin.xml
apiwright run examples/demo-workspace --data cases.csv
```

`run <workspace>` loads `forge.json`. `--scope` may identify a request,
collection or nested folder; omission runs the workspace. `--data` accepts
CSV or JSON rows, `--bail` stops after the first failed request,
`--delay-ms` throttles requests, and `--report` writes JUnit XML. JUnit output
is currently an option of this legacy runner, not `apiwright ci`.

## gRPC commands

```sh
forge grpc list api.proto -I proto/includes
forge grpc call api.proto -I proto/includes \
  --endpoint https://localhost:50051 \
  --method example.Users/GetUser \
  --data @request.json -m 'authorization:Bearer token'
printf '{"id":"42"}' | forge grpc call api.proto \
  --endpoint http://localhost:50051 --method example.Users/GetUser --data -
```

`grpc list` prints every compiled service method and whether it is unary or
streaming. `grpc call` currently executes unary methods. `--data` accepts an
inline JSON object, `@file.json`, or `-` for stdin; `--meta key:value` and
`--include`/`-I` are repeatable.

## Exit status contract

| Code | Meaning |
| --- | --- |
| `0` | Command completed and all selected tests/checks passed |
| `1` | A completed check failed: assertion/test failure, invalid v1 diagnostics, asset/lock drift, blocked migration, or a gRPC call failure |
| `2` | Usage, project setup, parsing, I/O, configuration, or execution could not be completed |

For multi-result v1 runs, the worst status wins: `Error` overrides `Failed`,
which overrides `Passed`. Shell termination by a signal follows the operating
system's status and is not normalized by ApiWright.

## CI examples

Keep secrets in the CI secret store, pin `Cargo.lock`, and preserve ApiWright's
exit code:

```sh
cargo build --release --locked -p forge-cli
target/release/forge validate requests/health.request.json --root . --env ci
target/release/forge ci requests --root . --env ci --frozen
```

Minimal GitHub Actions step:

```yaml
- uses: actions/checkout@v4
- uses: dtolnay/rust-toolchain@stable
- run: cargo build --release --locked -p forge-cli
- name: Run ApiWright regression suite
  env:
    API_TOKEN: ${{ secrets.API_TOKEN }}
  run: target/release/forge ci --root . --env ci --regression
```

Do not append `|| true` or pipe through a command that hides the runner's
status. Archive logs or reports in a later `if: always()` step instead.
