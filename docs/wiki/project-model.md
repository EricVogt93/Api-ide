# Project model

## Conventional layout

```text
project/
├── project.json
├── requests/          # request files and sidecars
├── assets/            # reusable assertions, hooks, extractors, generators, mocks
├── environments/      # environment JSON files
├── specs/             # OpenAPI documents
└── .forge-local/      # ignored advisor config and local state
```

Folders can define an environment, OpenAPI source and Jira link. Descendants inherit these values; a request or child folder can override them. Jira links are visible in the tree and open from the context menu.

The project tree also exposes Git state, branch switching, worktrees and context actions such as run, beautify, export and recursive operations. Generated suites are written below the selected folder and carry a manifest so hand-written files are not overwritten accidentally.
