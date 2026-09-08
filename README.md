<p align="center">
  <img src="src-tauri/icons/icon.png" alt="ProxyVouch icon" width="112" height="112">
</p>

<h1 align="center">ProxyVouch</h1>

<p align="center">
  <a href="https://github.com/hightemp/proxy-vouch/actions/workflows/quality.yml"><img src="https://github.com/hightemp/proxy-vouch/actions/workflows/quality.yml/badge.svg" alt="Quality checks"></a>
  <a href="https://github.com/hightemp/proxy-vouch/actions/workflows/release.yml"><img src="https://github.com/hightemp/proxy-vouch/actions/workflows/release.yml/badge.svg" alt="Release build"></a>
  <a href="https://github.com/hightemp/proxy-vouch/releases/latest"><img src="https://img.shields.io/github/v/release/hightemp/proxy-vouch" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT license"></a>
  <a href="https://v2.tauri.app/"><img src="https://img.shields.io/badge/Tauri-2-24C8D8?logo=tauri&amp;logoColor=white" alt="Built with Tauri 2"></a>
  <img src="https://asdertasd.site/counter/proxy-vouch" alt="ProxyVouch counter">
</p>

A local desktop proxy checker built with Tauri, Rust and React. Import a mixed list, verify real requests through each proxy, and copy or save the results.

![ProxyVouch main window with the proxy list and check results](screenshots/2026-09-06_12-15.png)

## Run the application

Requirements: Rust 1.90.0, Node 22.16.0+, pnpm 10.32.1 and the [Tauri system dependencies](https://v2.tauri.app/start/prerequisites/). On Ubuntu 24.04, install development packages for WebKitGTK 4.1, GTK 3, libcurl/OpenSSL, libxdo and librsvg.

```sh
pnpm install --frozen-lockfile
make doctor
make dev
```

`make dev` opens the native desktop app. `make preview` runs only the frontend in a browser; it clearly disables desktop operations and does not fabricate check results.

The desktop process automatically adds loopback and Tauri's local hosts to both `NO_PROXY` and `no_proxy` before WebKit starts. Existing exclusions and HTTP/HTTPS proxy settings are preserved. This keeps the development UI at `127.0.0.1:1420` local when your shell has `HTTP_PROXY` configured; no shell or system configuration changes are needed. The checker still uses each explicitly selected proxy.

```sh
make build-debug  # Standalone debug executable: target/debug/proxy-vouch
make build        # Release executable: target/release/proxy-vouch
make package      # Linux .deb + AppImage: target/release/bundle/
make appimage     # Linux AppImage only
make help         # All commands
```

Without Make, use `pnpm desktop`, `pnpm tauri build --no-bundle`, `pnpm tauri build --debug --no-bundle` and `cargo test --workspace --locked`. Native Windows/macOS packaging must be checked on those systems before distributing a supported package.

Application versions come from [VERSION](VERSION). Run `make version` after editing it; normal development/build commands also synchronize the package metadata automatically. `make release` creates and pushes the corresponding version tag from a clean, already-pushed commit, triggering GitHub Actions to build all platform packages and publish a release with linked commits and checksums. See [the release guide](docs/releases.md) for setup, `make release-dry-run`, AppImage builds and retry behavior.

## Import

Use **Add proxies**, **Import file**, clipboard paste, or drop a UTF-8 TXT/CSV/TSV file onto the app. The preview shows valid records, errors and duplicates. Edit rejected records or keep them as Invalid rows for later correction. Appending is the default; replacing the list and keeping duplicates are explicit options.

Choose **Supported formats** inside **Add proxies** for examples of every supported format family, CSV/TSV mappings, IPv6 and credential escaping. **Back to import** restores your input and preview. The same reference is available from **Formats & help** in the sidebar.

```text
192.0.2.10:8080
proxy.example:1080:demo-user:demo-pass socks5
demo-user:demo-pass@proxy.example:1080
https://demo-user:demo-pass@proxy.example:8443
socks5h://demo%40user:p%3Ass@[2001:db8::10]:1080
socks4a://userid@proxy.example:1080
```

For CSV/TSV, use columns such as `host,port,username,password,protocol`, supported header aliases, or an explicit column mapping. Select the reverse-order template for `username:password:host:port`; it is never guessed. Select Text explicitly if a delimiter inside a compact record could be confused with CSV. Use an encoded URI or mapped fields for special characters in credentials.

Limits: 100,000 records, 20 MiB per input and 8 KiB per record. IPv6 endpoints require brackets and ports are mandatory. See the in-app **Formats & help** dialog and [PRD.md](PRD.md) for the full input contract.

## Check

Supported routes: HTTP, HTTPS (TLS to the proxy), SOCKS4, SOCKS4a, SOCKS5 with local DNS (`socks5`) and SOCKS5 with remote DNS (`socks5h`). Without a protocol, Auto tries HTTPS, SOCKS5 remote DNS, HTTP, SOCKS4a and SOCKS4. Candidates incompatible with supplied credentials are skipped. Explicit protocols are respected; **Detect again** changes a row to Auto.

The default check requests an IP echo endpoint over HTTPS. Settings also support custom HTTP/HTTPS URLs, response validation, a fallback URL, concurrency, pacing, deadlines and limited retries. Both certificate layers are verified. System and environment proxy settings do not select an alternative route for checks.

| Status | Meaning |
| --- | --- |
| Working | A real request passed the selected profile |
| Failed | A concrete connection, proxy, certificate or authentication error |
| Inconclusive | The endpoint failed, the protocol was not confirmed, or the client could not evaluate the proxy |
| Invalid | The record needs correction before a check |
| Cancelled | The check was stopped before completion |

Open a row for protocol attempts, the sanitized check URL and error details. Latency measures the successful attempt, including setup and response validation. It is not ICMP ping or bandwidth. Authentication validity is reported separately; a successful request does not automatically prove that a supplied password was required.

Select row checkboxes to reveal **Check selected** and **Remove selected (N)** above the table. Removal asks for confirmation and applies to the entire selection, including rows hidden by the current filter. Other rows stay in the list; the change is saved automatically. Removal is disabled while checks are running.

## Export and session data

**Copy working / Save working** use proxy URLs. **Copy failed / Save failed** preserve original records. **More** offers Checked, Inconclusive, Selected, Filtered and All scopes, with URLs, original lines, compact text, CSV reports and versioned JSON reports.

Checked includes Working, Failed and Inconclusive. It excludes Invalid and Cancelled. Working and Failed actions use the complete list, independently of the table filter.

Reusable lists include credentials by default; reports omit them by default. Unknown Auto protocols and incompatible source schemas require an appropriate format instead of silently dropping rows. CSV reports escape spreadsheet formula prefixes; URLs and JSON preserve exact credential data.

Your proxy list, credentials, last results, full check settings and appearance are saved automatically and restored at startup. **Saved locally / Saving… / Not saved** shows the current state. Closing an idle window flushes the data; closing during a check offers **Stop, save and quit**. Interrupted checks restore as Cancelled.

Open **Backup & restore** to export a full workspace, proxies with results, or settings alone. Import shows the file contents before applying them: merge skips exact duplicates and keeps existing results, while replace restores the list including its duplicates. Settings can be imported independently. Portable backup JSON files are distinct from the existing CSV/JSON reports; ordinary proxy lists still use **Add proxies**.

The data folder is shown in **Backup & restore**. On Linux it defaults to `~/.local/share/dev.hightemp.proxypulse/` (or `$XDG_DATA_HOME/dev.hightemp.proxypulse/`), on macOS to `~/Library/Application Support/dev.hightemp.proxypulse/`, and on Windows to `%APPDATA%/dev.hightemp.proxypulse/`. ProxyVouch retains this stable legacy identifier path so upgrades do not strand an existing workspace. Local files and backups contain passwords and custom URLs without encryption; Unix data directories and files use owner-only permissions. There is no telemetry, cloud account or system proxy switcher. See [storage and backups](docs/storage.md) for format, recovery and limits.

## Verify

```sh
make quality           # Format, lint, types, core contracts, local protocols, browser UI
make test-integration  # Real loopback proxy matrix; no public proxies
make test-ui           # Browser layout/help tests (not native protocol evidence)
make test-storage      # Linux: real app restart, autosave, backup/restore file dialogs
```

Python 3 and OpenSSL are required for protocol fixtures. Browser checks use local Chrome when available, or an installed Playwright Chromium (`pnpm exec playwright install chromium`). `CHROME_PATH` can select a test browser. The UI test wrapper removes proxy environment variables only for its child process, so loopback readiness checks are not sent through an external proxy.

For native Linux GUI acceptance, install a matching WebKitWebDriver and tauri-driver into a test tools directory, then run:

```sh
env -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY -u http_proxy -u https_proxy -u all_proxy \
  xvfb-run -a tauri-driver --native-driver /path/to/WebKitWebDriver --port 4457 --native-port 4458
# In another terminal, after make build-debug:
make test-native
```

The native script exercises actual Rust IPC, local proxies, the rendered WebView, clipboard groups, reports, search and cancellation. It launches the app with temporary XDG data/config folders so tests cannot change your saved workspace. Results are written under ignored `artifacts/`. [Initial Linux evidence](docs/verification/initial-linux.md) distinguishes completed checks from remaining work.

With xdotool available (on PATH or under `artifacts/xdotool/extracted/usr/bin/`) and xclip on PATH, the Linux native test also drives the save/open choosers. It pastes paths to avoid GTK completion changing simulated keystrokes, discovers the matching driver's isolated display and reports this optional coverage as `native_file_dialogs_verified`.

`make test-startup` builds the development executable and tests its real WebView with uppercase/lowercase proxy variables and pre-existing exclusions. It manages its own Vite/WebDriver processes, uses an isolated proxy trap and verifies both local UI loading and explicit checker routing. It requires Xvfb, tauri-driver and WebKitWebDriver, found on PATH or in the test tool locations under `artifacts/`. See [the startup regression report](docs/verification/proxy-environment.md).

## Structure

```text
crates/core/         Parser, model, checker, scheduler/session and export
src-tauri/           Desktop commands, native dialogs, clipboard and capabilities
src/                 English React interface and virtualized result table
scripts/             Offline fixtures, native smoke and development utilities
tests-ui/            Browser UI smoke checks
docs/adr/            Platform, network and result-semantics decisions
docs/verification/   Recorded acceptance evidence and limits
```

Remaining work includes full 100,000-row GUI/RSS measurements, richer profile snapshots and independent fallback validators, remaining context-menu workflows, further native dialog edge cases, and Windows/macOS packaging and acceptance. Consult TASKS.md for precise completion boundaries.
