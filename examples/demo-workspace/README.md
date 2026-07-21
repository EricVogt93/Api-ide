# Forge Demo Workspace

Open this folder in Forge to explore the complete project workflow without
creating configuration first. Every modern request has an offline mock, so the
feature tour remains deterministic even without network access.

## What to explore

- `requests/pets/` inherits its OpenAPI source and Jira link from the folder.
- `list.request.json` demonstrates query parameters, response headers, delay,
  built-in assertions, extractors and a regression tag.
- `create.request.json` combines data refs, generated bindings, a JSON body,
  custom catalog assets, a dynamic mock, assertion and hook sidecars.
- `delete.request.json` shows path bindings, a `204 No Content` mock and cleanup
  assertions.
- `by-id-matrix.request.json` executes once per row in `pet-cases.json`.
- `requests/auth/` demonstrates a short-lived project auth request and an
  automatically authenticated consumer.
- `demo.sequence.json` executes a complete ordered journey.
- `specs/petstore.json` powers OpenAPI completion, coverage and generators.
- `collections/httpbin/` keeps the legacy workspace/import format visible.

Run the modern project offline:

```sh
forge ci requests \
  --root examples/demo-workspace --env demo --mock --allow-project-code
forge ci requests \
  --root examples/demo-workspace --env demo --mock --regression --allow-project-code
```

The View → User tour explains the Project tree, catalog, request editor,
OpenAPI tools, response tabs, bottom tools, environments and status bar.
