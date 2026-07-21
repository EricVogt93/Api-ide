# Authentication

Authentication can be attached directly to a request or supplied by a reusable auth request. Providers support bearer/basic flows and presets for common OAuth/OIDC services such as Keycloak, Auth0 and Azure.

Configure the token path, lifetime and refresh window. Forge refreshes before a dependent request would outlive the remaining token lifetime; a request can therefore be interrupted and refreshed predictively instead of failing with an expired token.

Keep client secrets in `.env.local` or the environment. Local auth configuration is stored below `.forge-local/` and is ignored by Git.
