# AI Advisor

The Advisor is an optional OpenAI-compatible chat-completions integration. Configure endpoint, model and an API-key variable in the Advisor sidebar; the key is resolved from `.env.local` or the process environment.

Every request sends a bounded context assembled from the active file:

- active request JSON and relative path;
- assertion, hook and auth sidecars;
- matching OpenAPI operation and source document;
- `project.json`/`forge.json` metadata;
- relevant JSON/YAML/JS files in the active folder;
- optionally the last response, including status, timing and headers.

Sensitive fields and headers are redacted before transport. The active file is authoritative; surrounding files are supporting context. Do not enable project code merely to use the Advisor.
