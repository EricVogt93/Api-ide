# Import, export and portability

ApiWright supports two different export goals: executable code snippets for sharing a request with another language/tool, and lossless bundles for moving ApiWright projects without losing sidecars or metadata.

## Lossless bundles

Use the project-tree context menu or CLI:

```sh
apiwright export requests/orders/get.request.json --root . --format json -o get-order.forge.json
apiwright export requests/orders --root . --format curl -o orders.forge.sh
apiwright import orders.forge.sh requests/imported
```

A single-request export includes its `.assertions.json` and `.hooks.json` siblings. A folder export walks descendants and retains request files, sidecars and project metadata inside that scope. UTF-8 remains readable; binary files alone use Base64. Secret providers (`.env.local`, `*.secrets.json`) and runtime state are excluded.

JSON bundles identify themselves as `forge.bundle` format version 1. A ApiWright cURL bundle is both an executable shell representation and a lossless bundle encoded in marked comments. Importing either restores the complete ApiWright files. Paths are validated against traversal, duplicate paths are rejected, and any existing destination collision aborts before writing.

## Code snippets

The request toolbar can render cURL, HTTPie, JavaScript `fetch`, Axios, Python `requests`, Go and Java. These are transport snippets, not backups: catalog references, assertions, hooks and folder properties are not representable in ordinary cURL or language snippets.

## Imports

- **cURL:** paste a command; ApiWright maps method, URL, headers, query and supported bodies.
- **OpenAPI:** select operations from JSON/YAML and generate request skeletons.
- **Postman:** import collections or environments; secret environment values are written separately.
- **Bruno:** import folder structure, requests and optionally environments. Bruno exports do not contain secret values.
- **ApiWright bundle:** restore a lossless JSON or ApiWright-generated cURL bundle.

Imported files are ordinary project files. Review and format them before committing, especially scripts or authentication values from third-party exports.
