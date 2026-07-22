# Settings

Open **File → Settings…**, click the activity-bar gear, or press `Ctrl/Cmd+Alt+S`. The dialog is resizable; categories stay on the left and content scrolls independently.

## Save semantics

Appearance, editor, and view options apply immediately. HTTP options edit a draft:

- **Save** persists HTTP settings and closes.
- **Apply** persists HTTP settings and keeps the dialog open.
- **Cancel** closes without applying the current HTTP draft; live appearance/view changes are not rolled back.

HTTP settings are stored in the project's `forge.json` and are reviewable project configuration. UI preferences are captured per workspace in `.forge-local/ui-state.json` on project switch and application exit; `.forge-local/` is ignored by Git.

## Appearance

- **Theme:** Dark or Light.
- **UI font:** IBM Plex Sans or JetBrains Mono.
- **UI font size:** 11–20 px.

These settings update the interface immediately. Code editors keep a separate monospaced size.

## Editor

- **Code font size:** 9–24 px. The same value can be changed with `Ctrl/Cmd+mouse wheel` over the request or result area.
- **Save dirty files when switching or closing:** when enabled, ApiWright saves the active request before opening/creating another or closing it. If saving fails, the current request remains open.

A v1 Save writes the request plus assertion and hook sidecars. Autosave is enabled by default.

## View

Toggle Activity bar, Project, Collections, legacy Environment, bottom tool bar, and status bar. **Zen mode** provides a distraction-free editor while retaining edge-hover access to tools. The same controls are available under **View** in the menu bar.

## HTTP

Defaults are a 30-second timeout, redirect following with a maximum of 10, and TLS certificate verification enabled.

- **Timeout:** 1–600,000 ms.
- **Redirects:** enable/disable following and cap the redirect count at 0–50.
- **TLS:** certificate verification. Disable only for controlled local diagnostics.
- **User-Agent:** blank uses the engine default.
- **Client certificate:** workspace-relative or absolute PEM certificate, optional separate private key, and optional CA bundle. The key may be embedded in the certificate PEM.
- **OpenAPI:** project fallback URL or project-relative spec path. A closer request/folder property overrides it.
- **Proxy:** proxy URL and comma-separated no-proxy host suffixes.

Certificate paths and proxy configuration belong in project settings only when they are portable and non-secret. Never store private-key contents, proxy credentials, or tokens in committed JSON.

## Keymap

The Keymap page is the read-only source of current shortcuts. Shortcuts follow the platform command modifier (`Ctrl` on most Linux/Windows setups and `Cmd` on macOS). They are not currently user-remappable.

## Restored workspace state

ApiWright restores valid open legacy tabs, active environment, theme, tool visibility, autosave, typography, and the selected bottom tool. Missing files are skipped; if the previously active file disappeared, ApiWright selects the last restored tab. Modern request state remains file-backed and is reopened from Project.
