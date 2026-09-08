import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { parseVersion, readVersion, synchronizeVersion } from "../version.mjs";
import {
  prepareRelease,
  publishTag,
  releaseNotes,
  repositoryFromRemote,
  validateTag,
} from "../release.mjs";
import {
  collectAssets,
  expectedAssets,
  writeChecksums,
} from "../release-assets.mjs";
import { publishGithubRelease } from "../publish-release.mjs";

function git(root, ...args) {
  return execFileSync("git", args, {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}
function fixture(t, version = "0.1.0") {
  const directory = fs.mkdtempSync(
    path.join(os.tmpdir(), "proxy-vouch-release-"),
  );
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const root = path.join(directory, "repo");
  fs.mkdirSync(path.join(root, "src-tauri"), { recursive: true });
  fs.writeFileSync(path.join(root, "VERSION"), version + "\n");
  fs.writeFileSync(
    path.join(root, "package.json"),
    JSON.stringify({ name: "proxy-vouch", version }, null, 2) + "\n",
  );
  fs.writeFileSync(
    path.join(root, "Cargo.toml"),
    `[workspace]\nmembers = []\n\n[workspace.package]\nversion = "${version}"\nedition = "2021"\n\n[profile.release]\nstrip = true\n`,
  );
  fs.writeFileSync(
    path.join(root, "Cargo.lock"),
    `version = 4\n\n[[package]]\nname = "external"\nversion = "8.0.0"\n\n[[package]]\nname = "proxy-vouch"\nversion = "${version}"\n\n[[package]]\nname = "proxy-vouch-core"\nversion = "${version}"\n`,
  );
  fs.writeFileSync(
    path.join(root, "src-tauri/tauri.conf.json"),
    JSON.stringify({ version: "../package.json" }),
  );
  fs.writeFileSync(path.join(root, ".gitignore"), "artifacts/\n");
  git(root, "init", "-b", "main");
  git(root, "config", "user.name", "Release Tests");
  git(root, "config", "user.email", "release-tests@example.invalid");
  git(root, "config", "commit.gpgSign", "false");
  git(root, "config", "tag.gpgSign", "false");
  git(root, "config", "core.hooksPath", path.join(root, ".git/hooks"));
  git(root, "add", ".");
  git(root, "commit", "-m", "Initial implementation");
  const remote = path.join(directory, "remote.git");
  git(directory, "init", "--bare", remote);
  git(remote, "config", "core.hooksPath", path.join(remote, "hooks"));
  git(root, "remote", "add", "origin", remote);
  git(root, "push", "-u", "origin", "main");
  return { root, remote, directory };
}

function bump(root, version, subject = "Change application version") {
  fs.writeFileSync(path.join(root, "VERSION"), version + "\n");
  synchronizeVersion(root);
  git(root, "add", ".");
  git(root, "commit", "-m", subject);
}

function assets(root, version) {
  const directory = path.join(root, "artifacts/release");
  fs.mkdirSync(directory, { recursive: true });
  for (const name of expectedAssets(version))
    fs.writeFileSync(path.join(directory, name), `fixture:${name}`);
  return directory;
}

test("VERSION validates SemVer and rejects malformed or injected values", () => {
  for (const valid of [
    "0.1.0",
    "1.2.3-rc.1",
    "1.2.3+build-9",
    "2.0.0-alpha.1+git.abc",
  ])
    assert.equal(parseVersion(valid + "\n"), valid);
  for (const invalid of [
    "v1.2.3",
    "01.2.3",
    "1.2",
    "1.2.3-rc.01",
    "1.2.3\n2.0.0",
    "1.2.3; echo bad",
    "1.2.3+",
    "",
  ])
    assert.throws(() => parseVersion(invalid), /SemVer/);
});

test("synchronization follows VERSION and preserves external dependency resolutions", (t) => {
  const { root } = fixture(t);
  fs.writeFileSync(path.join(root, "VERSION"), "0.2.0-rc.1\n");
  assert.throws(() => synchronizeVersion(root, true), /differs from VERSION/);
  assert.equal(
    JSON.parse(fs.readFileSync(path.join(root, "package.json"))).version,
    "0.1.0",
  );
  assert.equal(synchronizeVersion(root).changed.length, 3);
  assert.equal(
    JSON.parse(fs.readFileSync(path.join(root, "package.json"))).version,
    "0.2.0-rc.1",
  );
  const lock = fs.readFileSync(path.join(root, "Cargo.lock"), "utf8");
  assert.match(lock, /name = "external"\nversion = "8.0.0"/);
  assert.equal(lock.match(/version = "0.2.0-rc.1"/g).length, 2);
  assert.equal(synchronizeVersion(root, true).version, "0.2.0-rc.1");
  assert.equal(synchronizeVersion(root).changed.length, 0);
});

test("GitHub URLs are canonical and never retain remote credentials", () => {
  assert.equal(
    repositoryFromRemote("git@github.com:example/proxy-vouch.git"),
    "example/proxy-vouch",
  );
  assert.equal(
    repositoryFromRemote(
      "https://user:secret@github.com/example/proxy-vouch.git",
    ),
    "example/proxy-vouch",
  );
  assert.throws(
    () => repositoryFromRemote("ssh://git@unrelated.example/repository"),
    /GitHub/,
  );
});

test("release notes include only commits since the previous reachable version tag", (t) => {
  const { root } = fixture(t);
  const initial = git(root, "rev-parse", "HEAD");
  git(root, "tag", "v0.1.0");
  bump(root, "0.2.0", "Fix [layout] and <input> rendering");
  const head = git(root, "rev-parse", "HEAD");
  const notes = releaseNotes(root, "example/proxy-vouch");
  assert.ok(notes.includes(`/commit/${head}`));
  assert.ok(!notes.includes(`/commit/${initial}`));
  assert.ok(notes.includes("Fix \\[layout\\] and \\<input\\> rendering"));
  assert.ok(notes.includes("/compare/v0.1.0...v0.2.0"));
  assert.ok(!notes.includes("Initial implementation"));
});

test("first release notes include the initial history", (t) => {
  const { root } = fixture(t);
  assert.match(
    releaseNotes(root, "example/proxy-vouch"),
    /Initial implementation/,
  );
  assert.match(releaseNotes(root, "example/proxy-vouch"), /Initial release/);
});

test("dry run creates no tag and publish pushes the committed VERSION tag", (t) => {
  const { root, remote } = fixture(t);
  const options = { repository: "example/proxy-vouch", dryRun: true };
  assert.equal(publishTag(root, options).pushed, false);
  assert.equal(git(root, "tag", "--list"), "");
  assert.equal(git(root, "ls-remote", "--tags", remote), "");
  const plan = publishTag(root, { ...options, dryRun: false });
  assert.equal(plan.tag, "v0.1.0");
  assert.equal(git(root, "cat-file", "-t", "refs/tags/v0.1.0"), "tag");
  assert.match(git(root, "ls-remote", "--tags", remote), /refs\/tags\/v0.1.0/);
  assert.throws(() => publishTag(root, options), /already on/);
});

test("dirty or unpushed changes are rejected before tag creation", (t) => {
  const { root } = fixture(t);
  fs.writeFileSync(path.join(root, "unfinished.txt"), "work");
  assert.throws(
    () => prepareRelease(root, { repository: "example/proxy-vouch" }),
    /clean/,
  );
  git(root, "add", ".");
  git(root, "commit", "-m", "Unpushed work");
  assert.throws(
    () => prepareRelease(root, { repository: "example/proxy-vouch" }),
    /Push the current/,
  );
  assert.equal(git(root, "tag", "--list"), "");
});

test("conflicting local tags are not moved; an unpushed correct tag can be resumed", (t) => {
  const { root } = fixture(t);
  const first = git(root, "rev-parse", "HEAD");
  bump(root, "0.2.0");
  git(root, "push", "origin", "main");
  git(root, "tag", "v0.2.0", first);
  assert.throws(
    () => prepareRelease(root, { repository: "example/proxy-vouch" }),
    /different commit/,
  );
  git(root, "tag", "-d", "v0.2.0");
  git(root, "tag", "-a", "v0.2.0", "-m", "Release v0.2.0");
  assert.equal(
    publishTag(root, { repository: "example/proxy-vouch" }).pushed,
    true,
  );
});

test("a rejected push retains the local tag for a safe retry", (t) => {
  const { root, remote } = fixture(t);
  const hook = path.join(remote, "hooks/pre-receive");
  fs.writeFileSync(hook, "#!/bin/sh\nexit 1\n", { mode: 0o755 });
  assert.throws(
    () => publishTag(root, { repository: "example/proxy-vouch" }),
    /Local tag v0.1.0 was retained/,
  );
  assert.equal(
    git(root, "rev-parse", "v0.1.0^{commit}"),
    git(root, "rev-parse", "HEAD"),
  );
  assert.equal(git(root, "ls-remote", "--tags", "origin"), "");
  fs.unlinkSync(hook);
  assert.equal(
    publishTag(root, { repository: "example/proxy-vouch" }).pushed,
    true,
  );
});

test("workflow rejects tag/version mismatch", (t) => {
  const { root } = fixture(t);
  git(root, "tag", "v9.0.0");
  assert.throws(() => validateTag(root, "v9.0.0"), /does not match VERSION/);
});

test("asset collection requires AppImage and ignores stale versions", (t) => {
  const { root } = fixture(t);
  const target = "x86_64-unknown-linux-gnu";
  for (const [folder, extension] of [
    ["deb", ".deb"],
    ["appimage", ".AppImage"],
  ]) {
    const dir = path.join(root, "target", target, "release/bundle", folder);
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(
      path.join(dir, `ProxyVouch_0.1.0_amd64${extension}`),
      "current",
    );
    fs.writeFileSync(
      path.join(dir, `ProxyVouch_0.0.9_amd64${extension}`),
      "stale",
    );
  }
  const output = path.join(root, "artifacts/linux");
  assert.equal(collectAssets(root, target, "linux_x86_64", output).length, 2);
  assert.ok(
    fs.readFileSync(
      path.join(output, "proxy-vouch_0.1.0_linux_x86_64.AppImage"),
      "utf8",
    ) === "current",
  );
  assert.throws(() => writeChecksums(output, "0.1.0"), /incomplete/);
});

test("missing assets block publication before any GitHub mutation", (t) => {
  const { root } = fixture(t);
  git(root, "tag", "v0.1.0");
  const directory = assets(root, "0.1.0");
  fs.unlinkSync(
    path.join(directory, "proxy-vouch_0.1.0_linux_x86_64.AppImage"),
  );
  const calls = [];
  assert.throws(
    () =>
      publishGithubRelease(
        root,
        "example/proxy-vouch",
        "v0.1.0",
        directory,
        (...args) => calls.push(args),
      ),
    /incomplete/,
  );
  assert.deepEqual(calls, []);
});

test("draft is published only after all packages and checksums upload", (t) => {
  const { root } = fixture(t);
  git(root, "tag", "v0.1.0");
  const directory = assets(root, "0.1.0");
  const calls = [];
  publishGithubRelease(
    root,
    "example/proxy-vouch",
    "v0.1.0",
    directory,
    (program, args) => {
      assert.equal(program, "gh");
      calls.push(args);
      return args[1] === "view"
        ? { status: 1, stderr: "release not found" }
        : { status: 0, stdout: "" };
    },
  );
  assert.deepEqual(
    calls.map((args) => args[1]),
    ["view", "create", "upload", "edit"],
  );
  assert.ok(calls[1].includes("--verify-tag") && calls[1].includes("--draft"));
  assert.equal(calls[2].filter((arg) => arg.startsWith(directory)).length, 6);
  assert.ok(calls[3].includes("--draft=false"));
  assert.match(
    fs.readFileSync(path.join(root, "artifacts/release-notes.md"), "utf8"),
    /https:\/\/github.com\/example\/proxy-vouch\/commit\//,
  );
  assert.equal(
    fs
      .readFileSync(path.join(directory, "SHA256SUMS"), "utf8")
      .trim()
      .split("\n").length,
    5,
  );
});

test("upload failures leave drafts unpublished and published releases are immutable", (t) => {
  const { root } = fixture(t);
  git(root, "tag", "v0.1.0");
  const directory = assets(root, "0.1.0");
  const calls = [];
  assert.throws(
    () =>
      publishGithubRelease(
        root,
        "example/proxy-vouch",
        "v0.1.0",
        directory,
        (_, args) => {
          calls.push(args[1]);
          if (args[1] === "view")
            return { status: 0, stdout: JSON.stringify({ isDraft: true }) };
          return { status: 1, stderr: "upload failed" };
        },
      ),
    /remains a draft/,
  );
  assert.deepEqual(calls, ["view", "upload"]);
  assert.throws(
    () =>
      publishGithubRelease(
        root,
        "example/proxy-vouch",
        "v0.1.0",
        directory,
        () => ({ status: 0, stdout: JSON.stringify({ isDraft: false }) }),
      ),
    /already published/,
  );
});

test("a dash in build metadata does not mark a stable release as prerelease", (t) => {
  const { root } = fixture(t, "1.0.0+build-9");
  const tag = `v${readVersion(root)}`;
  git(root, "tag", tag);
  const calls = [];
  publishGithubRelease(
    root,
    "example/proxy-vouch",
    tag,
    assets(root, readVersion(root)),
    (_, args) => {
      calls.push(args);
      return args[1] === "view"
        ? { status: 1, stderr: "release not found" }
        : { status: 0, stdout: "" };
    },
  );
  assert.ok(!calls[1].includes("--prerelease"));
  assert.ok(calls[3].includes("--prerelease=false"));
});
