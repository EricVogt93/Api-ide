# Secrets and security

## Licensing boundary

ApiWright is source-available under PolyForm Noncommercial 1.0.0. Personal and
other noncommercial use is permitted. Internal business use, commercial
services, paid products and customer work require a separate paid commercial
license; see the repository's `COMMERCIAL-LICENSE.md`.

## Local-only data

ApiWright keeps machine-specific state out of normal project files:

- `.env.local` stores request-v1 secret names and values.
- `*.secrets.json` accompanies legacy environment files.
- `.forge-local/` contains Advisor configuration, cookies, history and UI state.
- `.forge/` contains generated runtime/index data such as the asset lock.

Workspace creation and local configuration update `.gitignore`. Verify the ignore rules before the first commit in an imported project.

## Variable use and masking

Reference secrets explicitly as `${secret.NAME}`. Secret values are loaded lazily, recorded during interpolation and masked from public results, diagnostics, hook logs and preview output. The Advisor redacts sensitive JSON keys and headers such as Authorization before transmitting context. Names may be suggested in forms; secret values are never used as completion text.

## Project code

JavaScript catalog assets are repository-owned executable code. They are disabled unless **Allow project code** or `--allow-project-code` is selected. Review code before enabling it. The embedded runtime has execution limits and exposes no general filesystem, network or process API, but it is not advertised as an adversarial sandbox.

## Transport and exports

TLS verification is enabled by default. Disabling it should be limited to controlled development environments. Export bundles omit secret-provider files and runtime state. Generated cURL/language snippets may contain already-resolved literal headers if copied after manual editing, so inspect them before sharing.
