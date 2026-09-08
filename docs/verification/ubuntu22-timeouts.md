# Ubuntu 22.04 proxy handshake timeout correction

Date: 2026-09-05. Source version: 0.1.2.

## Reproduction and cause

The unmodified core and protocol fixtures were built and run in an Ubuntu 22.04 Docker container with system libcurl 7.81.0 and Python 3.10.12. The `explicit wrong protocol` case reproduced the release failure: a SOCKS5 greeting sent to the HTTP fixture returned `Failed / CONNECT_TIMEOUT` after about one second.

TCP had connected, but the server could not answer the SOCKS greeting. In [libcurl 7.81's connection implementation](https://github.com/curl/curl/blob/curl-7_81_0/lib/connect.c), `post_SOCKS` updates connection timing and peer information only after SOCKS negotiation succeeds. Reading `connect_time` and `primary_ip` after the failed negotiation therefore did not prove whether TCP had connected. The local libcurl 8.5.0 did not expose this failure.

## Correction

The response handler now observes the OS socket's peer address during the existing nonblocking transfer loop. It retains the fact that TCP connected even when SOCKS negotiation never completes. Temporary owned socket duplicates keep the observation safe if libcurl closes or replaces a descriptor; they are released as soon as a connection is observed, or when the attempt ends, including cancellation. No extra connection is opened, and the observer does not read or write network data.

The original case now requires `Inconclusive / PROXY_HANDSHAKE_TIMEOUT`, stage `protocol`, no detected protocol and exactly one SOCKS5 attempt. Additional fixtures cover silent handshakes for all six explicit protocol modes, target TLS/response timeouts through HTTP and SOCKS5, HTTP forwarding timeouts and refused TCP connections for all six modes. TLS verification, environment-proxy isolation and the expected `Inconclusive` status remain enforced.

## Validation

| Check | Ubuntu 22.04 container, libcurl 7.81.0 | Local Linux, libcurl 8.5.0 |
| --- | --- | --- |
| Protocol acceptance | 55 passed | 55 passed |
| Rust tests | 29 core tests passed | 33 workspace tests passed |
| Clippy, warnings denied | Core, all targets passed | Workspace, all targets passed |
| Actual TCP connect timeouts | All six protocol modes: `Failed / CONNECT_TIMEOUT / connect` | Same |

The separate TCP timeout check filled a loopback listener's Linux `listen(0)` accept queue with one connection and left it unaccepted. Further connects timed out before TCP establishment. This exercised real connection timeouts without an external destination or firewall changes; they remain distinct from a connected server that does not answer the handshake.

Commands used for the repeatable suite:

```sh
cargo build -p proxy-vouch-core --example check --locked
python3 scripts/network_fixtures.py
cargo test -p proxy-vouch-core --locked
cargo clippy -p proxy-vouch-core --all-targets --locked -- -D warnings
```

Local workspace tests, workspace Clippy, Rust formatting, version consistency and `git diff --check` also passed. Protocol reports and the before/after Ubuntu logs were saved under the ignored `artifacts/ubuntu22-timeouts/` directory.

The container verifies Ubuntu 22.04 userspace and its system libcurl on the host's Linux kernel. This is not a GitHub-hosted workflow run, native macOS/Windows verification or installer build.

## Release

VERSION and existing tags were not changed. Commit this correction and use a fresh version/tag following [the release procedure](../releases.md); retrying an old tag still builds its original source.
