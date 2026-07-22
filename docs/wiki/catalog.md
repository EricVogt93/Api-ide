# Catalog and reusable assets

The catalog is ApiWright's single point of concern for behavior that would otherwise be
copied into every test. A request stores only a stable `use` reference and test-specific
`with` values. Fixing the builtin or project asset changes every consumer without
rewriting request files.

Catalog entries are grouped by intent: **Validate**, **Prepare**, **Capture**,
**Generate** and **Simulate**. Metadata drives search, filtering and the typed form; the
runtime still validates the selected asset and its input before execution.

## Shipped builtins

All shipped references are version 1. `binding` entries belong in `bindings`/`matrix`;
the other entries belong in the named hook/assertion phase.

| Reference | Intent / target | `with` parameters | Behavior |
| --- | --- | --- | --- |
| `builtin:uuid@1` | Generate / binding | none | UUID v4 string |
| `builtin:now@1` | Generate / binding | none | Current Unix timestamp in seconds |
| `builtin:bearer@1` | Prepare / `beforeRequest` | `token: string` required; `prefix: string = "Bearer"` | Upserts `Authorization: <prefix> <token>` |
| `builtin:basic@1` | Prepare / `beforeRequest` | `username: string`, `password: string` required | Base64 Basic Authorization header |
| `builtin:header@1` | Prepare / `beforeRequest` | `name: string`, `value: string` required | Adds/replaces one request header |
| `builtin:assert-status@1` | Validate / `afterResponse` | `expected: integer` required | Exact response status |
| `builtin:assert-json-path@1` | Validate / `afterResponse` | `path: string` required; `operator = exists`; optional `value: JSON` | `exists`, `notExists`, `equals` or string `contains` |
| `builtin:assert-schema@1` | Validate / `afterResponse` | `schema: JSON` required | Response JSON against JSON Schema |
| `builtin:assert-header@1` | Validate / `afterResponse` | `name: string` required; optional `value: string` | Presence, or case-insensitive value equality |
| `builtin:assert-response-time@1` | Validate / `afterResponse` | `maxMs: integer > 0` required | Elapsed time must be strictly below the limit |
| `builtin:assert-body-text@1` | Validate / `afterResponse` | `text: string` required | Body contains literal text |
| `builtin:assert-body-regex@1` | Validate / `afterResponse` | `pattern: string` required | Body matches a validated regular expression |
| `builtin:assert-json-path-type@1` | Validate / `afterResponse` | `path`, `expected` required | Type is `null`, `boolean`, `number`, `string`, `array` or `object` |
| `builtin:assert-json-path-length@1` | Validate / `afterResponse` | `path`, `expected: integer` required; `operator = equals` | String character, array item or object member count; `equals`, `lt`, `lte`, `gt`, `gte` |
| `builtin:assert-cookie@1` | Validate / `afterResponse` | `name` required; optional `value` | `Set-Cookie` contains the named cookie, optionally with exact value |
| `builtin:assert-openapi-response@1` | Validate / `afterResponse` | exactly one of `spec: JSON` or `specRef: string`; optional `method`, `url` | Declared status, content type and response JSON Schema |
| `builtin:extract-json-path@1` | Capture / `afterResponse` | `path`, `target` required | Writes selected JSON to `runtime[target]` |
| `builtin:extract-header@1` | Capture / `afterResponse` | `name`, `target` required | Writes a response header string to `runtime[target]` |

`assert-openapi-response` defaults omitted `method` and `url` from the resolved current
request. `specRef` is resolved relative to that request and may point at JSON or YAML;
it cannot contain a JSON Pointer. The assertion requires exactly one of `spec` and
`specRef`.

Builtin validation rejects unknown parameters, missing required values, wrong JSON
types, unsupported options, invalid JSONPath/regex syntax, the wrong execution phase and
versions other than `@1`. `contains` specifically requires a string `value`.

## Inserting a reusable assertion

Selecting **Status is**, entering `201`, and inserting creates this sidecar entry:

```json
{
  "use": "builtin:assert-status@1",
  "with": { "expected": 201 },
  "enabled": true
}
```

Only `201` varies. The title, description, intent, target phase, parameter type and
example remain defined once in the builtin catalog. A capture entry works the same way:

```json
{
  "phase": "afterResponse",
  "use": "builtin:extract-json-path@1",
  "with": { "path": "$.id", "target": "createdPetId" }
}
```

Later requests in a sequence read the result as `${runtime.createdPetId}`. `target` is a
plain key, not the expression `runtime.createdPetId`.

## Project asset layout

Project assets are scanned recursively below `assets/`:

```text
assets/
├── data/
│   ├── customers.json
│   └── customers.schema.json
├── assertions/
│   ├── customer-shape.js
│   └── customer-shape.meta.json
├── hooks/add-signature.js
├── extractors/customer-id.js
├── generators/order-id.js
└── mocks/customer-created.js
```

All `.json` files except `*.schema.json` and `*.meta.json` are data assets. `.js`/`.ts`
files are classified by a containing `hooks`, `assertions`, `extractors`, `generators`
or `mocks` directory; executable files elsewhere are shown as generic executables.
TypeScript is discoverable but not executable in v1—transpile it to `.js`.

For `customers.json`, an optional `customers.schema.json` validates the whole data file
before a JSON Pointer is selected. A request-local JSON Patch operates on a clone, so it
cannot mutate the cache or another request's fixture.

## Executable contract

A project executable defines one global function:

```js
function run(ctx, input) {
  return {
    passed: ctx.response.status === input.expectedStatus,
    message: "status matches",
    expected: input.expectedStatus,
    actual: ctx.response.status
  };
}
```

`ctx` is frozen JSON containing `request`, optional `response`, and `bindings`; `input`
is the resolved `with` object. Return contracts are:

| Asset kind | Return value |
| --- | --- |
| Generator | Any JSON value |
| Hook (`beforeRequest`) | `{ url?, headers?: [{name,value}] }` |
| Assertion (`afterResponse`) | One `{passed,message,expected?,actual?,path?}` or an array |
| Extractor (`afterResponse`) | `{ runtime: { key: value } }` |
| Mock | `{ status, headers?, body?, delayMs? }` |

Project JavaScript runs in the bounded QuickJS host only after project-code trust is
granted. Treat it as trusted repository code; the runtime limit is not an adversarial
sandbox. See [Secrets and security](security.md).

## Metadata sidecar

An executable `customer-shape.js` may have `customer-shape.meta.json`:

```json
{
  "title": "Customer shape",
  "description": "Checks the stable customer response contract.",
  "intent": "validate",
  "phase": "afterResponse",
  "parameters": [
    {
      "name": "expectedTier",
      "label": "Expected tier",
      "kind": "string",
      "required": true,
      "default": "standard",
      "options": ["standard", "premium"],
      "example": "premium"
    }
  ],
  "example": { "expectedTier": "premium" }
}
```

| Field | Rule |
| --- | --- |
| `title` | Required, non-empty. |
| `description` | Optional string; defaults empty. |
| `intent` | Required: `validate`, `prepare`, `capture`, `generate` or `simulate`. |
| `phase` | Optional: `beforeRequest`, `afterResponse`, `onError` or `finally`. |
| `parameters` | Optional array; parameter names must be non-empty and unique. |
| `example` | Optional complete input object/value used by the form. |

Each parameter requires `name`, `label` and `kind`; kind is `string`, `integer`,
`boolean` or `json`. `required`, `default`, `options` and `example` are optional. The
metadata is catalog/UI data, not a registry and not executable behavior. Missing
metadata does not make the `.js` file disappear; invalid metadata is reported in the
project index.

## Addressing assets

Aliases live in `project.json`:

```json
{
  "aliases": {
    "data:customers": "./assets/data/customers.json",
    "project:assertions": "./assets/assertions"
  }
}
```

- A file target creates an exact ref: `data:customers#/primary`.
- A directory target creates a prefix ref:
  `project:assertions/customer-shape` resolves to `customer-shape.js`.
- Exact aliases win; otherwise the longest matching prefix wins.
- With no alias, use a forward-slash relative path from the request file.
- The final canonical path must remain inside the project root.

The index recommends an exact alias first, then a prefix alias, then a relative path.
It also records reverse usage so asset changes can be reviewed against every consumer.
The index is rebuildable and never the source of truth.

## Parameters stay local; behavior stays central

`with` values can use the same `${env.*}`, `${secret.*}`, `${bindings.*}`,
`${matrix.*}` and `${runtime.*}` expressions as the request. This is how two tests reuse
one assertion while naming their local bindings differently:

```json
{ "use": "project:assertions/customer-shape", "with": { "expected": "${bindings.createdCustomer}" } }
```

```json
{ "use": "project:assertions/customer-shape", "with": { "expected": "${runtime.customerFromSetup}" } }
```

The catalog centralizes the rule; each request explicitly maps its own semantic value
to that rule's input contract.
