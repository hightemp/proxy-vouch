import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";
import { projectRoot, readVersion } from "./version.mjs";

export const platforms = {
  linux_x86_64: [
    { folder: "deb", extension: ".deb" },
    { folder: "appimage", extension: ".AppImage" },
  ],
  windows_x86_64: [{ folder: "nsis", extension: ".exe" }],
  macos_aarch64: [{ folder: "dmg", extension: ".dmg" }],
  macos_x86_64: [{ folder: "dmg", extension: ".dmg" }],
};

export function expectedAssets(version) {
  return Object.entries(platforms)
    .flatMap(([platform, formats]) =>
      formats.map(
        (format) => `proxy-vouch_${version}_${platform}${format.extension}`,
      ),
    )
    .sort();
}

export function collectAssets(root, target, platform, output) {
  if (!/^[a-z0-9_-]+$/.test(target) || !platforms[platform])
    throw new Error("Choose a supported build target and release platform.");
  const version = readVersion(root);
  fs.mkdirSync(output, { recursive: true });
  return platforms[platform].map((format) => {
    const directory = path.join(
      root,
      "target",
      target,
      "release/bundle",
      format.folder,
    );
    const candidates = fs
      .readdirSync(directory)
      .filter(
        (name) =>
          name.includes(`_${version}_`) && name.endsWith(format.extension),
      );
    if (candidates.length !== 1)
      throw new Error(
        `Expected one ${version} ${format.extension} bundle for ${platform}, found ${candidates.length}.`,
      );
    const source = path.join(directory, candidates[0]);
    const destination = path.join(
      output,
      `proxy-vouch_${version}_${platform}${format.extension}`,
    );
    if (!fs.statSync(source).isFile() || fs.statSync(source).size === 0)
      throw new Error("A generated release asset is empty or not a file.");
    fs.copyFileSync(source, destination);
    fs.chmodSync(destination, fs.statSync(source).mode & 0o777);
    return destination;
  });
}

export function verifyAssets(directory, version) {
  const expected = expectedAssets(version);
  const actual = fs
    .readdirSync(directory)
    .filter((name) => name !== "SHA256SUMS")
    .sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected))
    throw new Error(
      "The release asset set is incomplete or contains unexpected files. All platform packages, including AppImage, are required.",
    );
  for (const name of expected) {
    const stat = fs.lstatSync(path.join(directory, name));
    if (!stat.isFile() || stat.size === 0)
      throw new Error(`Invalid release asset: ${name}`);
  }
  return expected.map((name) => path.join(directory, name));
}

export function writeChecksums(directory, version) {
  const assets = verifyAssets(directory, version);
  const checksums =
    assets
      .map(
        (file) =>
          `${createHash("sha256").update(fs.readFileSync(file)).digest("hex")}  ${path.basename(file)}`,
      )
      .join("\n") + "\n";
  const destination = path.join(directory, "SHA256SUMS");
  fs.writeFileSync(destination, checksums);
  return [...assets, destination];
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  try {
    const [action, ...argv] = process.argv.slice(2);
    const options = {};
    for (let i = 0; i < argv.length; i += 2) {
      if (
        !["--target", "--platform", "--out"].includes(argv[i]) ||
        !argv[i + 1]
      )
        throw new Error(
          "Usage: release-assets.mjs collect --target triple --platform platform --out directory | verify --out directory",
        );
      options[argv[i].slice(2)] = argv[i + 1];
    }
    if (!options.out) throw new Error("An output directory is required.");
    if (action === "collect")
      console.log(
        collectAssets(
          projectRoot,
          options.target,
          options.platform,
          options.out,
        ).join("\n"),
      );
    else if (action === "verify")
      console.log(writeChecksums(options.out, readVersion()).join("\n"));
    else throw new Error("Choose collect or verify.");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
