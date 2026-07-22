# Authentication

ApiWright supports request-local authentication and a project auth fetcher for short-lived Bearer tokens. The latter runs an ordinary request, extracts a token, caches it, and refreshes before a protected call can outlive the remaining lifetime.

## Request-local authentication

For legacy request forms, the authorization selector supports inherit, none, Basic, Bearer, API key, Digest, NTLM, AWS Signature v4, and OAuth 2.0 Client Credentials. An enabled explicit `Authorization` header also counts as request-local auth.

For request-format v1, reusable `beforeRequest` catalog entries can add Bearer, Basic, or an `Authorization` header. Explicit request auth wins: project auth is not injected when such a header or enabled auth hook already exists.

## Reuse an existing request as project auth

1. Build and save a normal `*.request.json` that returns a token.
2. Open any request and select the bottom **Auth** tab.
3. Under **Source**, choose **Existing request**.
4. Select the auth request and choose **Use selected**. Alternatively, open the auth request itself and choose **Use current request**.
5. Expand **Token and refresh settings**, set the fields, and save.

The configuration lives in `project.json`:

```json
{
  "auth": {
    "request": "requests/auth/fetch-token.request.json",
    "tokenPath": "$.access_token",
    "lifetimeSeconds": 900,
    "refreshBeforeSeconds": 30,
    "applyTo": "requests/protected"
  }
}
```

`request` and `applyTo` must be project-relative and cannot contain `..`. `request` must end in `.request.json`; `tokenPath` must select a non-empty string. Lifetime must be positive and greater than the refresh reserve. `applyTo` accepts one request file or an entire request folder. The auth request never authenticates itself.

## Create a provider request

Choose **Provider setup** in the Auth tab. ApiWright creates a normal form-encoded Client Credentials request below `requests/auth/`, activates it, and keeps it editable:

| Provider | Enter | Derived token endpoint |
| --- | --- | --- |
| OAuth 2.0 | Token URL, client ID, scope | supplied token URL |
| Keycloak | Server URL, realm, client ID, scope | `/realms/{realm}/protocol/openid-connect/token` |
| Auth0 | Domain, client ID, audience | `/oauth/token` |
| Microsoft Entra | Tenant ID, client ID, scope | Microsoft v2.0 token endpoint |

If a client secret is entered, ApiWright writes it to `.env.local` as `OAUTH_CLIENT_SECRET`, `KEYCLOAK_CLIENT_SECRET`, `AUTH0_CLIENT_SECRET`, or `ENTRA_CLIENT_SECRET`, restricts the file to owner access on Unix, and stores only `${secret.NAME}` in the request. Leave the field empty to reuse an existing secret value.

## Cache and predictive refresh

The cache key includes project, auth request, effective environment, auth settings, and run mode. A token is reused only when its remaining lifetime is greater than:

```text
refresh reserve + longest observed duration of this protected request
```

After each run, ApiWright records the maximum observed duration for that request. If the next call might finish too close to expiry, ApiWright executes the auth request first and then continues with a fresh token. Refreshes are serialized within the session to avoid duplicate concurrent fetches. A successful refresh appears as an auth-refresh diagnostic.

GUI sessions share the cache for their lifetime; each CLI command owns its own session. Changing environment or auth settings naturally uses a different cache entry.

## Failure behavior

The protected request does not run when the auth request fails, returns a non-2xx status, has no response, or yields no non-empty string at `tokenPath`. These errors appear in **Diagnostics**. Use mock mode only when both the auth provider request and protected request have appropriate mocks.

Never commit `.env.local`, `.forge-local/`, environment secret sidecars, raw access tokens, or client secrets.
