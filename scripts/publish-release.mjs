import fs from "node:fs";
import path from "node:path";
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
  const notesFile = path.join(root, "artifacts/release-notes.md");
  fs.mkdirSync(path.dirname(notesFile), { recursive: true });
  fs.writeFileSync(notesFile, releaseNotes(root, repository, tag));
  const call = (args) => {
    const result = execute("gh", args, root);
    if (result.status !== 0)
      throw new Error(
        `GitHub release ${args[1]} failed. Any created release remains a draft; rerun the workflow after resolving the error.`,
      );
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
    call([
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
    ]);
  }
  call([
    "release",
    "upload",
    tag,
    ...assets,
    "--repo",
    repository,
    "--clobber",
  ]);
  // The public release becomes visible only after every upload has succeeded.
  call([
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
  ]);
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
