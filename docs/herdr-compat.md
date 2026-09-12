# Herdr 0.9.0 compatibility note (G1)

Qualified against the installed binary (`herdr 0.9.0`, client + server, protocol 22) and the
v0.9.0 plugins doc. This is the herdr-compat-FIRST gate evidence: no schema/sync work was done
before this was proven.

## What herdr 0.9.0 supports (verified)

- **Plugin = a directory with `herdr-plugin.toml` + argv commands.** Any language (Rust binary
  included); no SDK, no restricted command set. The full `herdr` CLI is the plugin API.
- **Manifest contract** — required `id`, `name`, `version`, `min_herdr_version`; optional
  `description`, `platforms`, `[[build]]`, `[[startup]]`, `[[actions]]`, `[[events]]`,
  `[[panes]]`, `[[link_handlers]]`.
- **Native tab panes** — `[[panes]]` with `placement = "tab"` (also `overlay` [default], `popup`,
  `split`, `zoomed`). Opened via `herdr plugin pane open --plugin <id> --entrypoint <id>
  --placement tab`. A `tab` pane is a normal Herdr pane (has `HERDR_PANE_ID`, supports pane APIs).
- **Runtime context injection** — `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_ENV=1`,
  `HERDR_PLUGIN_ID`, `HERDR_PLUGIN_ROOT`, `HERDR_PLUGIN_CONFIG_DIR`, `HERDR_PLUGIN_STATE_DIR`,
  `HERDR_PLUGIN_CONTEXT_JSON`, plus `HERDR_WORKSPACE_ID` / `HERDR_TAB_ID` / `HERDR_PANE_ID` when
  available, and `HERDR_PLUGIN_ENTRYPOINT_ID` for pane commands.
- **Local dev** — `herdr plugin link <path>` registers a working directory (no build commands);
  **install** — `herdr plugin install owner/repo[/subdir]` clones + runs `[[build]]` + registers.
- **Secrets/state hygiene** — config under `HERDR_PLUGIN_CONFIG_DIR`, state under
  `HERDR_PLUGIN_STATE_DIR`; never the (managed) plugin root.

## What herdr 0.9.0 does NOT support (design constraints)

- **No native non-terminal plugin UI.** A plugin pane is a terminal pane; the board must be a
  TUI (ratatui) running in that pane. Confirms the "Rust, our own TUI" plan.
- **No Herdr-managed storage API (v1).** The board owns its store (SQLite), per REQUIREMENTS.
- **No runtime action registration.** Actions/panes/hooks/link handlers are manifest-declared only.
- **No plugin update verb (v1);** reinstall refreshes a GitHub-managed plugin.
- **`plugin link` does not run build commands** — the pane binary must already be built.

## Pinning

- `min_herdr_version = "0.9.0"` in `herdr-plugin.toml` (Herdr refuses to link/install if newer
  than the running binary).
- Installed binary: `herdr 0.9.0` (client + server, `protocol 22`).

## Implications for the board build (post-G1)

- Board UI = ratatui TUI launched by the `board` pane entrypoint.
- `Process with factory` = a manifest `[[actions]]` entrypoint (or link handler), invoked via
  `herdr plugin action invoke`, handed to coordinator/planner — never started by sync.
- Durable state (cards, requests, receipts) in SQLite under `HERDR_PLUGIN_STATE_DIR`; credentials
  only via env/config, never in card/prompt/SQLite content.
