# OpenAPI

Forge treats an OpenAPI 3.x document as active editor tooling: source inheritance, operation browsing, completion, request/response diagnostics, coverage, and generated suites all use the same parsed spec.

## Choose a source

There are three supported paths:

1. Put `openapi.json`, `openapi.yaml`, `openapi.yml`, `swagger.json`, `swagger.yaml`, or `swagger.yml` in the project root, or any JSON/YAML file below `specs/`. With no explicit source, Forge scans these candidates and uses the first valid spec in sorted order.
2. Right-click the project, a folder, or a request, choose **Properties…**, and enter an HTTP(S) URL or project-relative path such as `specs/petstore.json` under **OpenAPI source**.
3. Set the project fallback in **Settings → HTTP → OpenAPI**.

Request and folder properties inherit upward to the project root. The closest explicit value wins; clearing a value restores inheritance. A scoped property takes precedence over the `forge.json` fallback. Remote sources are fetched asynchronously and must return success; local configured paths are resolved below the project root.

## Browse operations

Expand the right tool window and select the code-shaped **OpenAPI** tab. The header shows title, version, operation count, first server, and actions to open the source. **Open Swagger UI** appears when the document provides an HTTP(S) `x-swagger-ui-url` or `externalDocs.url`.

Search matches operation path, summary, operation ID, and tags. The dropdown filters by HTTP method or operations with headers, query parameters, path parameters, or a request body. Results are sorted by method and then path.

Each operation card shows:

- colored method and path;
- `sch`, `req`, or `opt` (`sch` = body schema, `req` = required input, `opt` = no required input);
- summary, when supplied;
- **Add to request**, which applies method/path, path bindings, required query/header inputs, content type, and an example/schema-derived body;
- **Generate custom value**, which also populates generated sample values;
- an unboxed checkmark for coverage.

Coverage is automatic when any indexed request matches the method and templated path. The current unsaved request is also considered. A manual checkmark is stored locally in `.forge-local/openapi-covered.json`; it does not replace an automatically covered mark.

## Editor completion and validation

The line below the request editor reports the matched operation or explains a mismatch. When paths are similar, up to five operation suggestions are shown; click one or press `Tab` for the first. **Apply fixes** adds missing required query parameters, headers, content type, path bindings, and a request body without discarding unrelated request fields.

Request diagnostics cover method/path matching, missing required inputs, content type, and inline JSON body schema. After a run, the Response tab checks the declared exact status, status-class fallback, or default response; it then checks response content type and JSON Schema when declared.

## Generate suites

Select the destination folder in Project, then choose a generator tab in the right tool window. If no folder is selected, Forge uses the active request folder and finally `requests/`.

| Tool | Output | Generated content |
| --- | --- | --- |
| Contract tests | `contract/` | one request per operation, status/content-type/schema assertion sidecars, sequence, source snapshot, manifest |
| API tests | `api/` | contract checks plus a 2-second response-time assertion, operation requests and ordered sequence |
| Load & performance | `performance/` | `operations.json`, `k6.js`, README, manifest; smoke/load/stress/spike/soak profiles |

Generation replaces a previous folder only when `manifest.json` identifies the same `forge-openapi` generator and kind. A pre-existing non-generated folder is left intact and generation fails. If generation itself fails, the incomplete generated directory is removed.

Generated Forge suites use the first HTTP(S) server or `http://localhost:3000`, sample required values, sidecar assertions, and tags from the operation. External response-schema references and wildcard-only response statuses produce warnings instead of unreliable assertions.

The k6 suite runs GET, HEAD, and OPTIONS by default. Mutating operations require an explicit opt-in against disposable data:

```sh
cd requests/story/performance
k6 run -e BASE_URL=https://staging.example.com -e PROFILE=load k6.js
k6 run -e BASE_URL=https://staging.example.com -e INCLUDE_MUTATIONS=true k6.js
```

Profiles default to a 1% failure-rate threshold and a 500 ms p95 latency threshold; edit the generated script when the service SLO differs.

## Troubleshooting

- **No OpenAPI spec found:** set a scoped source or add a supported root/specs file.
- **Invalid spec:** fix the first reported candidate or configure the intended file explicitly.
- **Path is not declared:** verify the effective server prefix and request path; variables in the host are supported, but method and templated path must match.
- **Generator disabled:** both a parsed spec and an existing project folder are required.
- **Existing folder was not generated by Forge:** rename it or select another parent; Forge intentionally will not overwrite it.
