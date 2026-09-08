# Initial Linux implementation verification

Date: 2026-09-05. This records a working implementation milestone, not full cross-platform MVP acceptance.

This is the initial checkpoint. Later builds reuse the local artifact paths; their current packaging and checksums are described in [release automation verification](release-automation.md).

## Environment

- Ubuntu userspace with glibc 2.39; Linux x86_64, kernel 7.0.0-30-generic.
- Intel Core i5-13400F, 16 logical CPUs, approximately 32 GiB RAM.
- Rust 1.90.0; Node 22.16.0; pnpm 10.32.1.
- WebKitGTK 2.52.6, GTK 3.24.41, system libcurl 8.5.0 and OpenSSL 3.0.13.
- Native GUI automation uses Tauri WebDriver 2.0.6, matching WebKitWebDriver and an isolated Xvfb display. Browser-only tests use a separate headless Chrome instance.

The host is faster and has more memory than the PRD's proposed 4-CPU/8-GiB baseline. Measurements below do not close the full performance acceptance criteria.

## Completed checks

| Check | Evidence |
| --- | --- |
| Rust and TypeScript checks | `make quality`: format checks, Clippy with warnings denied, TypeScript type checking |
| Core tests | 17 contract/property tests and 2 scheduler integration tests |
| Proxy protocols | 39 local network cases in `scripts/network_fixtures.py` |
| Browser UI | 3 Playwright tests: honest preview state, keyboard-accessible help, minimum-width layout |
| Native Tauri UI | Actual WebView + Rust IPC: import, duplicate/error preview, real checks, result table, clipboard groups, JSON report and search |
| Native cancellation | An active and queued run stopped within 0.27 seconds in the initial debug GUI test; 0.235 seconds in the release GUI test |
| Native file dialogs | Save working produced exactly four records through the system save dialog; Import file read the same file back and previewed four valid records |
| Atomic file writing | Exact replacement and failure-preserves-existing-file core tests |
| Build | Standalone debug/release Linux executables and a Debian package built locally |
| Packaged executable | The executable extracted from the actual Debian package passed the native GUI, file-dialog, clipboard, search and cancellation smoke; cancellation measured 0.357 seconds |
| Public default profile | One real request to the default ipify URL through a local HTTP proxy returned Working with a valid IP; the address itself was not recorded |

The protocol matrix includes explicit HTTP, HTTPS, SOCKS4, SOCKS4a, SOCKS5 and SOCKS5h; Auto; valid/invalid/missing HTTP credentials; valid/invalid SOCKS5 credentials; SOCKS4 USERID; unsupported authentication; SOCKS destination rejection; local/remote DNS; IPv6 proxy and destination addresses; both TLS trust failures; hostname mismatch; HTTP forward/CONNECT; target 302/403/429/503; invalid and oversized bodies; fallback; non-proxy services; and explicit wrong protocols, refused proxy connections and proxy hostname resolution failures.

All fixture credentials are synthetic. An environment-proxy trap received zero connections despite upper- and lowercase proxy variables and `NO_PROXY=*` being set for the checker process. SOCKS5 fixtures assert that GSSAPI is not offered. No certificate-verification bypass is used.

The native list initially contained four Working rows, one Failed row and one Invalid row. Copy working produced four reusable URLs; Copy failed produced exactly the incorrect-credential record; a Checked JSON report contained five records and omitted passwords. Browser tests are not substituted for native IPC evidence.

## Core performance sample

Command: `cargo run -p proxy-vouch-core --example benchmark --release --locked`.

| Records | Parse, preview and commit | Initial snapshot serialization |
| --- | --- | --- |
| 10,000 | 16 ms | 13 ms |
| 100,000 | 346 ms | 321 ms |

These are single measurements on the host above. They exclude native IPC delivery, frontend rendering, GUI p95 latency and total process RSS. The table is virtualized, but the 100,000-row GUI/RSS criteria remain open.

## Artifacts and reproducibility

Generated files are intentionally ignored by Git:

- `artifacts/network-results.json`: individual protocol cases and trap counters.
- `artifacts/native-results.json`: native smoke counters and measured cancellation.
- `artifacts/public-endpoint-smoke.json`: boolean success evidence without the observed IP.
- `artifacts/native-results.png`: the real native result table with synthetic proxies.
- `target/release/proxy-vouch` and `target/release/bundle/deb/ProxyVouch_0.1.0_amd64.deb`.

The package SHA-256 is `13a26b068f04e286c8511aaee7cea8a647dd2429c17418a41c08d97c2540aa00`; `SHA256SUMS` is written next to it. The standalone and packaged executable have the same ELF build ID; the packaged copy carries Tauri's `DEB` bundle marker instead of `UNK`. The extracted copy was tested directly, without installing it system-wide.

`make quality`, `make package` and the README's native-driver procedure reproduce the corresponding local checks. A generated CI workflow exists, but no remote CI run, push, release publication or system-wide package installation has been performed.

## Remaining acceptance work

- Extend native file-dialog coverage to cancellation, overwrite prompts, Unicode paths and OS-specific failure cases; the basic save/reopen path has passed.
- Full 100,000-row GUI performance, p95 input latency and RSS measurements on the stated baseline.
- Profile-change indicators, independent fallback validators and fuller per-stage timeout accounting.
- Full context-menu/bulk editing workflows and remaining import-preview metadata refinements.
- Expanded endpoint-breaker, retry, address-type rejection and late-run event regression cases.
- Native Windows/macOS builds, TLS/DNS behavior, GUI acceptance, signing and packaging.

The corresponding tasks remain open. A built Debian package is not a claim that all PRD requirements or release gates have passed.
