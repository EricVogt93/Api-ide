# Examples

## `demo-workspace/`

A small, complete Forge workspace that exercises the on-disk format against
the public [httpbin.org](https://httpbin.org) test service. Open the
`demo-workspace` folder directly in the Forge IDE, or run it headlessly:

```sh
forge run examples/demo-workspace --env httpbin
```

What it contains:

- **`forge.json`** — the workspace marker (name `Demo`).
- **`environments/httpbin.env.json`** — one environment (`httpbin`) with a
  plain `baseUrl` variable and a *declared* secret, `apiToken` (no value is
  committed — see below).
- **`collections/httpbin/`** — a collection named `HTTPBin` with an explicit
  `order` array and three top-level requests plus a `status-codes` folder:
  - `get-json.request.json` — `GET /json`, asserts status, content type,
    a JSONPath and response time.
  - `post-echo.request.json` — `POST /post` with a JSON body containing the
    `{{$uuid}}` dynamic variable; asserts the echoed payload and extracts
    `$.json.id` into a runtime variable (`echoedId`) for use by later
    requests in the same run.
  - `auth-bearer.request.json` — `GET /bearer` using bearer auth sourced
    from the secret `{{apiToken}}` variable.
  - `status-codes/get-404.request.json` — `GET /status/404`, asserting a
    `404` response.

### Secrets

`apiToken` is declared in `httpbin.env.json` with `"secret": true` and no
value, as secrets are never committed. To run the `Auth Bearer` request,
create the gitignored sibling file `environments/httpbin.secrets.json`:

```json
{
  "apiToken": "any-non-empty-value"
}
```

(httpbin's `/bearer` endpoint accepts any non-empty bearer token.)
