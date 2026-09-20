import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";
import {
  command,
  repositoryName,
  releaseNotes,
  validateTag,
} from "./release.mjs";
import { projectRoot } from "./version.mjs";
import { writeChecksums } from "./release-assets.mjs";

export function publishGithubRelease(
  root,
  repository,
  tag,
  directory,
  execute = command,
) {
  repository = repositoryName(repository);
  const version = validateTag(root, tag);
  const prerelease = version.split("+")[0].includes("-");
  // Validate the entire asset set before creating or changing a GitHub release.
  const assets = writeChecksums(directory, version);
  const expectedAssets = assets.map((file) => ({
    name: path.basename(file),
    size: fs.statSync(file).size,
    digest: `sha256:${createHash("sha256").update(fs.readFileSync(file)).digest("hex")}`,
  }));
  const notesFile = path.join(root, "artifacts/release-notes.md");
  fs.mkdirSync(path.dirname(notesFile), { recursive: true });
  const notes = releaseNotes(root, repository, tag);
  fs.writeFileSync(notesFile, notes);
  const readRemote = () => {
    const response = execute(
      "gh",
      ["api", `repos/${repository}/releases/tags/${encodeURIComponent(tag)}`],
      root,
    );
    if (response.status !== 0) return null;
    try {
      return JSON.parse(response.stdout);
    } catch {
      return null;
    }
  };
  const isDraft = (release) =>
    release?.tag_name === tag && release.draft === true;
  const hasAssets = (release) =>
    Array.isArray(release?.assets) &&
    release.assets.length === expectedAssets.length &&
    expectedAssets.every((expected) =>
      release.assets.some(
        (asset) =>
          asset.name === expected.name &&
          asset.size === expected.size &&
          asset.digest === expected.digest &&
          asset.state === "uploaded",
      ),
    );
  const isPublished = (release) =>
    release?.tag_name === tag &&
    release.draft === false &&
    release.prerelease === prerelease &&
    release.name === `ProxyVouch ${tag}` &&
    release.body === notes &&
    hasAssets(release);
  const call = (args, confirm) => {
    const result = execute("gh", args, root);
    if (result.status !== 0) {
      // A failed response does not imply a failed mutation: GitHub may have
      // committed the draft/publication already. Verify before proceeding.
      if (confirm(readRemote())) return result;
      const httpStatus = /\bHTTP (\d{3})\b/.exec(result.stderr || "")?.[1];
      throw new Error(
        `GitHub release ${args[1]} failed${httpStatus ? ` (HTTP ${httpStatus})` : ""}. The completed operation could not be verified remotely. Inspect the release before retrying; it may already be public.`,
      );
    }
    return result;
  };
  const existing = execute(
    "gh",
    ["release", "view", tag, "--repo", repository, "--json", "isDraft"],
    root,
  );
  if (existing.status === 0) {
    if (!JSON.parse(existing.stdout).isDraft)
      throw new Error(
        "This release is already published. Published releases are not overwritten.",
      );
  } else {
    if (!/release not found|HTTP 404|Not Found/i.test(existing.stderr || ""))
      throw new Error(
        "Cannot inspect the GitHub release. Check authentication and repository access.",
      );
    call(
      [
        "release",
        "create",
        tag,
        "--repo",
        repository,
        "--verify-tag",
        "--draft",
        "--title",
        `ProxyVouch ${tag}`,
        "--notes-file",
        notesFile,
        ...(prerelease ? ["--prerelease"] : []),
      ],
      isDraft,
    );
  }
  call(
    ["release", "upload", tag, ...assets, "--repo", repository, "--clobber"],
    (release) => isDraft(release) && hasAssets(release),
  );
  // The public release becomes visible only after every upload has succeeded.
  call(
    [
      "release",
      "edit",
      tag,
      "--repo",
      repository,
      "--draft=false",
      "--title",
      `ProxyVouch ${tag}`,
      "--notes-file",
      notesFile,
      `--prerelease=${prerelease}`,
    ],
    isPublished,
  );
  return `https://github.com/${repository}/releases/tag/${encodeURIComponent(tag)}`;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  try {
    const tag = process.env.RELEASE_TAG;
    const repository = process.env.GITHUB_REPOSITORY;
    if (!tag || !repository)
      throw new Error("RELEASE_TAG and GITHUB_REPOSITORY must be set.");
    console.log(
      publishGithubRelease(
        projectRoot,
        repository,
        tag,
        path.resolve(process.argv[2] || "artifacts/release"),
      ),
    );
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
