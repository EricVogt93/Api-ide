# GUI reference

ApiWright uses a resizable IDE shell. Compact icon controls expose their purpose after a two-second hover; disabled controls explain missing prerequisites where relevant.

## Application shell

From left to right and top to bottom:

- **Menu bar:** File, Run, View, Help, active environment, and Zen mode.
- **Activity bar:** opens Project, collection/history/run tools, legacy Environment, and Settings.
- **Project tool window:** file tree or Git view. Drag its right edge; collapsing it leaves the activity icon available.
- **Catalog:** reusable built-ins and project assets, filtered by source, intent, and text.
- **Request editor:** toolbar, JSON editor/minimap, OpenAPI assistance, and result splitter.
- **Right tool window:** icon tabs for OpenAPI, contract generation, API generation, k6 generation, and AI Advisor. Drag its left edge or collapse to an icon strip.
- **Bottom tools:** Run, Problems, Terminal, History; **More** contains Log, Console, Cookies, and Variables. Clicking the active tab collapses it.
- **Status bar:** branch/worktree actions, environment/readiness, execution time, and ApiWright version.

At narrow widths, the request toolbar moves Format/Validate into its overflow menu and lower result tabs move Runtime/Trace/Diagnostics into **More**.

## Project explorer

The **Files** tab presents ordinary filesystem folders. `assets` and `environments` use distinct icons; request, sequence, and general files are visually separated. Click a directory to make it the destination for new content. Jira icons appear only when enough horizontal space exists; inherited links are muted and own links use the accent color.

Folder/project context actions include **Run project/folder**, **New request**, **New folder**, **Add files**, recursive JSON beautification, ApiWright bundle/cURL export, ApiWright bundle import, Properties, file-manager reveal, Jira link actions, and Git actions. Request menus add Open, export, Properties, and Jira; asset menus can copy a stable reference or run affected requests.

The **Git** tab groups conflicts, untracked files, unstaged changes, partially staged files, and staged changes. Context Git actions stage, revert with confirmation, or open a commit dialog. The status-bar branch menu switches branches and opens **New worktree…**.

Properties show the effective path and request count, then configure inherited environment and OpenAPI source. Request Properties additionally toggles **Regression test** for `apiwright ci --regression`.

## Catalog workflow

Choose **Built-ins** or **Project assets**, then filter by Validate, Prepare, Capture, Generate, or Simulate. Select an entry to open its typed form. Parameter sources can be literal values or references to binding, environment, runtime, matrix, and—where valid—secret namespaces.

The preview does not send HTTP. Before-request assets show request differences; after-response assets use the last response and show assertion results, logs, and runtime writes. Project JavaScript remains disabled until **Allow project code** is explicitly enabled.

Assertions and hooks inserted from the Catalog are written to sidecars. In the lower **Assertions**/**Hooks** tab, **Edit in catalog** reloads the existing parameters into the same form.

## Request editor

The toolbar runs, saves, formats, validates, overrides the inherited environment, and exposes mock/project-code/sequence actions. A dirty marker appears beside the request path. The code editor provides JSON highlighting, numbered lines, a diagnostic underline/message, minimap, horizontal/vertical scrolling, and OpenAPI assistance.

Drag the splitter between request and results. `Ctrl/Cmd+mouse wheel` over either area scales editor/result typography without scaling the Project or right tool windows.

The lower tabs are deliberately separate:

- **Response:** run status, HTTP status/time/bytes, OpenAPI response warnings, formatted JSON/XML/HTML or Raw, and Copy.
- **Assertions:** configured checks, enable/remove/edit actions, and pass/fail details from the last run.
- **Hooks:** configured before/after behavior with enable/remove/edit actions.
- **Auth:** reusable auth request/provider setup and refresh policy.
- **Runtime:** run environment, duration, extracted values, and transport data.
- **Trace:** reserved for lifecycle, hook, redirect, and network timing traces.
- **Diagnostics:** JSON, reference, OpenAPI, and execution errors.

Matrix and batch runs add a run selector above the tabs and update the active response when a case is selected.

## Menus and shortcuts

**File** creates/opens projects, saves, imports cURL/OpenAPI/Postman/Bruno, opens Settings, and quits. **Run** sends the current request, runs a legacy collection, or opens gRPC. **View** controls tool windows, theme, Zen mode, environment management, and the User tour. **Help** checks for updates and opens About.

| Action | Shortcut |
| --- | --- |
| Save / Save all | `Ctrl/Cmd+S` / `Ctrl/Cmd+Shift+S` |
| Send request | `Ctrl/Cmd+Enter` |
| Close / next / previous tab | `Ctrl/Cmd+W`, `Ctrl/Cmd+Tab`, `Ctrl/Cmd+Shift+Tab` |
| Open project | `Ctrl/Cmd+O` |
| Toggle Collections | `Ctrl/Cmd+1` |
| Zen mode | `Ctrl/Cmd+Shift+F11` |
| Settings | `Ctrl/Cmd+Alt+S` |
| Import cURL | `Ctrl/Cmd+Shift+V` |
| Search actions | `Ctrl/Cmd+Shift+A` |
| Search Everywhere | bare `Shift` twice |

Zen mode hides application chrome. Hover the left, right, or bottom screen edge to reveal its tools temporarily; hover the upper-right corner for **Exit Zen**.

## User tour

Start **View → User tour…** at any time. It turns off Zen mode, reveals the relevant tool window for each step, outlines the live target, and never creates or mutates project data. Navigate with buttons or Left/Right Arrow; close with Esc.
