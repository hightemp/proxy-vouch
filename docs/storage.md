# Local storage and portable backups

ProxyVouch saves the current workspace by default. This includes the list in import order, original text and table mappings, credentials, invalid rows, last results and their check profiles, full check settings and appearance. Pending imports, selected rows, filters and unfinished settings drafts are not stored. A restart never starts network checks: Queued and Checking records become Cancelled.

## Location and file access

The folder is resolved with [Tauri's app data directory](https://v2.tauri.app/reference/javascript/api/namespacepath/#appdatadir) and the stable application identifier `dev.hightemp.proxypulse`, retained so the ProxyVouch rename preserves existing workspaces:

| Platform | Default folder |
| --- | --- |
| Linux | `~/.local/share/dev.hightemp.proxypulse/`, respecting `XDG_DATA_HOME` |
| macOS | `~/Library/Application Support/dev.hightemp.proxypulse/` |
| Windows | `%APPDATA%/dev.hightemp.proxypulse/` |

The actual path is selectable text in **Backup & restore**. `workspace.json` stores the current workspace; `workspace.previous.json` keeps the preceding valid save. `workspace.lock` is held while the process runs to prevent another instance from overwriting the same files. On Unix the app directory uses mode 0700 and new files use mode 0600. Windows files inherit the user profile's access rules.

Files contain passwords, original proxy lines and custom URLs without encryption. Store exported copies in a private location. Masked row views and report defaults continue to exclude passwords and URL query strings. No workspace or backup is uploaded.

## Saving and recovery

A Rust worker checks for changes about once per second. It captures one consistent snapshot and writes through a temporary file in the same directory, then atomically replaces the destination. Concurrent background and explicit saves are serialized. Settings changes and backup imports report success only after writing; rejected settings or a failed restore leave the existing in-memory state intact.

The main screen shows **Saving…**, **Saved locally** or **Not saved**. A persistent error explains a failure. On a normal close the app flushes changes before exiting. During a check, **Stop, save and quit** waits for cancellation before the final save. A failed final save leaves the window open and offers retry or an explicit exit without new changes. Abrupt process termination can lose changes since the last autosave, normally about one second plus any pending write.

If the main file is damaged and the previous file is readable, the app restores the previous generation and preserves the damaged file as `workspace.damaged-<timestamp>.json`. It also recovers a missing main file from the previous generation. A notice explains recovery. Unsupported schema versions and unrecoverable files block automatic overwriting. To recover manually, close the app, copy both JSON files to a safe location, move them out of the data folder, restart and import a known-good full backup. An unavailable folder or a second instance also shows an error instead of claiming that data was saved.

**Clear list** persists an empty current list. The previous generation or exported copies may still contain deleted records; it is not a secure erase. Existing `preferences.json` files in the old app config directory are read once when no workspace exists, migrating theme, concurrency and rate limit while preserving the old file.

## Export and import

Use **Backup & restore** for portable JSON files:

- **Full workspace** includes proxies, their results and all settings.
- **Proxies and results only** excludes the current application settings. Historical results still carry the profiles used to check them.
- **Settings only** excludes all proxy rows and their credentials; custom URLs and response text remain part of settings.

Backups are identified by the compatibility format `format: "proxy-pulse-backup"` and `version: 1`. ProxyVouch continues to read existing backups and writes the same format under its new default filename. Optional `entries` and `preferences` sections distinguish the scopes. Records preserve the requested protocol independently from the detected protocol, so Auto and SOCKS DNS modes round-trip exactly. The format has a 256 MiB limit and a maximum of 100,000 records; files with unknown versions or invalid fields are rejected before changes are applied.

Choosing a file shows its name, record/result/invalid counts, and whether it contains settings or credentials. The backend keeps the actual pending archive; the preview does not send passwords to the frontend. Choose **Merge**, **Replace** or **Do not import proxies**, and independently choose whether to import settings. Merge compares the complete endpoint, credentials and requested protocol, skips exact duplicates, and retains existing results; identical invalid raw lines are also skipped. Replace preserves archive order and duplicates. An empty list can explicitly replace a nonempty one. Import is disabled while checks are running.

TXT/CSV/TSV proxy lists continue to use **Add proxies** and its parser preview. CSV/JSON reports under **More** describe check outcomes; they are not restore files. Portable archives keep full reusable state and therefore have a separate format and UI.

## Verification

`make test-storage` builds a standalone debug app, starts its own Linux WebDriver/Xvfb environment and tests real restarts, file dialogs and write-failure recovery. It requires tauri-driver, WebKitWebDriver, Xvfb, xclip and xdotool; the local tool locations under `artifacts/` are supported. All application data uses temporary XDG directories. Native checks on other operating systems remain separate release work.
