import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { startDesktop } from "../desktop.mjs";
import { startDevServer } from "../dev-server.mjs";

async function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "proxy-vouch-dev-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.writeFileSync(
    path.join(root, "index.html"),
    "<h1>ProxyVouch port fixture</h1>",
  );
  const occupied = net.createServer();
  occupied.listen(0, "127.0.0.1");
  await once(occupied, "listening");
  t.after(() => {
    if (occupied.listening) occupied.close();
  });
  return {
    root,
    occupied,
    serverOptions: {
      root,
      configFile: false,
      logLevel: "silent",
      server: { port: occupied.address().port },
    },
  };
}

function listening(port) {
  return new Promise((resolve) => {
    const socket = net.connect({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => resolve(false));
  });
}

async function until(predicate) {
  const deadline = Date.now() + 5000;
  while (!(await predicate())) {
    assert.ok(
      Date.now() < deadline,
      "Timed out waiting for development process cleanup",
    );
    await delay(20);
  }
}

test("occupied ports are skipped and Vite keeps its URL across restarts", async (t) => {
  const { occupied, serverOptions } = await fixture(t);
  const { server, url } = await startDevServer(serverOptions);
  t.after(() => server.close());
  const selected = Number(new URL(url).port);
  assert.ok(selected > serverOptions.server.port);
  assert.match(await (await fetch(url)).text(), /ProxyVouch port fixture/);
  await new Promise((resolve) => occupied.close(resolve));
  await server.restart();
  assert.equal(server.httpServer.address().port, selected);
  assert.match(await (await fetch(url)).text(), /ProxyVouch port fixture/);
});

for (const failure of [false, true]) {
  test(`Tauri receives the live URL and the port closes after ${failure ? "failure" : "exit"}`, async (t) => {
    const { serverOptions } = await fixture(t);
    let selected;
    const launch = startDesktop(
      ["--no-watch", "--", "--locked", "--", "app-argument"],
      {
        serverOptions,
        run: async (args) => {
          const configIndex = args.indexOf("--config");
          const { build } = JSON.parse(args[configIndex + 1]);
          assert.equal(build.beforeDevCommand, null);
          selected = Number(new URL(build.devUrl).port);
          assert.ok(selected > serverOptions.server.port);
          assert.match(
            await (await fetch(build.devUrl)).text(),
            /ProxyVouch port fixture/,
          );
          assert.deepEqual(args.slice(configIndex + 2), [
            "--",
            "--locked",
            "--",
            "app-argument",
          ]);
          if (failure) throw new Error("Tauri startup fixture failed");
        },
      },
    );
    if (failure) await assert.rejects(launch, /Tauri startup fixture failed/);
    else await launch;
    assert.equal(await listening(selected), false);
    assert.equal(await listening(serverOptions.server.port), true);
  });
}

test("the frontend releases its port if the launcher is terminated", async (t) => {
  const { root, serverOptions } = await fixture(t);
  const report = path.join(root, "ready.json");
  const moduleUrl = new URL("../desktop.mjs", import.meta.url).href;
  const script = `
    import { startDesktop } from ${JSON.stringify(moduleUrl)};
    import fs from 'node:fs';
    await startDesktop([], {
      serverOptions: ${JSON.stringify(serverOptions)},
      run: async (args) => {
        fs.writeFileSync(${JSON.stringify(report)}, args[args.indexOf('--config') + 1]);
        await new Promise(() => {});
      },
    });
  `;
  const child = spawn(process.execPath, ["--input-type=module", "-e", script], {
    stdio: ["ignore", "ignore", "inherit"],
  });
  const exited = once(child, "exit");
  t.after(() => {
    if (child.exitCode === null) child.kill();
  });
  await until(() => fs.existsSync(report));
  const { build } = JSON.parse(fs.readFileSync(report, "utf8"));
  const selected = Number(new URL(build.devUrl).port);
  assert.equal(await listening(selected), true);
  child.kill("SIGTERM");
  await exited;
  await until(async () => !(await listening(selected)));
  assert.equal(await listening(serverOptions.server.port), true);
});
