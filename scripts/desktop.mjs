import { fork } from "node:child_process";
import { run as runTauri } from "@tauri-apps/cli";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { projectRoot } from "./version.mjs";

export async function startDesktop(
  args = [],
  { serverOptions = {}, run = runTauri } = {},
) {
  const frontend = fork(new URL("./dev-server.mjs", import.meta.url), {
    cwd: projectRoot,
    execArgv: [],
    stdio: ["inherit", "inherit", "inherit", "ipc"],
  });
  const closed = new Promise((resolve) => {
    frontend.once("exit", resolve);
    frontend.once("error", resolve);
  });
  try {
    const ready = new Promise((resolve, reject) => {
      frontend.once("error", reject);
      frontend.once("exit", (code, signal) =>
        reject(
          new Error(`Vite stopped before opening a port (${signal ?? code}).`),
        ),
      );
      frontend.once("message", (message) => {
        if (message.error) reject(new Error(message.error));
        else resolve(message.url);
      });
    });
    frontend.send(serverOptions);
    const url = await ready;
    console.log(`ProxyVouch development URL: ${url}`);
    const config = JSON.stringify({
      build: { devUrl: url, beforeDevCommand: null },
    });
    // Merge our URL last, before any arguments forwarded to Cargo or the app.
    const separator = args.indexOf("--");
    const options = separator < 0 ? args : args.slice(0, separator);
    const forwarded = separator < 0 ? [] : args.slice(separator);
    // Use the same native CLI entrypoint as the standard tauri command, so it
    // retains ownership of Cargo, app restarts and signal handling.
    await run(
      ["dev", ...options, "--config", config, ...forwarded],
      "pnpm desktop",
    );
  } finally {
    if (frontend.connected) frontend.disconnect();
    await closed;
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  try {
    await startDesktop(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
