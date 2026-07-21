# Forge Request Format & Execution Architecture (v1)

Status: design specification. This describes a **new, IDE-independent subsystem** — a
persisted request format, an asset store, and a resolution/execution engine. It is not
coupled to the existing egui GUI; the GUI and the `forge` CLI consume it through the
stable types in §"Canonical IR". The current `forge-core` model (the `*.request.json`
files) is a valid **v0** and can be migrated (§16).

Design goals, taken as hard constraints:

- Local-first, Git-friendly, deterministic, CI-runnable.
- No custom DSL, no proprietary monolithic collection, no code-as-JSON-strings, no
  hidden database as source of truth, no mandatory multi-file-per-request, no forced
  central registry.
- Exactly one request per normal JSON file.
- Reusable behavior and data live in an asset store, referenced from request files.
- Reuse existing standards: JSON Pointer (RFC 6901), JSON Patch (RFC 6902), JSON
  Schema (2020-12).

The recurring theme: **the request document is a thin, declarative description; all
reuse is a reference.** Hooks/extractors and assertions live in the derived siblings
`*.hooks.json` and `*.assertions.json`, so the request stays focused on HTTP data.

---

## Implementation status

A working first version lives in `crates/forge-core/src/reqv1/` and the
`forge validate` / `forge run-v1` CLI subcommands. Built:

- Document model + `deny_unknown_fields` validation (`model.rs`), schema at
  `schemas/request-v1.schema.json`.
- Request-adjacent hook and assertion documents (`hooks.rs`, `assertions.rs`), schemas at
  `schemas/hooks-v1.schema.json` and `schemas/assertions-v1.schema.json`; no path
  configuration is required.
- Ref parsing, alias (exact/prefix) resolution, path-escape guard (`refs.rs`).
- Data-asset resolution: load, JSON Pointer, JSON Patch, clone-on-read cache,
  reference-cycle detection, full diagnostic set (`resolve.rs`, `diag.rs`).
- Namespaced, type-preserving variable resolution with `$$` escaping, the
  coercion table, and secret masking (`vars.rs`).
- Binding resolution with topological ordering + `BINDING_CYCLE`, builtin
  generators `uuid`/`now` (`build.rs`), canonical IR (`ir.rs`).
- 4-phase pipeline with builtin assets `bearer`, `basic`, `header` (hooks),
  `assert-status`, `assert-json-path`, `assert-header` (assertions),
  `extract-json-path` (extractor), plus the header-upsert conflict warning
  (`pipeline.rs`).
- Runner: validate (no network) + run over the existing `forge-core` HTTP
  engine, or serve the document's static mock; `RunResult`/`Diagnostic` with
  secret masking (`runner.rs`).

Since landed (originally deferred, all additive):

- **Matrix parameterization** (`matrix.rs`): each matrix binding must resolve
  to an array; cartesian product across names; one run per case with
  `${matrix.<name>}` bound; per-case results; CLI iterates automatically.
- **Project `.js` executable assets** (`jshost.rs`): QuickJS host with a
  128 MB memory cap and 5 s interrupt budget. Contract: the file defines
  `function run(ctx, input)`; `ctx` is a frozen JSON snapshot
  (`request`/`response?`/`bindings`) — assets never see engine memory. The
  return shape decides the kind (hook patch / assertion result(s) /
  `{runtime}` / generator value / mock response). `.ts` gets a clear
  "transpile to .js" diagnostic — v1 does not pretend to run TypeScript.
- **Executable (dynamic) mocks** — same JS contract, `{ status, headers?,
  body? }`, with assertions running against the produced response.
- **`.env.local` secret provider** in the CLI (file first, process env
  fallback — the §14 declared order).
- **All four pipeline phases**: `onError` (runs only when the run errored,
  with `ctx.error`) and `finally` (always, for teardown/always-checks) now
  execute instead of being silently ignored. Reaction assets may assert or
  extract; builtins that need a response are skipped with an info note when
  none exists.
- **Runtime threading + sequences** (§9): `${runtime.*}` resolves from
  earlier requests, and `run_sequence` runs a list of request files in order,
  carrying each request's extracted runtime to the next. Persisted
  `*.sequence.json` documents use `schemas/sequence-v1.schema.json`; CLI
  `run-sequence` and the IDE execute them in declared order and retain each
  response.
- **`builtin:assert-schema@1`**: validates the response body against an
  inline JSON Schema (reuses the crate's `jsonschema` validator).

**v1 request editor** (`dialogs/v1_editor.rs`): a self-contained window for
authoring a `*.request.json` with chill store access — the asset store on the
left (data fixtures browsable to any JSON node, hooks/assertions/extractors/
generators/mocks), each with an "insert" that drops a ready `ref`/`use`
entry into the typed request model or assertion sidecar. Bindings receive a safe, collision-free
name and the editor shows the corresponding `${bindings.name}` expression;
pipeline entries, assertions, and mocks go to their structural slots. The right side is a
Postman/Bruno-style vertical split (draggable): the
request JSON on top (toolbar: Validate/Save/mock/environment picker/Run), and
below it a tabbed results pane — **Result**, **Assertions** (its own pane,
with a pass/fail count), **Runtime** (extracted vars), **Diagnostics**. Run
goes over the bridge (HTTP or `mock`). Opened from the Assets panel's request
list ("edit") or "New request". This closes the last-mile authoring UX; the
format already supported the referencing.

**Asset store view** (§11): `index.rs` scans a project into a `ProjectIndex`
— assets grouped by kind, each data asset browsable to any JSON node, a
reverse-reference (usage) graph, broken-ref detection with request + instance
path, and `suggest_ref` (alias-preferred, else relative). Surfaced by
`forge assets [--json]` and a GUI "Assets" tool window (left stripe): browse
by kind, expand data assets to copy a `data:x#/pointer` ref for any node,
per-asset usage badges, broken refs flagged, open-in-editor. Read-only — the
filesystem stays the source of truth.

- **Full sibling-schema validation** (`resolve.rs`): a data asset with a
  `*.schema.json` sibling is validated against it (draft-2020-12) at load time
  — the `ponytail:` presence+parse seam is closed.
- **Lockfile + integrity** (`lock.rs`): `forge lock` writes `.forge/lock.json`
  (sha256 per file asset); `forge lock --check` and `run-v1 --frozen` verify
  and report drift (changed / missing / unlocked). Off by default.
- **Mock server** (`mock.rs`): `forge mock <root>` serves each mock-bearing
  request document over HTTP, routed by method + a path template derived from
  its URL (`:seg` and `${...}` segments are wildcards; literal routes beat
  wildcards). Static and dynamic (JS) mocks both served; an optional
  `mocks.routes.json` adds/overrides explicit routes. Routing lives outside
  the request document (§10). Matching is a pure `handle(method, path)`,
  socket-free-testable.

The Assets tool window can scaffold executable JavaScript assets together
with typed colocated metadata, run every request affected by an asset, create
and run stored sequences, and preview/execute whole-tree migrations. Execution
history for both request generations shares `.forge-local/history.sqlite`.

Still deferred (each additive, no format break): keychain/external secret
providers (interface exists; not built — headless-untestable); Worker-process
isolation tiers beyond trusted-local; asset rename/move; matrix × sequence
combined (niche).

The shipped runnable example is
`crates/forge-core/tests/fixtures/reqv1/project/` — the canonical §1 document
using builtins instead of project assets, exercised end-to-end (HTTP + mock)
by `crates/forge-core/tests/reqv1_test.rs`.

## 0. Decisions at a glance

| Area | Decision | Why default | When to deviate |
|------|----------|-------------|-----------------|
| Pipeline phases | 4: `beforeRequest`, `afterResponse`, `onError`, `finally` | Covers auth, assertion, extraction, cleanup. `beforeResolve` has no useful context. | Add `beforeResolve` only if a hook must mutate bindings before resolution. |
| Asset kinds | 6: `data`, `generator`, `hook`, `assertion`, `extractor`, `mock` | `transformer` is just a `hook` returning a `RequestPatch`. | Split out `transformer` only if its contract genuinely diverges. |
| Binding shapes | `value` \| `ref` \| `use` (unchanged from brief) | One model for static-local, static-ref, executable. | — |
| Parameterization | Separate top-level `matrix`, not magic array-in-bindings | Explicit iteration marker; bindings stay single-valued and predictable. | — |
| Variable resolution | Namespaced (`env`/`secret`/`bindings`/`runtime`/`matrix`), strict, type-preserving, **no re-scan of data-asset content** | No implicit precedence; deterministic; prevents data-driven `${}` injection. | — |
| Executable assets | Plain `.js` on in-process QuickJS, timeout+memory cap, deep-frozen JSON context; **not an adversarial sandbox** | Small host with an explicit trust gate. | Untrusted projects require a separate process/container (§15). |
| Version suffix `@N` | **Required** on `builtin:` assets, **optional** on `project:` assets | Builtins evolve with the tool → need pinning; project assets evolve with git. | Use `@N` on project assets to run two contract versions during a migration. |
| Lockfile | Optional `.forge/lock.json`, off by default | Reproducibility for CI; git already pins project assets. | Turn on for release CI or shared fixtures. |
| Asset index | Optional generated cache, never source of truth | Refs resolve from the filesystem; index only speeds lookup. | — |

---

## 1. Persisted request document model

A request document contains only metadata, bindings, an optional matrix, the HTTP request
and an optional mock. Hooks and assertions are stored in automatically derived siblings
(`create.hooks.json` and `create.assertions.json`). Inline pipeline entries remain readable
for compatibility and are split into those sidecars when the IDE saves the request.

Canonical example (`requests/users/create.request.json`):

```json
{
  "$schema": "../../schemas/request-v1.schema.json",
  "formatVersion": 1,
  "kind": "request",

  "meta": { "id": "users.create", "name": "Create user", "tags": ["users"] },

  "bindings": {
    "user":      { "ref": "data:users#/valid/alice" },
    "tenant":    { "ref": "data:tenants#/default" },
    "requestId": { "use": "builtin:uuid@1" }
  },

  "request": {
    "method": "POST",
    "url": "${env.baseUrl}/users",
    "headers": [
      { "name": "Content-Type", "value": "application/json", "enabled": true },
      { "name": "X-Request-ID", "value": "${bindings.requestId}", "enabled": true }
    ],
    "body": {
      "type": "json",
      "value": {
        "name": "${bindings.user.name}",
        "email": "${bindings.user.email}",
        "tenantId": "${bindings.tenant.id}"
      }
    }
  },
  "mock": {
    "status": 201,
    "body": { "ref": "data:user-responses#/created" }
  }
}
```

Companion `requests/users/create.hooks.json`:

```json
{
  "$schema": "../../schemas/hooks-v1.schema.json",
  "formatVersion": 1,
  "kind": "hooks",
  "hooks": [
    { "phase": "beforeRequest", "use": "project:auth/service-token@1" },
    { "phase": "afterResponse", "use": "project:extractors/user-id@1",
      "with": { "target": "runtime.userId" } }
  ]
}
```

Companion `requests/users/create.assertions.json`:

```json
{
  "$schema": "../../schemas/assertions-v1.schema.json",
  "formatVersion": 1,
  "kind": "assertions",
  "assertions": [
    { "use": "builtin:assert-status@1", "with": { "expected": 201 } },
    { "use": "project:assertions/user-created@1",
      "with": { "expectedUser": "${bindings.user}" } }
  ]
}
```

The authoritative structure is `schemas/request-v1.schema.json` (companion to this doc).
Every field is `additionalProperties: false` — unknown keys are validation errors, so
typos surface immediately and forward-compat is a deliberate, versioned act.

---

## 2. JSON Schema structure

See `schemas/request-v1.schema.json`. Shape summary:

- Top level requires `formatVersion: 1`, `kind: "request"`, `meta`, `request`.
- `meta.id` restricted to `[a-zA-Z0-9._-]+` (safe as a map key and CLI selector).
- `bindings` / `matrix`: maps of `binding` (`$defs/binding`, a `oneOf` over
  value/ref/use — mutually exclusive by `additionalProperties: false`).
- `request.body` is `{ type, value? | ref? }` — the body can itself be an asset ref.
- `<name>.hooks.json`: `{ formatVersion, kind: "hooks", hooks[] }`; each hook is
  `{ phase, use, with?, enabled? }`.
- `<name>.assertions.json`: `{ formatVersion, kind: "assertions", assertions[] }`;
  each assertion is `{ use, with?, enabled? }` and runs in `afterResponse`.
- `mock`: `oneOf` static (`status`+…) or executable (`use`+`with?`).
- `assetRef` pattern forbids backslashes (OS portability, §11).

Validation runs **before** any resolution (first stage of the pipeline in §12). A
document that fails schema validation never resolves or executes.

---

## 3. TypeScript types — persisted documents

These mirror the schema. They are the *unresolved* types: bindings and refs are still
descriptions, not values.

```ts
export interface RequestDocument {
  $schema?: string;
  formatVersion: 1;
  kind: "request";
  meta: RequestMeta;
  bindings?: Record<string, Binding>;
  matrix?: Record<string, Binding>;
  request: RequestSpec;
  pipeline?: PipelineEntry[];
  mock?: MockDef;
}

export interface RequestMeta {
  id: string;
  name: string;
  description?: string;
  tags?: string[];
}

export interface RequestSpec {
  method: HttpMethod;
  url: string;                 // may contain ${...}
  headers?: HeaderSpec[];
  query?: HeaderSpec[];
  body?: BodySpec;
}

export type HttpMethod =
  | "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE";

export interface HeaderSpec {
  name: string;
  value: string;               // may contain ${...}
  enabled?: boolean;           // default true
}

export interface BodySpec {
  type: "json" | "text" | "form" | "multipart" | "binary" | "none";
  value?: unknown;             // may contain ${...} at any depth
  ref?: string;                // asset ref, alternative to value
}

export type PipelinePhase =
  | "beforeRequest" | "afterResponse" | "onError" | "finally";

export interface PipelineEntry {
  phase: PipelinePhase;
  use: string;                 // asset ref -> hook | assertion | extractor
  with?: Record<string, unknown>;
  enabled?: boolean;           // default true
}

export type MockDef =
  | { status: number; headers?: HeaderSpec[]; body?: BodySpec; delayMs?: number }
  | { use: string; with?: Record<string, unknown> };
```

### The unified binding model

```ts
export type Binding =
  | { value: unknown }
  | { ref: string; patch?: JsonPatchOperation[] }
  | { use: string; with?: Record<string, unknown> };

export interface JsonPatchOperation {
  op: "add" | "remove" | "replace" | "move" | "copy" | "test";
  path: string;                // JSON Pointer
  from?: string;               // JSON Pointer (move/copy)
  value?: unknown;
}
```

Semantics:

- `value` — a request-local literal.
- `ref` — resolve a **static** asset (a `data` asset), optionally `patch`ed locally.
- `use` — execute an **executable** asset. In a `binding`/`matrix`, `use` must name a
  `generator` asset (pure, produces a value). In a `pipeline` entry, `use` must name a
  `hook`/`assertion`/`extractor`. The engine validates that the referenced asset's
  declared `kind` matches the position; a mismatch is an `INCOMPATIBLE_ASSET_TYPE`
  diagnostic.

---

## 4. TypeScript types — canonical IR

The persistence model is **never** executed directly. Resolution produces a Canonical
Intermediate Representation with fully-resolved, validated, type-preserved values.

```ts
export interface ResolvedRequest {
  meta: RequestMeta;
  method: HttpMethod;
  url: string;                       // fully interpolated
  headers: ResolvedHeader[];         // enabled only, interpolated
  query: ResolvedHeader[];
  body: ResolvedBody;                // ${...} resolved at every depth
  pipeline: ResolvedPipelineEntry[]; // assets loaded, `with` resolved
  mock?: ResolvedMock;
  bindings: Record<string, unknown>; // resolved values, for diagnostics/masking
  secretRefs: ReadonlySet<string>;   // resolved values that came from secret.* (masking)
}

export interface ResolvedHeader { name: string; value: string; }

export type ResolvedBody =
  | { type: "none" }
  | { type: "json"; value: unknown }
  | { type: "text"; value: string }
  | { type: "form"; fields: ResolvedHeader[] }
  | { type: "multipart"; parts: ResolvedPart[] }
  | { type: "binary"; bytesRef: string };

export interface ResolvedPipelineEntry {
  phase: PipelinePhase;
  ref: AssetDescriptor;              // parsed, located asset
  kind: "hook" | "assertion" | "extractor";
  input: Record<string, unknown>;   // resolved `with`
}

export interface AssetDescriptor {
  raw: string;                       // original ref string
  scheme: "builtin" | "project" | "path";
  address: string;                   // resolved absolute file path (or builtin id)
  pointer?: string;                  // JSON Pointer (data assets)
  version?: number;                  // @N
  patch?: JsonPatchOperation[];      // for `ref` bindings only
}

export type ResolvedMock =
  | { kind: "static"; status: number; headers: ResolvedHeader[];
      body: ResolvedBody; delayMs: number }
  | { kind: "dynamic"; ref: AssetDescriptor; input: Record<string, unknown> };
```

### Execution context and patch/result types

The context handed to assets is **frozen** and minimal. Assets never mutate it; they
return patches the runner validates and merges.

```ts
export interface ExecutionContext {
  readonly request: DeepReadonly<ResolvedRequest>;
  readonly bindings: DeepReadonly<Record<string, unknown>>;
  readonly response?: DeepReadonly<HttpResponseView>; // reaction phases only
  readonly error?: string;                            // onError/finally only
}

export interface RequestPatch {
  url?: string;
  headers?: Array<{ name: string; value: string; enabled?: boolean }>;
  body?: unknown;                    // whole-body replacement
}

export interface RuntimePatch { runtime: Record<string, unknown>; }

export interface AssertionResult {
  passed: boolean;
  message: string;
  expected?: unknown;
  actual?: unknown;
  path?: string;                     // JSON Pointer into the response body
}

export interface MockResponse {
  status: number;
  headers?: Array<{ name: string; value: string }>;
  body?: unknown;
  delayMs?: number;
}
```

Assets log through bounded `console.log`. Environment, secret, runtime and
matrix values are exposed only when the request explicitly resolves them into
bindings or `with`; no ambient capability API is injected.

Every executable is a plain script defining synchronous global
`function run(ctx, input)`. Its directory/use position supplies the kind; the
return shape is `RequestPatch`, assertion result(s), `RuntimePatch`, generated
JSON value, or `MockResponse`. ES modules, imports, promises and TypeScript are
not executed by v1.

`data` assets are plain JSON files with an optional companion `*.schema.json`; they have
no module contract.

---

## 5. Asset model and asset contracts

Assets are normal files in the project. No registry file lists them; they are addressed
by alias or path (§11).

```ts
export type AssetKind =
  | "data" | "generator" | "hook" | "assertion" | "extractor" | "mock";
```

- **`data`** — `*.json`. Selected by JSON Pointer, patched locally with JSON Patch,
  validated against an optional sibling `*.schema.json`. Never re-scanned for `${...}`.
- **`generator`** — script producing a JSON value for a binding. No `response` in
  context.
- **`hook`** — `beforeRequest` (or `onError`/`finally`); returns a `RequestPatch`.
  Auth, dynamic headers, signing are all hooks. A "transformer" is a hook.
- **`assertion`** — `afterResponse`; returns `AssertionResult[]`. Never throws for a
  failed check; throwing means the asset itself broke.
- **`extractor`** — `afterResponse`; returns a `RuntimePatch` (values for later requests
  in a run).
- **`mock`** — produces a `MockResponse` from request context.

Built-in assets (shipped with the tool) satisfy the **same** contracts and are addressed
`builtin:<name>@<version>`. Adding a new assertion is a new builtin, not a new schema
field. Minimum builtin set for v1: `builtin:uuid@1` (generator), `builtin:now@1`
(generator), `builtin:assert-status@1`, `builtin:assert-json-path@1`,
`builtin:assert-header@1`, `builtin:assert-schema@1` (assertions),
`builtin:extract-json-path@1` (extractor), `builtin:bearer@1`, `builtin:basic@1` (hooks).

---

## 6. Reference-resolution algorithm

Applies to any `ref` (static asset) and to the *location* half of any `use`. Precise,
ordered, and the only place I/O happens during resolution.

```
resolveRef(refString, patchOps?, stack):
  1. Parse refString -> { addr, pointer, version }.
       Split on first '#'  -> addr, pointer (RFC 6901; absent => whole document).
       Strip trailing '@N' from the last path segment -> version.
       Reject any '\' -> INVALID_ALIAS (portability).
  2. Resolve addr to an absolute file path (§11 alias/path rules).
       Normalize ('.', '..'); canonicalize; assert inside project root -> else PATH_ESCAPE.
       For builtin:* -> map to the shipped asset id, skip filesystem.
  3. Cycle check: frame = addr#pointer.
       If frame in stack -> REFERENCE_CYCLE (report the stack).
       Push frame.
  4. Load + parse the file (cache by absolute path + mtime/hash).
       Parse error -> INVALID_ASSET.
       If a sibling *.schema.json exists (data assets) -> validate -> else INVALID_ASSET_INPUT.
       If asset declares a kind incompatible with the caller position
         -> INCOMPATIBLE_ASSET_TYPE.
       If version requested and asset's declared version != requested (builtins) or
         asset declares an incompatible contract -> UNSUPPORTED_ASSET_VERSION.
  5. Apply JSON Pointer to the parsed document.
       Pointer misses -> INVALID_POINTER.
  6. Deep-clone the selected value (cache stays pristine; patches are request-local).
  7. Apply patchOps in array order (RFC 6902).
       Any op fails (incl. a 'test' op) -> JSON_PATCH_FAILED (report op index).
  8. Recursively resolve ${...} and nested bindings *that the request document authored*
     inside the cloned value — but NOT inside content that originated from a data asset
     (see §9, no re-scan). In practice: patch `value`s and `with` inputs are scanned;
     loaded data payloads are not.
  9. Preserve the original JSON type at every leaf.
  10. Pop frame. Return the resolved value.
```

Note on step 4 caching: the parsed asset is cached per resolution run keyed by absolute
path + content hash. Clone-on-read (step 6) guarantees a local `patch` never leaks into
another binding that references the same asset.

### Detected + reported failures (diagnostic codes)

| Code | Trigger |
|------|---------|
| `ASSET_NOT_FOUND` | resolved path does not exist |
| `INVALID_ALIAS` | alias unknown, ambiguous, or contains `\` |
| `INVALID_POINTER` | JSON Pointer selects nothing |
| `JSON_PATCH_FAILED` | a patch op (incl. `test`) failed; carries op index |
| `REFERENCE_CYCLE` | a `ref` re-enters an in-progress frame |
| `BINDING_CYCLE` | a `${bindings.x}` chain forms a loop |
| `INVALID_ASSET_INPUT` | data asset fails its sibling schema, or `with` fails the asset's input schema |
| `INCOMPATIBLE_ASSET_TYPE` | asset kind wrong for the position (e.g. assertion in a binding) |
| `UNSUPPORTED_ASSET_VERSION` | requested `@N` not satisfiable |
| `PATH_ESCAPE` | resolved path leaves the project root |

Every diagnostic carries a JSON Pointer into the request document (`instancePath`) so an
editor can underline the exact `ref`/`use`.

---

## 7. Cycle detection and resolution error handling

Two independent cycle classes:

- **Reference cycles** (asset A `ref`s B `ref`s A): the `stack` in §6 catches these. A
  data asset that references another asset only via `${bindings.*}` cannot cycle because
  data content is not re-scanned; cycles only exist through the request document's own
  binding graph.
- **Binding cycles** (`bindings.a` = `${bindings.b}`, `bindings.b` = `${bindings.a}`):
  bindings resolve lazily with a `visiting` set. Requesting a binding already in
  `visiting` → `BINDING_CYCLE`. `matrix` entries may reference `bindings` but not vice
  versa (matrix is the outer loop), which removes a whole class of ambiguity.

Error handling policy: **resolution is all-or-nothing per request, but collects all
independent errors first.** The resolver does not stop at the first bad ref; it walks the
whole binding graph and pipeline, accumulates every diagnostic it can reach, and only
then fails the request. This gives an editor a full error list per save instead of
one-at-a-time. A cycle short-circuits just its own branch.

---

## 8. Variable-resolution algorithm

Variables are namespaced and explicit. No implicit precedence, ever.

Namespaces: `env`, `secret`, `bindings`, `runtime`, `matrix`.

```
interpolate(node, scope):
  if node is string:
    if node matches ^\$\{ ([^}]+) \}$   (a single whole-string expression):
      return resolveVar(expr, scope)        // TYPE PRESERVED
    else:
      replace each ${expr} occurrence with coerceToString(resolveVar(expr, scope))
      // literal $${ ... } -> ${ ... } (escape), not an expression
  if node is array: map interpolate over elements
  if node is object: map interpolate over values (keys are never interpolated)
  else: return node unchanged
```

Rules, decided:

- **Type preservation.** `"${bindings.timeout}"` where `bindings.timeout` is `5000`
  resolves to the number `5000`, not `"5000"`. Any whole-string single expression keeps
  the source type (number, boolean, null, object, array).
- **String coercion.** Inside a larger string (`"Bearer ${secret.apiToken}"`), the value
  is coerced with a defined table: string→itself, number/boolean→JSON text,
  null→**error** (`NULL_IN_STRING`), object/array→**error** (`STRUCTURED_IN_STRING`).
  You may not silently stringify a null or an object into the middle of a header.
- **Missing variable** → `MISSING_VARIABLE` (strict; no silent empty string). v1 has no
  optional-variable syntax; add `${x?}` later only if a real need appears.
- **null whole-expression** → resolves to `null` (preserved), allowed.
- **Object / array whole-expression** → preserved (that is how `expectedUser` receives
  the whole `bindings.user` object).
- **Escaping** → `$$` before `{` escapes: `"$${keep}"` → literal `"${keep}"`.
- **No recursion into resolved data.** A value resolved from a `data` asset is inserted
  verbatim; its own `${...}`-looking strings are **not** re-interpolated. This is the
  single most important determinism rule: test data cannot inject variable expressions
  into your request. Only the request document's authored strings and asset `with`
  inputs are scanned.
- **Recursive variables / cycles** across bindings → `BINDING_CYCLE` (§7).
- **Secret masking.** Any leaf whose value came through the `secret` namespace is
  recorded in `ResolvedRequest.secretRefs`. The result model and every log writer replace
  those exact substrings with `***`. Secrets never appear in a persisted result, a
  diagnostic, or a log line.

---

## 9. Pipeline execution model

Phases run in fixed lifecycle order; within a phase, entries run in array order.
Deterministic, no reordering.

```
1. beforeRequest : hooks -> RequestPatch, merged into the IR in order.
2. (send HTTP, or run the mock — §10)
3. afterResponse : assertions -> AssertionResult[]; extractors -> RuntimePatch.
4. onError       : runs iff a hook threw, the send failed, or an assertion asset itself threw
                   (NOT for a merely failed assertion). Gets ctx.error.
5. finally       : always runs, even after onError. Cleanup only.
```

Contracts and merging:

- **`beforeRequest` hooks** return a `RequestPatch`. The runner applies patches
  sequentially. Headers are **upserted by case-insensitive name**; scalar fields
  (`url`, `body`) are **last-write-wins**. When two hooks in the same phase write the
  same header or `url`/`body`, the runner keeps the later one **and emits a
  `PIPELINE_CONFLICT` warning** naming both assets. Silent clobbering is never allowed.
- **`afterResponse` assertions** return results; the runner appends them. A failed
  assertion does **not** throw and does **not** stop the phase — every assertion runs so
  you see all failures. The request's overall status becomes `failed` if any assertion
  failed.
- **`afterResponse` extractors** return `RuntimePatch`. Runtime keys are merged
  last-write-wins with a `PIPELINE_CONFLICT` warning on collision. The runtime snapshot in
  `ctx` is frozen per phase, so assertions in the same phase see a consistent view;
  extracted values become visible to the *next request* in the run, not to peers in the
  same phase (removes intra-phase ordering surprises).
- **Error propagation.** A hook or extractor that throws aborts the remaining entries in
  its phase, skips straight to `onError`, then `finally`. An assertion that throws is
  treated as a broken asset (→ `onError`), distinct from an assertion that returns
  `passed: false`.
- **Cancellation.** `ctx.abortSignal` fires on run cancellation or per-asset timeout
  (§15). A well-behaved asset checks it; the runner enforces the timeout regardless by
  terminating the Worker.
- **Immutability.** Assets receive a frozen `ctx` and return patches. All mutation of the
  IR and runtime happens in the runner, after validating each patch against its contract.

Is 4 phases enough? Yes for v1: auth/signing/dynamic-data (`beforeRequest`),
assertion/extraction (`afterResponse`), retry-or-report (`onError`), teardown
(`finally`). `beforeResolve` was dropped — a hook running before bindings resolve has no
resolved request, no response, and almost no useful input; generators already cover
"produce data before the request." Add it later only with a concrete use case.

### Persisted sequences

A sequence is a small, versioned project artifact, not an IDE-only selection:

```json
{
  "$schema": "./schemas/sequence-v1.schema.json",
  "formatVersion": 1,
  "kind": "sequence",
  "meta": { "id": "smoke", "name": "Smoke" },
  "requests": [
    "requests/auth/login.request.json",
    "requests/users/me.request.json"
  ]
}
```

Entries are project-relative `*.request.json` paths and cannot escape the
project. They run in array order and thread `${runtime.*}` forward. “Run
affected” is intentionally different: it runs every consumer independently,
including its matrix, so unrelated tests cannot exchange runtime state.

### Jira links and folder inheritance

Story folders may contain a versioned `.forge-jira` file with either a Jira
key (`API-123`) or full ticket URL. Requests and child folders inherit the
nearest ancestor link. A test-specific override is stored beside the request
as `.<request-file>.forge-jira`; removing it restores folder inheritance.
These files contain only the link, never Jira credentials or remote state. The
GUI stores full URLs so tickets open directly without a Jira host setting;
key-only files remain readable for compatibility.

### Environment defaults and inheritance

Project and story folders can select a default environment in a versioned
`.forge-environment` file. Descendants inherit the nearest selection; a
request-specific override lives beside the request as
`.<request-file>.forge-environment`. Removing an override restores inheritance.
The files contain only the environment name, never variable values or secrets.
An explicit GUI or CLI environment selection takes precedence for that run.

---

## 10. Mock execution model

Mocks reuse the exact binding and asset-resolution machinery. Two forms (schema `oneOf`):

- **Static** — `{ status, headers?, body?, delayMs? }`, where `body` may be a `ref`. The
  body resolves through §6 like any other reference.
- **Executable** — `{ use: "project:mocks/...@1", with }` — a `mock` asset returns a
  `MockResponse` from request context.

Decided behavior:

- **Same pipeline lifecycle.** A mock replaces only the *send* step. `beforeRequest`
  hooks still run (so a mock exercises your auth/signing path). `afterResponse`
  assertions and extractors run **against the mock response** — meaning your assertions
  are tested against your own fixtures, and a drifting mock fails its own tests. This is
  the main reason to unify mock and real execution.
- **Input context.** A mock asset gets `ctx.request` (the resolved request), the matched
  route params (if invoked via the mock server), and its `with`. It does not get a live
  `response`.
- **Response contract.** `MockResponse` (status/headers/body/delayMs). `delayMs`
  simulates latency; a mock may set a 5xx status to simulate errors.
- **Determinism.** Static mocks are deterministic. Dynamic project scripts are trusted
  code; v1 does not currently lint `Date`/`Math.random`, so reproducibility remains the
  asset author's responsibility.
- **Validation.** If the mock's `body` ref has a sibling schema, it is validated on load —
  a malformed fixture fails before it is ever served.
- **Matching/routing is out of the document.** The request document says *what* a mock
  returns, never *when* it is served. A separate mock-server config
  (`mocks.routes.json`, not part of `request-v1`) maps `METHOD path-template ->
  request-id`, deriving a default route from `request.method` + the path of
  `request.url`. Keeping routing external means the request format does not grow HTTP
  server concerns.

---

## 11. Alias resolution and path security

Two addressing modes, both first-class:

- **Relative path** — `"../assets/data/users.json#/valid/alice"`, resolved relative to
  the request file.
- **Alias** — defined in `project.json`:

```json
{
  "formatVersion": 1,
  "aliases": {
    "data:users":          "./assets/data/users.json",
    "data:tenants":        "./assets/data/tenants.json",
    "data:user-responses": "./assets/data/responses/users.json",
    "project:auth":        "./assets/hooks",
    "project:assertions":  "./assets/assertions",
    "project:extractors":  "./assets/extractors"
  }
}
```

Matching rules, decided:

- An alias whose target is a **file** is an **exact** alias (`data:users`). The ref must
  be exactly the alias, optionally followed by `#pointer` and/or `@version`.
- An alias whose target is a **directory** is a **prefix** alias (`project:assertions`).
  The remainder after the alias is a path *under* that directory
  (`project:assertions/user-created@1` → `<dir>/user-created.js@1`).
- **Exact beats prefix.** If both an exact alias and a prefix alias could match, exact
  wins.
- **Longest prefix wins** among competing prefix aliases.
- **Ambiguity is a load-time error.** If two aliases normalize to the same key, or an
  exact and prefix alias collide unresolvably, `project.json` fails validation — not at
  request time.
- **Extension inference** for executable assets: `.js` then `.ts` (first that exists).
  A lone `.ts` file resolves only to produce the explicit "transpile to .js"
  diagnostic. Data refs must name the file explicitly.

Path security:

- Refs use **forward slashes only**; a `\` is rejected (`INVALID_ALIAS`). Loading
  converts to the OS separator.
- Resolve `.`/`..`, canonicalize (including symlink targets), then assert the final
  absolute path is **inside the project root**. Anything else → `PATH_ESCAPE`. This holds
  for both relative paths and alias targets — an alias in `project.json` cannot point
  outside the project either.
- Moving an asset breaks its refs with a precise `ASSET_NOT_FOUND` naming the resolved
  path; the fix is to move the file or update one alias, never a manifest per asset.

No global manifest is required. An optional generated index (`.forge/index.json`) can
cache "alias/kind → path" for fast lookup and editor autocomplete, but it is
**rebuildable from the filesystem** and never the source of truth; a stale or missing
index only costs a rescan.

An executable may optionally have a colocated `<stem>.meta.json`. This is not a
registry: moving the executable moves its metadata. The file supplies `title`,
`description`, `intent`, optional `phase`, typed `parameters`, and an `example` for the
IDE form. The lockfile hashes both executable and metadata.

---

## 12. Execution pipeline (document → result)

```
Request JSON
  → 1. Schema validation           (request-v1.schema.json; fail closed)
  → 2. Reference resolution        (§6 — assets located, loaded, validated)
  → 3. Binding resolution          (§7 — bindings + matrix, cycle-checked)
  → 4. Variable resolution         (§8 — namespaced, type-preserving)
  → 5. Canonical IR                (§4 — fully resolved ResolvedRequest)
  → 6. Pipeline: beforeRequest     (§9 — hooks patch the IR)
  → 7. HTTP send   OR   mock       (§10)
  → 8. Pipeline: afterResponse     (§9 — assertions + extractors)
       (onError / finally as applicable)
  → 9. Result model                (§17)
```

Stages 1–5 are pure and side-effect-free except reads; they can run in CI as a "validate
only" pass (`forge validate`) that never touches the network. Stages 6–8 are the run.

---

## 13. Parameterized dataset execution

`matrix` is the parameterization primitive — separate from `bindings` so single values
stay single and iteration is explicit.

```json
{
  "matrix": {
    "case": { "ref": "data:create-user-cases#/cases" }
  },
  "bindings": {
    "requestId": { "use": "builtin:uuid@1" }
  },
  "request": {
    "method": "POST",
    "url": "${env.baseUrl}/users",
    "body": { "type": "json", "value": "${matrix.case.payload}" }
  },
  "pipeline": []
}
```

The sibling assertion document uses
`{"use":"builtin:assert-status@1","with":{"expected":"${matrix.case.expectedStatus}"}}`.

Where `data:create-user-cases#/cases` is an array. Decided semantics:

- Each `matrix` binding must resolve to an **array**; the request runs **once per
  element** (cartesian product if multiple matrix names — kept but discouraged; document
  it).
- Inside an iteration, `${matrix.case}` is that element. `bindings`, `pipeline` `with`,
  and expected values all reference `${matrix.case.*}` normally.
- **Runtime is per-iteration and isolated** — extractions from case 1 do not leak into
  case 2. (A shared setup belongs in a preceding request in the run, not the matrix.)
- `bindings` resolve **per iteration** too, so `builtin:uuid@1` yields a fresh id each
  case.
- The result is an **array of per-case results**, each tagged with the matrix values
  (masked as usual), so a CI report reads "case `missingEmail`: expected 422, got 500".

This keeps one dataset file feeding N cases with zero duplication of assertions, hooks,
or expected values.

---

## 14. Environments and secret-provider boundaries

Non-secret environment values are normal committed JSON:

```json
{ "baseUrl": "http://localhost:3000", "timeout": 5000 }
```

Selected by name (`environments/local.json` → `${env.baseUrl}`). Environment files may
**not** contain secrets and may **not** reference `${secret.*}` (an env file resolving a
secret would persist it). The `secret` namespace resolves only through a provider.

Provider abstraction (small, one impl required for v1):

```ts
export interface SecretProvider {
  name: string;
  get(key: string): Promise<string | undefined>;
}
```

- **v1 default:** a `.env.local` provider (gitignored file, `KEY=value`) plus process
  environment fallback. That is the whole first version.
- **Later:** OS keychain provider, external providers (Vault, cloud secret managers) —
  same interface, resolved in a declared order from `project.json`
  (`"secrets": ["env", "keychain"]`). No implicit precedence: the order is written down.
- A missing secret is `MISSING_VARIABLE` like any other, but its *value* is never logged
  even on success.

Secrets never appear in request documents, environment files, results, diagnostics, or
the lockfile — only the *reference* `${secret.apiToken}` is persisted.

---

## 15. Executable asset security

Be honest: the current host is in-process QuickJS, not an adversarial sandbox.
It has a 128 MB memory limit and a 5 s interrupt budget. Assets receive only
deep-frozen JSON snapshots (`request`, optional `response`, `bindings`); there
is no Node `require`, filesystem, process, ambient environment or built-in
network API.

CLI and GUI refuse repository-owned executable assets by default.
`--allow-project-code` or the editor's **Allow project code** switch is an
explicit per-run trust decision. CI that enables project code must provide its
real isolation boundary (container and, where needed, an egress policy).

Do not add a pretend in-process security tier. Separate process/container
execution and capability declarations remain deferred until importing and
running genuinely untrusted assets is a demonstrated workflow.

---

## 16. Asset versioning and lockfile strategy

- **Document version:** every persisted document carries `formatVersion` + `kind`.
  The existing `forge-core` `*.request.json` is **v0**. `forge migrate` maps the
  representable request/auth/assertion/extractor subset onto v1 and refuses the
  entire conversion when a field (for example inline scripts or unsupported
  transport auth) would otherwise be lost.
- **Built-in asset versions (`@N`, required):** builtins ship with the tool and evolve
  across releases, so a request pins the contract it was written against. `assert-status@1`
  and `assert-status@2` can coexist; a request keeps working when the tool upgrades.
- **Project asset versions (`@N`, optional):** git already versions project assets, so the
  suffix is not required. It is **useful** in exactly one case: running two contract
  versions **side by side during a migration** — `user-created@1` and `user-created@2`
  living together while callers move over. Absent a suffix, the asset's own declared
  `version` is used. Recommendation: omit `@N` on project assets until a migration needs
  it; do not decorate every ref.
- **Are project version suffixes actually useful?** Mostly no — source-control revision
  plus an integrity hash (below) reproduce a project asset exactly. The suffix earns its
  place only for in-repo dual-versioning during a breaking change. So: keep it supported,
  do not encourage it.
- **Lockfile (`.forge/lock.json`, optional, off by default):** pins each resolved asset's
  absolute-ish path and a content **integrity hash** (sha256). Purpose: reproducible CI
  and detecting a fixture changing under you. It is a **cache/guard**, never the project
  definition — deletable and `forge lock`-rebuildable. Turn it on for release CI or
  shared fixture suites; leave it off for day-to-day local work.

---

## 17. Result and diagnostic models

```ts
export interface RunResult {
  requestId: string;
  status: "passed" | "failed" | "error";   // failed = assertion(s) failed; error = threw/transport
  matrixCase?: Record<string, unknown>;     // masked
  http?: HttpResultView;                     // status, headers, timing, sizes; body optional/on-demand
  assertions: AssertionResult[];
  runtime: Record<string, unknown>;          // extracted this run (masked)
  diagnostics: Diagnostic[];
  startedAt: string;                         // ISO-8601
  durationMs: number;
}

export interface Diagnostic {
  severity: "error" | "warning" | "info";
  code: string;                              // e.g. "ASSET_NOT_FOUND", "PIPELINE_CONFLICT"
  message: string;
  instancePath?: string;                     // JSON Pointer into the request document
  assetRef?: string;                         // the offending ref/use, if any
}
```

- One `RunResult` per request, or an array of them for a `matrix` run.
- Secrets are masked everywhere (`ResolvedRequest.secretRefs` drives redaction).
- Response **history is not persisted in the request document**. The IDE writes
  it to `.forge-local/history.sqlite`; CLI output/JUnit remain separate artifacts,
  keeping request files clean and diff-friendly.
- The same `RunResult`/`Diagnostic` types back both the CLI (`forge run`, JUnit output)
  and the IDE; the GUI renders them but the types are UI-agnostic.

---

## 18. Complete end-to-end example

Project (abridged — see §19 for the full tree):

```
project.json
environments/local.json          { "baseUrl": "http://localhost:3000", "timeout": 5000 }
.env.local                       API_TOKEN=... (gitignored)
assets/data/users.json           { "valid": { "alice": { "name": "Alice", "email": "alice@example.com" } } }
assets/data/tenants.json         { "default": { "id": "t-1" } }
assets/data/responses/users.json { "created": { "id": "u-1", "name": "Alice" } }
assets/hooks/service-token.js    hook -> RequestPatch header Authorization
assets/assertions/user-created.js  assertion -> checks response.name === input.expectedUser.name
assets/extractors/user-id.js     extractor -> { runtime: { userId: response.id } }
requests/users/create.request.json   (the §1 document)
```

Run of `requests/users/create.request.json` against `local`:

1. **Validate** against `request-v1.schema.json` → ok.
2. **Resolve refs:** `data:users#/valid/alice` → `{name:"Alice",email:"alice@example.com"}`;
   `data:tenants#/default` → `{id:"t-1"}`. Both validated against sibling schemas.
3. **Resolve bindings:** `requestId` runs `builtin:uuid@1` → `"3f1c…"`.
4. **Resolve variables:** `url` → `"http://localhost:3000/users"`; `X-Request-ID` →
   `"3f1c…"`; body →
   `{"name":"Alice","email":"alice@example.com","tenantId":"t-1"}` (types preserved).
5. **Canonical IR** produced.
6. **beforeRequest:** `project:auth/service-token@1` reads the already resolved
   secret-backed binding and returns a `RequestPatch` upserting
   `Authorization: Bearer ***`. (Masked in results.)
7. **Send** `POST /users` → `201 { "id": "u-1", "name": "Alice" }`.
8. **afterResponse:** `builtin:assert-status@1` (expected 201) → pass;
   `project:assertions/user-created@1` (expectedUser=`{name:"Alice",…}`) → pass;
   `project:extractors/user-id@1` → `runtime.userId = "u-1"`.
9. **Result:** `status: "passed"`, `assertions: [pass,pass]`, `runtime:{userId:"u-1"}`,
   `http:{status:201,…}`, secrets masked.

Same document, mock mode: step 7 is replaced by the `mock` block
(`201` + `data:user-responses#/created`), steps 6 and 8 unchanged — so the assertions run
against the fixture and would fail if the fixture drifted from the contract.

---

## 19. Recommended project directory structure

```
project/
├── project.json                 # formatVersion, aliases, secret provider order
├── .env.local                   # gitignored secrets
├── environments/
│   ├── local.json
│   └── staging.json
├── requests/
│   └── users/
│       ├── create.request.json    # exactly one request per file
│       ├── create.hooks.json      # automatically derived hook sidecar
│       └── create.assertions.json # automatically derived assertion sidecar
├── sequences/
│   └── smoke.sequence.json      # ordered, runtime-threaded request list
├── assets/
│   ├── data/
│   │   ├── users.json
│   │   ├── users.schema.json     # optional sibling validation
│   │   ├── tenants.json
│   │   └── responses/users.json
│   ├── generators/random-user.js
│   ├── hooks/service-token.js
│   ├── assertions/user-created.js
│   ├── assertions/user-created.meta.json
│   ├── extractors/user-id.js
│   └── mocks/create-user-response.js
├── schemas/
│   ├── request-v1.schema.json
│   ├── hooks-v1.schema.json
│   ├── assertions-v1.schema.json
│   ├── sequence-v1.schema.json
│   └── asset-metadata-v1.schema.json
├── mocks.routes.json            # optional: mock server routing (NOT part of request-v1)
└── .forge/                      # generated, gitignorable
    ├── index.json               # rebuildable alias/kind cache
    └── lock.json                # optional integrity lockfile
```

`schemas/` and `assets/schemas/` are distinct: the former holds the *format* schema, the
latter holds *data* schemas. Data-asset schemas live as siblings (`users.schema.json`
next to `users.json`) so a fixture and its contract move together.

---

## 20. Trade-offs and rejected alternatives

- **`matrix` vs iterate-in-bindings.** Rejected auto-detecting an array in `bindings` and
  iterating: it makes "is this one value or N runs?" depend on the data, which is
  non-obvious in review. `matrix` is explicit. Cost: one more top-level field.
- **4 vs 5 pipeline phases.** Dropped `beforeResolve`. A pre-resolution hook has no
  resolved request and no response; generators already produce pre-request data. Cheap to
  add back with a real use case.
- **6 vs 7 asset kinds.** Merged `transformer` into `hook`. A transformer is a
  `beforeRequest` hook returning a `RequestPatch`; a separate kind adds a contract with no
  new capability. Re-split only if transform assets need a distinct signature.
- **Type-preserving whole-expression interpolation.** Chosen over always-stringify because
  numeric/boolean/object payloads are the common case (timeouts, expected values, whole
  objects passed as `with`). Cost: one branch in the interpolator and a coercion table
  for embedded expressions.
- **No re-scan of data-asset content for `${}`.** Chosen for determinism and to prevent
  data-driven injection. Cost: you cannot put a live variable expression *in* a data
  fixture; if you need that, model it as a generator or a `with` input, which is where
  computed values belong.
- **JSON Patch on refs.** Kept (standard, tiny, covers "same fixture, one field
  different"). It overlaps with pointer-plus-override, but RFC 6902 is a known quantity
  and cheaper than inventing an override syntax.
- **Optional lockfile, off by default.** Rejected making it mandatory: git already pins
  project assets; a mandatory lockfile becomes a second source of truth to keep in sync.
  It is a CI-reproducibility guard, opt-in.
- **No in-process "secure" VM.** Rejected `vm2`-style sandboxing as the security story —
  documented history of escapes. The honest boundary is a separate process/container for
  untrusted code; in-process limits are for accidents.
- **Routing outside the request document.** Rejected putting mock match/route rules in the
  request file: it would drag HTTP-server concerns (path templates, precedence, wildcards)
  into a format that should describe one request. A separate routes file keeps the
  document about *what*, not *when*.

---

## 21. Minimal first version vs. later extension path

**Ship first (the smallest thing that is actually usable and CI-runnable):**

- `request-v1` schema + validation (fail-closed).
- Binding shapes: `value`, `ref` (with JSON Pointer + JSON Patch), `use` (generators
  only in bindings).
- Reference resolution (§6) with the full diagnostic set and both cycle detectors.
- Variable resolution (§8): `env`, `secret`, `bindings`, `runtime` — strict,
  type-preserving, masking. (`matrix` namespace can land with the matrix feature.)
- Pipeline: 4 phases; builtins `uuid`, `assert-status`, `assert-json-path`,
  `extract-json-path`, `bearer`, `basic`. Project hooks/assertions/extractors as
  plain `.js` modules.
- Aliases + relative paths + path-escape guard.
- Secret provider: `.env.local` + process env only.
- Executable assets: in-process QuickJS with timeout, memory cap, deep-frozen
  context and explicit adapter-level trust confirmation.
- `RunResult`/`Diagnostic`; JUnit output. `forge validate` (stages 1–5, no network) and
  `forge run`.

**Defer until a real need appears (each is additive, no format break):**

- `matrix` parameterization (adds the `matrix` field + namespace).
- Executable mocks + the mock server + `mocks.routes.json` (static mocks can ship in v1).
- Lockfile + integrity hashes; generated index cache.
- Additional secret providers (keychain, external) via the existing interface.
- Restricted/untrusted execution tiers (process/container isolation, import allowlists).
- More builtins (`assert-schema`, `assert-header`, signing hooks).
- `beforeResolve` phase and a `transformer` kind, only with concrete demand.

**Most likely to be overengineered, and the simple robust answer:**

- *Security.* Do not claim an in-process sandbox. v1 = QuickJS limits for
  explicitly trusted project code; real isolation is a container in CI.
- *Versioning.* Do not decorate every project ref with `@N`. Git + optional lockfile
  reproduce assets. Suffix builtins only.
- *Providers/registries.* No central asset registry, no mandatory manifest, one secret
  provider to start. The filesystem is the registry; `project.json` aliases are the only
  indirection.
- *Phases/kinds.* 4 phases, 6 kinds. Resist adding lifecycle stages and asset kinds
  speculatively; each new phase/kind is a permanent contract.
```
