# Building and publishing releases

`VERSION` is the source of truth. It contains a plain SemVer value, without the `v` prefix. The release tag is `v` followed by that value.

## Version flow

`make version` synchronizes package.json, Cargo.toml and the two local workspace package entries in Cargo.lock. External dependency resolutions stay unchanged. Tauri reads the version from package.json; Vite embeds VERSION into the interface; the checker's User-Agent uses the Cargo package version.

`pnpm dev`, `pnpm desktop`, `pnpm build` and `pnpm tauri` synchronize metadata before starting. A raw Cargo build rejects a mismatched VERSION rather than silently building the desktop app with stale metadata. `make version-check` verifies consistency without writing files.

After changing VERSION, commit VERSION together with the synchronized metadata before publishing. Changing only a build metadata suffix does not make a release a prerelease; versions such as `0.2.0-rc.1` do.

## Local builds

```sh
make build           # Release executable for the current OS
make package         # Linux: .deb + AppImage; Windows: NSIS; macOS: DMG
make appimage        # Linux AppImage only
```

Linux packages appear under `target/release/bundle/deb/` and `target/release/bundle/appimage/`. `make package` and `make appimage` enable the packaging tools' extract-and-run mode, so their build-time helpers do not require a FUSE mount.

When a FUSE mount is unavailable, run the produced AppImage from the project root with `APPIMAGE_EXTRACT_AND_RUN=1 "./target/release/bundle/appimage/ProxyVouch_$(cat VERSION)_amd64.AppImage"`. This is the mode used by the local AppImage smoke test.

The release workflow builds Linux on Ubuntu 22.04 to avoid unnecessarily raising the minimum glibc version. A locally built AppImage still inherits its build host's glibc requirements. This follows [Tauri's AppImage guidance](https://v2.tauri.app/distribute/appimage/).

## Publish from Make

Configure a GitHub remote first if the repository does not have one:

```sh
git remote add origin git@github.com:OWNER/REPOSITORY.git
```

The repository must already exist and allow you to push tags. The command does not create a repository or configure credentials.

Typical release sequence:

```sh
# Edit VERSION to the intended release version.
make version
make quality
git add VERSION package.json Cargo.toml Cargo.lock
git commit -m "chore: prepare release"
git push origin main
make release-dry-run
make release
```

Commit the implementation changes and workflow files as well before the first release. Use your actual branch name instead of `main` if it differs. `make release` verifies that the current branch's remote commit matches HEAD, creates an annotated version tag and pushes that tag. It never force-pushes, commits files for you or moves a conflicting tag. A local tag left behind after a failed push can be reused when it still points to the same commit; an already pushed tag is rejected.

`make release-dry-run` performs the same checks and previews the release notes without creating or pushing a tag. `make release-notes` prints notes from the current local history; fetch remote tags first if that history is out of date. The actual GitHub release description is generated in Actions from a complete checkout, using the nearest reachable earlier version tag, or all commits for the first release. Each commit has a direct GitHub link, and subsequent releases include a comparison link.

The remote defaults to `origin`. For a different remote or a GitHub SSH alias:

```sh
RELEASE_REMOTE=upstream RELEASE_REPOSITORY=owner/repository make release
```

`RELEASE_REPOSITORY` only supplies the GitHub owner/name for links; it does not change the push destination. Local publishing requires Git and Node. GitHub CLI authentication is provided by Actions for the release-upload job.

## GitHub Actions

The [release workflow](../.github/workflows/release.yml) runs on pushed `v*` tags. It also accepts an existing tag through manual workflow dispatch, useful for retrying a failed build. The tag must agree with committed VERSION and resolve to the checked-out commit.

| Runner | Target | Required packages |
| --- | --- | --- |
| Ubuntu 22.04 | Linux x86_64 | `.deb`, `.AppImage` |
| Windows 2022 | Windows x86_64 | NSIS `.exe` |
| macOS 15 | macOS arm64 | `.dmg` |
| macOS 15 Intel | macOS x86_64 | `.dmg` |

Platform builds run on native runners. See [GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners) for runner labels and [Tauri's pipeline documentation](https://v2.tauri.app/distribute/pipelines/github/) for packaging context.

Every matrix job runs Rust tests and builds its packages with the locked dependencies. Artifacts get unique version/platform names. The publishing job starts only when all builds succeed and requires all five packages, including AppImage. It calculates SHA256SUMS, creates or resumes a draft, uploads the complete artifact set and then makes the release public. If an upload fails, the release stays a draft. Already published releases are not overwritten.

If GitHub returns an error after creating a draft, uploading assets or publishing, the publisher checks the remote state before failing. Upload recovery requires every expected filename, size and SHA-256 digest; publication recovery also requires the expected tag, public status, title, notes and prerelease flag. An unverified or partial operation still stops the workflow. Existing public releases remain protected against overwrites.

The macOS bundle uses ad-hoc signing; Apple notarization and a trusted Windows publisher signature are not configured. This workflow does not establish completed GUI acceptance on Windows/macOS before it has run there.

No real tag, remote push or GitHub release is needed to test the release scripts:

```sh
make test-release
```

Tests use temporary repositories and local bare remotes; GitHub mutations are replaced by a recorded command interface. They cover version synchronization, tag guards, linked commit notes, complete assets, draft retries and publication ordering.
