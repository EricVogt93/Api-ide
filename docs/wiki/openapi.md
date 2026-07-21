# OpenAPI

Place a spec under `specs/`, configure it in project Properties, or set an inherited source on a folder. The right sidebar groups operations by method and shows path, description and coverage.

OpenAPI support provides:

- request completion and operation suggestions;
- parameter and body value generation;
- request/response compatibility diagnostics;
- endpoint coverage marking when a request matches an operation;
- contract, full API and k6 performance suite generation.

Generated output is placed below the current folder:

```text
contract/      # contract requests and assertions
api/           # complete API test requests and sequences
performance/   # k6 smoke/load/stress/spike/soak profiles
```

Existing generated folders without a Forge manifest are left untouched.
