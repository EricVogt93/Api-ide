# Request Format v1 and sidecars

One `*.request.json` describes one HTTP request. Assertions and hooks use derived
sibling names so the editor can keep the request, checks and lifecycle behavior in
separate tabs without losing one executable model.

The authoritative schemas are
[`request-v1.schema.json`](../../schemas/request-v1.schema.json),
[`assertions-v1.schema.json`](../../schemas/assertions-v1.schema.json) and
[`hooks-v1.schema.json`](../../schemas/hooks-v1.schema.json). Unknown fields are rejected
by the v1 Rust models; `formatVersion` must be the integer `1`.

## Complete request example

```json
{
  "$schema": "../../../schemas/request-v1.schema.json",
  "formatVersion": 1,
  "kind": "request",
  "meta": {
    "id": "pets.create",
    "name": "Create pet",
    "description": "Creates one reusable fixture",
    "tags": ["pets", "regression"]
  },
  "bindings": {
    "pet": { "ref": "data:pets#/primary" },
    "requestId": { "use": "builtin:uuid@1" }
  },
  "request": {
    "method": "POST",
    "url": "${env.baseUrl}/pets",
    "headers": [
      { "name": "Content-Type", "value": "application/json" },
      { "name": "X-Request-Id", "value": "${bindings.requestId}" }
    ],
    "query": [
      { "name": "dryRun", "value": "true", "enabled": false }
    ],
    "body": {
      "type": "json",
      "value": { "name": "${bindings.pet.name}" }
    }
  },
  "mock": {
    "status": 201,
    "body": { "type": "json", "value": { "id": 42, "name": "Demo" } }
  }
}
```

## Top-level fields

| Field | Required | Contract |
| --- | --- | --- |
| `$schema` | No | Editor hint only; any string. |
| `formatVersion` | Yes | Exactly `1`. |
| `kind` | Yes | Exactly `"request"`. |
| `meta` | Yes | Identity and tags. |
| `bindings` | No | Named single values resolved before the request. |
| `matrix` | No | Named arrays producing parameterized runs. |
| `request` | Yes | HTTP method, URL, headers, query and optional body. |
| `pipeline` | No | Inline compatibility form; canonical IDE storage uses sidecars. |
| `mock` | No | Static or executable replacement for the send step. |

`meta.id` must match `[a-zA-Z0-9._-]+` and be unique within the project. `name` is a
non-empty display name; `description` and string `tags` are optional. The Properties
checkbox **Regression test** adds/removes the canonical `regression` tag.

## HTTP request fields

`request.method` is one of `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS`
or `TRACE`; `request.url` is a non-empty template string. `headers` and `query` are
ordered arrays of `{ "name", "value", "enabled"? }`. `enabled` defaults to `true`.

A body is either inline or referenced:

```json
{ "type": "json", "value": { "active": true } }
```

```json
{ "ref": "data:payloads#/valid", "type": "json" }
```

Inline `type` is `json`, `text`, `form`, `multipart`, `binary` or `none`; `value` is
optional and preserves its JSON type. The current runner executes `json`, `text`, `form`
and `none`; `multipart` and `binary` are schema-reserved but currently return an explicit
unsupported-body diagnostic. A referenced body may persist an optional type, but the
current resolver emits the loaded value as JSON. Form objects become name/value pairs.
Body refs use the same project-contained resolution as bindings.

## Bindings

Every entry in `bindings` and `matrix` has exactly one of three shapes:

```json
{
  "literal": { "value": 5000 },
  "fixture": {
    "ref": "data:pets#/primary",
    "patch": [{ "op": "replace", "path": "/name", "value": "Ada" }]
  },
  "generated": {
    "use": "project:generators/demo-id",
    "with": { "prefix": "pet" }
  }
}
```

- `value` is a request-local JSON value and is interpolated.
- `ref` loads JSON, validates an optional `<stem>.schema.json`, selects an optional
  RFC 6901 JSON Pointer, clones it, then applies RFC 6902 `patch` operations in order.
  Loaded asset content is deliberately not re-interpolated.
- `use` invokes a builtin generator or trusted project JavaScript with the interpolated
  `with` object.

Bindings are resolved in dependency order. `${bindings.b}` inside binding `a` creates a
real dependency; cycles fail with `BINDING_CYCLE`. Independent resolution errors are
collected before execution.

`matrix` uses the same shapes, but each resolved value must be an array. ApiWright executes
one case per element; multiple names form a Cartesian product. `${matrix.case}` points
to the current element. Bindings are rebuilt per case, and runtime writes do not leak
between cases.

## Variables and types

Supported namespaces are:

| Expression | Source |
| --- | --- |
| `${env.baseUrl}` | Selected `environments/<name>.json` |
| `${secret.API_TOKEN}` | Configured secret provider |
| `${bindings.pet.id}` | Resolved local binding |
| `${matrix.case.id}` | Current matrix case |
| `${runtime.createdId}` | Earlier extractor/sequence output |

A whole-string expression preserves type: `"${bindings.timeout}"` can become the JSON
number `5000`, and `"${bindings.payload}"` can become an object. Embedded expressions
produce strings and accept only strings, numbers and booleans; embedding `null`, arrays
or objects is an error. Missing variables are errors. `$${env.name}` escapes to the
literal `${env.name}`. Object keys are never interpolated, and there is no implicit
namespace precedence.

## Asset references

The implemented reference shape is `<address>[@version][#json-pointer]`, for example:

```text
builtin:uuid@1
data:pets#/primary
project:assertions/pet-name
../../assets/data/pets.json#/primary
```

Use forward slashes. A file alias is exact; a directory alias is a prefix and the
longest prefix wins. Relative paths start at the request's directory. Executable refs
without an extension try `.js`, then `.ts`; TypeScript is indexed but v1 execution
requires transpiled `.js`. Canonicalization rejects any alias, relative path or symlink
that escapes the project root. See [Catalog](catalog.md#addressing-assets) for the asset
side.

## Assertion sidecar

For `create.request.json`, assertions are stored in `create.assertions.json`:

```json
{
  "formatVersion": 1,
  "kind": "assertions",
  "assertions": [
    {
      "use": "builtin:assert-status@1",
      "with": { "expected": 201 },
      "enabled": true
    },
    {
      "use": "project:assertions/pet-name",
      "with": { "expected": "${bindings.pet.name}" }
    }
  ]
}
```

Each entry allows only `use`, optional `with` and optional `enabled` (default `true`).
Assertions are applied as `afterResponse` pipeline entries. An absent sidecar means an
empty list; saving an empty list removes the file.

## Hook sidecar

For the same request, hooks are stored in `create.hooks.json`:

```json
{
  "formatVersion": 1,
  "kind": "hooks",
  "hooks": [
    {
      "phase": "beforeRequest",
      "use": "builtin:header@1",
      "with": { "name": "X-Trace-Id", "value": "${bindings.requestId}" }
    },
    {
      "phase": "afterResponse",
      "use": "builtin:extract-json-path@1",
      "with": { "path": "$.id", "target": "createdPetId" }
    }
  ]
}
```

Phases are `beforeRequest`, `afterResponse`, `onError` and `finally`; entries also allow
optional `with` and `enabled`. Missing files become empty documents, and empty documents
are removed on save. Loading builds one effective request by applying hook entries and
then assertions. An exact duplicate—same phase/reference/input/enabled state—is added
only once. Inline `pipeline` remains accepted for compatibility; the IDE separates its
validation entries into assertions and the remaining entries into hooks when saving.

## Mocks

A static mock is `{ "status", "headers"?, "body"?, "delayMs"? }`; status is 100–599
and delay is non-negative. A dynamic mock is
`{ "use": "project:mocks/name", "with": {...} }`. Mock mode replaces only the network
send: before-request hooks still modify the request, and after-response assertions and
extractors run against the mock response.

## Resolution and execution order

```text
parse/schema-check → resolve refs → resolve bindings/matrix → interpolate
→ beforeRequest → HTTP or mock → afterResponse → onError/finally → result
```

Use **Validate** to catch document, reference and OpenAPI errors without relying on a
successful request. The core reports an instance path such as `/bindings/pet` or
`/pipeline/0/use`, which lets the editor underline the responsible field.
