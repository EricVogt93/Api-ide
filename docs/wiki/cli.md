# CLI and CI

The `forge` CLI runs the same request-v1 engine as the GUI:

```sh
forge ci requests/checkout --root . --env staging
forge ci --root . --regression
forge validate requests/users/list.request.json --root .
forge export requests/checkout --format json -o checkout.forge.json
forge import checkout.forge.json requests
```

Targets may be files or folders. Folder execution is recursive and stable by path. `--regression` runs only requests marked Regression in Properties. `--mock` uses request-owned mocks; `--frozen` verifies `.forge/lock.json`; `--allow-project-code` explicitly permits project JavaScript.

Exit codes are CI-friendly: `0` passed, `1` assertion failure, `2` configuration or execution error. JSON exports preserve assertions, hooks, auth and metadata. cURL exports are Forge bundles with a runnable representation and lossless re-import data.
