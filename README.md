# Forge — API Test IDE

Forge is a native, GUI-free-core desktop IDE for writing, running and
managing API tests — built in Rust, styled after the JetBrains / IntelliJ
family of tools (dockable panels, dark **Darcula** and **Light** themes, a
keyboard-first workflow with a global "Search Everywhere"). It's the kind
of tool you reach for instead of Postman/Insomnia when you want your API
tests to live in git, review cleanly in diffs, and run the same way on
your laptop and in CI.

The project is a Cargo workspace:

- **`forge-core`** — the GUI-free domain core: the workspace file format,
  variable interpolation, the HTTP execution engine, assertions, scripting,
  the test runner, OpenAPI import/contract tests, curl/code-snippet
  conversion and execution history. Both the GUI and the CLI are thin
  shells over this crate.
- **`forge-gui`** — the desktop IDE (`forge-ide` binary), built with
  [`egui`](https://github.com/emilk/egui)/`eframe`.
- **`forge-cli`** — the headless runner (`forge` binary) for local use and CI.

## Features

- **Collections tree** — requests and folders organized on disk exactly as
  they appear in the sidebar, with drag-and-drop reordering and rename.
- **Environments & secrets** — named environments with plain and secret
  variables; secret *values* are always kept out of git.
- **`{{variables}}`** — interpolation across URLs, headers, params, bodies
  and auth configs, resolved through request → folder → collection →
  environment scopes, plus built-in dynamic variables (e.g. `{{$uuid}}`).
- **HTTP execution engine** — a `reqwest`-based engine with full timing
  breakdowns (DNS, connect, TLS, TTFB, download), redirects, proxies,
  cookies and gzip/brotli/deflate.
- **Declarative assertions** — status code/class, headers, content type,
  JSONPath, body contains/matches, response time, JSON Schema — plus
  **one-click assertion generation** from a live response.
- **Contract tests from OpenAPI** — import an OpenAPI 3.x spec and bind
  requests to `operationId`s to keep your collection honest as the spec
  evolves.
- **Rhai & JavaScript pre-/post-request scripts** — full scripting hooks
  per request for request mutation, chaining and custom validation, in
  either Rhai or sandboxed JavaScript (QuickJS).
- **Suite lifecycle hooks** — `beforeAll` / `beforeEach` / `afterEach` /
  `afterAll` scripts on collections and folders, sharing variables with the
  requests they wrap.
- **Data-driven runs** — replay a request or a whole collection once per
  row of a CSV or JSON dataset.
- **JUnit XML reports** — CI-friendly output from both the GUI runner and
  the CLI.
- **Headless CLI runner (`forge`)** — run collections in CI with the same
  engine and assertions as the IDE.
- **curl import/export** — paste a curl command to create a request, or
  copy any request back out as curl (plus other code snippets).
- **GraphQL, WebSocket & SSE** support alongside plain HTTP.
- **Request history & diff** — every run is recorded locally so you can
  diff responses over time.
- **Darcula & Light themes**, a **keyboard-first** UI, and **Search
  Everywhere** for jumping to any request, environment or setting.

## Building

Requires a recent stable Rust toolchain (see `rust-toolchain.toml`).

```sh
cargo build --release
```

On Linux, the desktop GUI links against the system windowing stack. Install
the following before building `forge-gui`:

```sh
sudo apt install libxkbcommon-dev libwayland-dev libx11-dev libxrandr-dev \
    libxi-dev libxcursor-dev libgl1-mesa-dev cmake pkg-config
```

The headless `forge-cli` and the `forge-core` library have no such
requirement and build anywhere Rust does.

Binaries are produced at `target/release/forge-ide` (GUI) and
`target/release/forge` (CLI).

## Workspace-on-disk format

A Forge workspace is a plain directory tree, designed to be readable,
diffable and merge-friendly in git. **Identity is the file or directory
name** — there are no UUIDs in committed files, so renaming a request in
the IDE is just a `git mv`.

```
my-workspace/
├── forge.json                          # workspace marker + global settings
├── .gitignore                          # generated; ignores .forge-local/ and *.secrets.json
├── .forge-local/                       # local-only state (never committed)
├── environments/
│   ├── dev.env.json                    # committed: variable names + non-secret values
│   ├── dev.secrets.json                # gitignored: secret variable values
│   ├── staging.env.json
│   └── staging.secrets.json
├── specs/
│   └── api.yaml                        # imported OpenAPI specs
└── collections/
    └── payments/
        ├── collection.json             # collection metadata, variables, auth, child order
        ├── create-charge.request.json
        ├── list-charges.request.json
        └── refunds/                    # sub-folder
            ├── folder.json             # folder metadata, variables, auth, child order
            └── create-refund.request.json
```

- **`forge.json`** — the workspace root marker. Holds the `format`
  version, workspace `name`, and global `settings` (timeout, redirects,
  TLS verification, proxy, user agent).
- **`environments/<name>.env.json`** — a committed environment. Variables
  are either a plain `value`, or declared as `"secret": true` with no
  value. **`environments/<name>.secrets.json`** is the gitignored sibling
  holding the actual secret values, keyed by variable name.
- **`collections/<name>/collection.json`** — a collection's metadata:
  `name`, `description`, `variables`, `auth`, an `openapi` binding when the
  collection was generated from a spec, an `order` array, and optional
  suite lifecycle `hooks`:

  ```json
  {
    "hooks": {
      "beforeAll": "vars.set(\"token\", \"...\");",
      "beforeEach": "log(\"about to run a request\");",
      "afterEach": "assert(res.status < 500, \"no server errors\");",
      "afterAll": "log(\"suite done\");",
      "language": "rhai"
    }
  }
  ```

  All four scripts are optional; `language` is `"rhai"` (default, omitted
  when default) or `"js"` and applies to all four.
- **`collections/<name>/folder.json`** — the same shape as a collection's
  metadata (minus the OpenAPI binding) for a sub-folder, including `hooks`.
- **`*.request.json`** — one HTTP request: `method`, `url`, `params`,
  `headers`, `auth`, `body`, `assertions`, `extractors`, pre-/post-request
  `scripts` (with an optional `"language": "rhai" | "js"`), and per-request
  `settings` overrides.
- **`specs/`** — OpenAPI specs imported into the workspace.
- **`.forge-local/`** — local-only state (run history, UI layout) that is
  never committed.

**Ordering:** `collection.json` and `folder.json` each carry an explicit
`order` array of child file/directory names. Children not listed are
appended alphabetically. This keeps reordering a one-line diff instead of
producing spurious changes across the whole file, and keeps merges of
concurrent additions conflict-free.

**Filename is identity:** a request's file name (not its `name` field) is
its stable identifier within its parent folder; the IDE keeps it in sync
with the display name but you're free to diverge (e.g. `v2-create.request.json`
titled "Create Charge (v2)").

See [`examples/demo-workspace`](examples/demo-workspace) for a complete,
working example of this layout.

## Scripting & lifecycle hooks

Scripts run in one of two sandboxed languages, chosen per request (Scripts
tab → Language) or per hook set (`hooks.language`):

- **Rhai** (default) — the embedded Rust scripting language. Snake_case
  API: `req.set_header(n, v)`, `res.body_text`, `base64_encode(s)`.
- **JavaScript** — QuickJS, fully sandboxed (32 MB memory cap, ~2 s wall
  clock budget, no filesystem/network/process access). CamelCase API:
  `req.setHeader(n, v)`, `res.bodyText`, `base64Encode(s)`, plus
  `console.log(...)`.

Both expose the same surface: `req` (pre-request only: `url` get/set,
`method`, header get/set/remove, body text get/set), `res` (post-response:
`status`, `bodyText`/`body_text`, `header(n)`, `timeMs`/`time_ms`,
`json()`), `vars.get(n)`/`vars.set(n, v)` (persisted into the run's
variable scope), `log(msg)`, `assert(cond, message)` and `test(name, cond)`
(recorded as assertion results), and helpers `uuid()`, `timestamp()`,
base64 encode/decode. Compile errors, runtime errors and runaway scripts
are captured as script errors — they never crash the app or the runner.

**Suite lifecycle hooks** attach to a collection or folder (right-click →
"Edit Hooks..." in the IDE, or edit `collection.json`/`folder.json`) and
run around the requests underneath during collection/folder/workspace runs:

- **Order.** For each request, `beforeEach` hooks fire outermost-first
  (collection first, then each folder down to the request's parent);
  `afterEach` fires in reverse (innermost first). `beforeAll` fires once
  per run, immediately before a scope's first executed request; `afterAll`
  once per run, after its last (on the final data iteration).
- **API.** `beforeAll`/`beforeEach` get the vars-only API (`vars`, `log`,
  `assert`/`test`, helpers — no `req`/`res`). `afterEach`/`afterAll`
  additionally get `res`, the just-finished request's response.
- **Variables.** `vars.set` from any hook lands in the shared runtime
  scope, visible to the affected request itself and everything after it.
- **Errors.** A `beforeAll`/`beforeEach` error fails the affected request
  (it is not sent; the outcome reads `beforeEach hook failed: ...`), and
  `--bail` semantics apply. `afterEach`/`afterAll` errors are appended to
  the request's script log (prefixed `afterEach:`/`afterAll:`) without
  flipping a passed request to failed.
- **Assertions.** `assert`/`test` from `afterEach`/`afterAll` extend the
  request's assertion list (a failing hook assertion fails the request);
  assertion calls from `before*` hooks are dropped by design.
- **Logs.** Hook log lines flow into the affected request's script log
  prefixed `hook:`; `beforeAll`/`afterAll` output attaches to the scope's
  first/last request.

Hooks are a runner concept: single ad-hoc sends from the editor run only
the request's own pre/post scripts.

## CLI usage

```sh
forge run <workspace> --env dev --report junit.xml --data rows.csv --bail
```

- `<workspace>` — path to a workspace directory (containing `forge.json`).
- `--env <name>` — environment to resolve `{{variables}}` against.
- `--report <path>` — write a JUnit XML report to `<path>`.
- `--data <rows.csv|rows.json>` — run the target once per row for
  data-driven testing.
- `--bail` — stop the run on the first failing request.

## Screenshots

_Coming soon — the desktop IDE is under active development. This section
will hold screenshots of the collections tree, request editor, and run
results once the GUI reaches a demonstrable state._

## License

Licensed under the [MIT License](https://opensource.org/licenses/MIT).
