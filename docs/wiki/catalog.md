# Catalog

The catalog is the single source of reusable API-test behavior. Built-ins are static metadata: title, description, intent, phase, typed parameters and example. Search by intent—**Validate**, **Prepare**, **Capture**, **Generate** or **Simulate**—then configure through the form.

Common built-ins include status, response time, body text/regex, JSONPath type/value/length, cookies, headers, JSON Schema, OpenAPI response validation, extractors, bearer/basic auth and generated UUID/timestamps.

Project assets live below `assets/` and use a metadata sidecar:

```text
assets/assertions/customer-shape.js
assets/assertions/customer-shape.meta.json
```

Requests reference the asset; they do not copy its implementation. This is the intended single point of concern for repeated assertions and scripts.
