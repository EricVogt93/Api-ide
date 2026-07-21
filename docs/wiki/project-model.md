# Project model

Forge projects are ordinary directories. The filesystem is the source of truth; the
project tree, catalog index and Git view are derived from it.

## Files created for a new project

```text
my-api/
├── forge.json
├── project.json
├── collections/
├── environments/
├── requests/
├── sequences/
├── specs/
├── assets/
│   ├── data/
│   ├── hooks/
│   ├── assertions/
│   ├── extractors/
│   ├── generators/
│   └── mocks/
└── .gitignore
```

Creation also adds `.forge-local/`, `*.secrets.json` and `.env.local` to
`.gitignore`. `.forge/` is generated state and should also remain uncommitted. Request
documents may live anywhere below the root, but `requests/<story>/` is the convention.
Forge discovers request and sequence files recursively by the exact suffixes
`.request.json` and `.sequence.json`; it skips `.git`, `.forge`, `node_modules` and
`target` while indexing.

## The two root documents

`forge.json` marks a desktop workspace and holds transport/editor defaults:

```json
{
  "format": 1,
  "name": "Pet API",
  "settings": {
    "timeoutMs": 30000,
    "followRedirects": true,
    "maxRedirects": 10,
    "verifyTls": true,
    "openapiUrl": "specs/petstore.json"
  }
}
```

All `settings` fields are optional as a group. Defaults are 30 seconds, redirects on,
10 redirects and TLS verification on. Optional fields are `proxy` (`url`, `noProxy`),
`userAgent`, `tls` (`clientCert`, `clientKey`, `caBundle`) and `openapiUrl`. TLS paths
may be workspace-relative or absolute PEM paths.

`project.json` configures Request Format v1 resolution:

```json
{
  "formatVersion": 1,
  "aliases": {
    "data": "./assets/data",
    "project:assertions": "./assets/assertions",
    "project:extractors": "./assets/extractors"
  },
  "secrets": ["env"],
  "auth": {
    "request": "requests/auth/token.request.json",
    "tokenPath": "$.access_token",
    "lifetimeSeconds": 900,
    "refreshBeforeSeconds": 30,
    "applyTo": "requests/private"
  }
}
```

| Field | Meaning |
| --- | --- |
| `formatVersion` | Optional numeric project-format marker; new projects write `1`. |
| `aliases` | Alias-to-project-path map used by every `ref` and `use`. |
| `secrets` | Declared secret-provider order. Empty currently falls back to environment lookup. |
| `auth` | Optional project-wide short-lived bearer-token provider. |

The auth defaults are `$.access_token`, 900 seconds, a 30-second refresh reserve and
`requests` for `applyTo`. `request` must end in `.request.json`; `request` and `applyTo`
must be non-empty project-relative paths without `..`; the refresh reserve must be less
than the lifetime. See [Authentication](authentication.md) for refresh behavior.

The CLI locates a Request Format v1 root by walking upward to the nearest
`project.json`. The core runner can operate without that file using an empty config,
but a generated Forge workspace contains both root documents.

## Environment files

Request Format v1 environments are committed JSON objects named
`environments/<name>.json`:

```json
{
  "baseUrl": "https://staging.example.test",
  "timeoutMs": 5000
}
```

Selecting `staging` loads exactly `environments/staging.json` and exposes it as
`${env.*}`. The name must be one path component: `staging` is valid; `../staging` and
`team/staging` are rejected. Secret values belong behind `${secret.NAME}`, never in
this file.

`collections/` additionally supports the older collection workspace model:
`collections/<collection>/collection.json`, optional `folder.json`, and
`environments/<name>.env.json` with secret values in the gitignored sibling
`<name>.secrets.json`. Do not mix that wrapper-shaped environment document with the
plain JSON object consumed by Request Format v1.

## Folder and request inheritance

Three properties use small plain-text files. Lookup starts at the selected request or
folder and walks toward the project root; the nearest non-empty value wins.

| Property | Folder/root file | Request-specific file | Contents |
| --- | --- | --- | --- |
| Environment | `.forge-environment` | `.<request-file>.forge-environment` | One environment name |
| OpenAPI | `.forge-openapi` | `.<request-file>.forge-openapi` | Workspace-relative path or URL |
| Jira | `.forge-jira` | `.<request-file>.forge-jira` | Jira key or full URL |

For `requests/orders/create.request.json`, the request-level files are therefore
`.create.request.json.forge-environment`, `.create.request.json.forge-openapi` and
`.create.request.json.forge-jira` in the same directory. Removing an override restores
inheritance; it does not copy the parent's value. An explicit environment selected for
one GUI/CLI run outranks all inherited environment files. OpenAPI and Jira use nearest
ancestor only. Jira's displayed label is the final URL segment, so a full
`https://jira.example/browse/API-123` link appears as `API-123`.

Example:

```text
requests/
├── .forge-environment                 # staging
└── checkout/
    ├── .forge-openapi                 # specs/checkout.yaml
    ├── .forge-jira                    # SHOP-120
    ├── create.request.json            # inherits all three
    └── .create.request.json.forge-jira # SHOP-124; overrides Jira only
```

## Sequences and regression selection

A `*.sequence.json` stores an ordered, project-relative list of requests:

```json
{
  "formatVersion": 1,
  "kind": "sequence",
  "meta": { "id": "checkout.smoke", "name": "Checkout smoke" },
  "requests": [
    "requests/auth/login.request.json",
    "requests/checkout/create.request.json"
  ]
}
```

Entries must end in `.request.json`, exist, remain below the project root and execute in
array order. Runtime values captured by one request are available to later requests as
`${runtime.*}`. A request is a regression test when `meta.tags` contains
`"regression"` case-insensitively; there is no separate regression sidecar.

## Git behavior

Project files, property files, sidecars and asset metadata are intentionally
reviewable. The Files/Git views show the same working tree, and branch/worktree actions
operate on that repository. Commit behavior-bearing sidecars with their request and
commit a moved asset with any alias update. Never commit `.forge-local/`, `.forge/`,
`.env.local` or `*.secrets.json`.
