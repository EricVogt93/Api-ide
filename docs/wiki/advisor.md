# AI Advisor

The Advisor is an optional OpenAI-compatible chat-completions client in the request editor's right tool window. It reviews the active request with bounded project context; it is not an autonomous editor and never changes files.

## Configure a provider

1. Open a request and expand the right tool window.
2. Select the console-shaped **AI Advisor** tab.
3. Expand **Connection** and enter an OpenAI-compatible base URL and model.
4. Optionally enter an API-key *variable name*, for example `OPENAI_API_KEY`.
5. Choose **Save connection**.

A base such as `http://localhost:11434/v1` becomes `/v1/chat/completions`; a full `/chat/completions` URL is accepted unchanged. Only HTTP(S) endpoints are allowed. Requests time out after 90 seconds.

Configuration is stored locally in `.forge-local/advisor.json`. It contains the endpoint, model, and key variable name—never the key value. The value is resolved first from `.env.local`, then from the process environment. No API key is required for a compatible local server that accepts unauthenticated requests.

## Ask with the correct context

The context row shows the authoritative active file. Enter a concrete question and choose **Ask advisor**. **Include last response** becomes available after a run; leave it off when response data is irrelevant or sensitive. Answers are selectable and can be copied.

Forge assembles, in this order and within a 48,000-character ceiling:

- active request path and parsed request JSON;
- active assertion, hook, and project-auth documents;
- `project.json` and `forge.json`, when present;
- the active OpenAPI source, capped separately;
- up to 20 sibling JSON, YAML, YML, or JavaScript files from the active folder;
- the compact matching OpenAPI operation (method, path, inputs, request schema, and responses);
- optionally the latest response status, time, headers, and body.

Large sections are truncated rather than silently expanding the prompt without limit. The active request remains authoritative; neighboring files are supporting evidence.

Good prompts name the desired outcome, for example:

- `Compare this request and its assertions with the matching OpenAPI response.`
- `Which required inputs are missing, and what exact sidecar assertions should I add?`
- `Review the auth scope and explain why this protected request may not receive a token.`

## Redaction and trust boundary

Before transport, Forge recursively masks JSON keys containing `authorization`, `apikey`, `password`, `secret`, `token`, or `cookie`. Header objects with a sensitive `name` also have their `value` masked. Sensitive response headers and JSON bodies receive the same treatment.

Redaction is a safety net, not a data-classification system. Plain text, unusual field names, source comments, and nearby JavaScript may still contain confidential data. Review the active file and provider policy before sending. Keep **Include last response** disabled unless needed.

The provider receives one system instruction, the question, and the assembled API context. Forge does not send the complete repository, execute model suggestions, or grant the model filesystem access.

## Troubleshooting

- **Ask advisor is disabled:** endpoint or model is empty, or a request is already in flight.
- **Secret … was not found:** define the configured variable in `.env.local` or the environment; do not paste the secret into the connection field.
- **HTTP 404:** verify whether the provider expects a `/v1` base or a full chat-completions URL.
- **No useful OpenAPI comparison:** assign a spec in Properties and verify that the active method/path matches an operation.
- **Invalid request JSON:** fix the editor diagnostic first; context assembly requires parseable JSON.
