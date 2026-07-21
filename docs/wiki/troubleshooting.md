# Troubleshooting

## Request does not validate

Open **Diagnostics** and use the red underline in the editor. Run **Format** first if line/column locations look confusing. Confirm `formatVersion`, `kind`, required `meta` fields and the request URL. Validate headlessly with:

```sh
forge validate path/to/test.request.json --root /path/to/project
```

## Variables or assets do not resolve

Check the namespace (`env`, `secret`, `bindings`, `runtime`, `data`) and the effective environment inherited from Properties. Run `forge assets .` for broken references and `forge lock . --check` when frozen execution reports drift.

## OpenAPI operation does not match

Confirm the effective spec on the request/folder, HTTP method, path template and configured base URL. Query strings do not define the operation path. OpenAPI 3.x is supported; Swagger 2 documents must be converted first.

## Authentication refresh fails

Run the auth request independently, verify its token JSONPath, lifetime and refresh-before window, then inspect the dependent request's **Auth** and **Runtime** tabs. Ensure the client secret exists in `.env.local` under the configured name.

## CI differs from the GUI

Use the same project root, environment, mock mode, asset lock mode and project-code permission. The GUI and CLI share execution logic, but local secrets and selected inherited properties can differ. `forge ci` exit code `2` means configuration/execution failure; `1` means the request ran but assertions failed.

## Advisor has poor context

Save the active request, ensure it lives inside a Forge project, and attach the correct OpenAPI source. The Advisor automatically includes active sidecars, project metadata and nearby JSON/YAML/JS files. Enable **Include last response** only when runtime evidence is useful. Context is intentionally bounded and secrets are redacted.

## Generated folder is not replaced

Forge only regenerates folders carrying its generator manifest. Rename or remove a hand-written conflicting directory yourself; Forge will not assume ownership of it.
