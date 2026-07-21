# Getting started

This guide takes a clean installation to a runnable, file-backed API project. Forge does not require an account, cloud workspace, or per-file storage configuration.

## Install and launch

Download the package for your platform from the GitHub Releases page (`.AppImage`, `.exe`, or `.dmg`), or run the GUI from source:

```sh
cargo run --release -p forge-gui --bin forge-ide
```

Unsigned packages may trigger the operating system's unknown-publisher warning. Release checksums are published with the artifacts.

## Create a project

1. Start Forge and select **New project** on the welcome screen, or use **File → New Project…**.
2. Choose an empty directory. Its directory name becomes the project name.
3. Forge creates the conventional structure and immediately opens a new request.
4. Edit the request, select an environment if needed, and choose **Run**.

Creation supplies `forge.json`, `project.json`, `requests/`, `sequences/`, `environments/`, `specs/`, and typed directories below `assets/`. It also updates `.gitignore` for local state. Assertions and hooks are saved as sibling sidecars, so there are no separate save-path choices.

Use **Open project…** for an existing Forge workspace. **Open Standalone API Project…** accepts a directory containing `project.json`. Recent valid projects are listed on the welcome screen.

## First request

The main editor works on `*.request.json`:

1. Select a folder in **Project**, then use **+ Request → Request**. The selected story folder determines the destination.
2. Set `request.method` and `request.url`; variables use expressions such as `${env.baseUrl}` or `${bindings.petId}`.
3. Choose **Format** to pretty-print and **Validate** to run document, reference, and OpenAPI checks.
4. Choose **Run** (`Ctrl/Cmd+Enter`). The environment selector defaults to inherited project/folder/request properties.
5. Inspect **Response**, then add reusable checks from **Assertions** or the Catalog.
6. Save (`Ctrl/Cmd+S`). Request JSON, assertion sidecar, and hook sidecar are written together.

Inline JSON validation is debounced briefly while typing. Parse errors show a line/column message and an editor diagnostic. When a spec is active, the line beneath the editor reports the matching operation, missing inputs, or suggestions; `Tab` accepts the first suggestion.

## Learn with the demo workspace

Open `examples/demo-workspace`, then start **View → User tour…**. The ten-step overlay points at the live UI and covers Project, Catalog, editor, OpenAPI tools, result tabs, bottom tools, menus, Git, worktrees, and timing. Use **Left/Right Arrow** to navigate and **Esc** to close.

A useful demo path is:

1. Open `requests/pets/list.request.json`; run its deterministic mock and inspect formatted output and assertions.
2. Open `create.request.json`; compare the request with its **Assertions** and **Hooks** sidecars.
3. Run `by-id-matrix.request.json` to see multiple cases in one result pane.
4. Inspect `requests/pets/` Properties to see inherited OpenAPI and Jira metadata.
5. Open the OpenAPI tool window, filter operations, and generate a suite below the selected folder.
6. Open `requests/auth/protected.request.json` and inspect the project auth setup.

Every modern demo request has an offline mock. Enable **Use mock response** from the editor's overflow menu; enable **Allow project code** only for the reviewed demo JavaScript assets.

## Everyday navigation

- Hover compact controls for a delayed explanation.
- Bare **Shift**, twice within 400 ms, opens Search Everywhere for collection requests, actions, and environments.
- `Ctrl/Cmd+Shift+A` searches actions only.
- `Ctrl/Cmd+O` opens a project; `Ctrl/Cmd+W` closes the current legacy editor tab.
- `Ctrl/Cmd+mouse wheel` over the request editor or result pane changes code/result text size from 9–24 px.
- Drag the horizontal splitter to resize request and result areas. Drag a side tool window edge to resize it; narrow tool windows collapse to icon strips or overflow menus.

See [GUI reference](gui-reference.md) for every work area and [Settings](settings.md) for persistence and configuration.
