import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

export function parseVersion(value) {
  const version = value.trim();
  const match =
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(
      version,
    );
  if (!match || match[4]?.split(".").some((part) => /^0\d+$/.test(part))) {
    throw new Error(
      "VERSION must contain a SemVer version without a v prefix, for example 0.2.0 or 0.2.0-rc.1.",
    );
  }
  return version;
}

export function readVersion(root = projectRoot) {
  return parseVersion(fs.readFileSync(path.join(root, "VERSION"), "utf8"));
}

export function synchronizeVersion(root = projectRoot, check = false) {
  const version = readVersion(root);
  const updates = new Map();
  const packageFile = path.join(root, "package.json");
  const pkg = JSON.parse(fs.readFileSync(packageFile, "utf8"));
  if (pkg.version !== version) {
    pkg.version = version;
    updates.set(packageFile, JSON.stringify(pkg, null, 2) + "\n");
  }
  const cargoFile = path.join(root, "Cargo.toml");
  const cargo = fs.readFileSync(cargoFile, "utf8");
  const workspace =
    /(^\[workspace\.package\]\r?\n)([\s\S]*?)(?=^\[|$(?![\s\S]))/m;
  const section = workspace.exec(cargo);
  if (!section || !/^version\s*=\s*"[^"]+"/m.test(section[2])) {
    throw new Error("Cargo.toml must define workspace.package.version.");
  }
  const updatedCargo = cargo.replace(
    workspace,
    (_, heading, body) =>
      heading +
      body.replace(/^version\s*=\s*"[^"]+"/m, `version = "${version}"`),
  );
  if (cargo !== updatedCargo) updates.set(cargoFile, updatedCargo);

  // Only local workspace package versions change; dependency resolutions stay locked.
  const lockFile = path.join(root, "Cargo.lock");
  const lock = fs.readFileSync(lockFile, "utf8");
  const names = new Set(["proxy-vouch", "proxy-vouch-core"]);
  const found = new Set();
  const updatedLock = lock.replace(
    /^\[\[package\]\]\r?\n[\s\S]*?(?=^\[\[package\]\]|$(?![\s\S]))/gm,
    (block) => {
      const name = /^name = "([^"]+)"/m.exec(block)?.[1];
      if (!names.has(name)) return block;
      if (found.has(name))
        throw new Error(`Duplicate workspace package in Cargo.lock: ${name}`);
      found.add(name);
      return block.replace(/^version = "[^"]+"/m, `version = "${version}"`);
    },
  );
  if (found.size !== names.size)
    throw new Error(
      "Cargo.lock is missing a workspace package. Run cargo generate-lockfile.",
    );
  if (lock !== updatedLock) updates.set(lockFile, updatedLock);

  const tauri = JSON.parse(
    fs.readFileSync(path.join(root, "src-tauri/tauri.conf.json"), "utf8"),
  );
  if (tauri.version !== "../package.json")
    throw new Error("Tauri version must reference ../package.json.");
  if (check && updates.size)
    throw new Error(
      `Version metadata differs from VERSION in ${[...updates.keys()].map((file) => path.relative(root, file)).join(", ")}. Run make version, then commit the changes.`,
    );
  if (!check) for (const [file, text] of updates) fs.writeFileSync(file, text);
  return {
    version,
    changed: [...updates.keys()].map((file) => path.relative(root, file)),
  };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  try {
    const command = process.argv[2] || "check";
    if (!["sync", "check", "print"].includes(command))
      throw new Error("Usage: node scripts/version.mjs sync|check|print");
    if (command === "print") console.log(readVersion());
    else {
      const result = synchronizeVersion(projectRoot, command === "check");
      console.log(
        `Version ${result.version}${result.changed.length ? `: updated ${result.changed.join(", ")}` : ": consistent"}`,
      );
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
