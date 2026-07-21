# Request and sidecars

Request files use the versioned `request-v1` schema. The request document contains HTTP data only:

```json
{"formatVersion":1,"kind":"request","meta":{"id":"users.list","name":"List users"},"request":{"method":"GET","url":"${env.baseUrl}/users"}}
```

Assertions and hooks are separate files beside the request:

```text
users.list.request.json
users.list.assertions.json
users.list.hooks.json
```

This keeps the request readable and lets reusable behavior be reviewed independently. Bindings use semantic names local to each test; catalog parameters can source values from environment variables, previous responses, data rows or generated values.

Use **Format** for canonical JSON, **Validate** for schema/OpenAPI diagnostics, and the minimap/editor diagnostics to locate exact JSON errors. The Response, Assertions, Hooks, Auth, Runtime, Trace and Diagnostics tabs keep execution concerns separated.
