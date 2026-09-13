import { createServer } from "vite";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { projectRoot } from "./version.mjs";

export async function startDevServer(options = {}) {
  const server = await createServer({
    root: projectRoot,
    ...options,
    server: {
      ...options.server,
      host: "127.0.0.1",
      port: options.server?.port ?? 1420,
      strictPort: false,
    },
  });
  try {
    await server.listen();
    const address = server.httpServer?.address();
    if (!address || typeof address === "string")
      throw new Error("Vite did not open a local TCP port.");
    // Keep the chosen URL when Vite restarts after a configuration change,
    // even if the originally requested port has become available meanwhile.
    Object.assign(server.config.inlineConfig.server, {
      port: address.port,
      strictPort: true,
    });
    return { server, url: `http://127.0.0.1:${address.port}` };
  } catch (error) {
    await server.close();
    throw error;
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  let activeServer;
  // The IPC connection also closes if Tauri exits through its native signal
  // handler. The frontend must not keep its port after the launcher is gone.
  process.once("disconnect", () => void activeServer?.close());
  process.once("message", async (options) => {
    try {
      const { server, url } = await startDevServer(options);
      activeServer = server;
      if (process.connected) process.send({ url });
      else await server.close();
    } catch (error) {
      if (process.connected) {
        process.send({ error: error.message });
        process.disconnect();
      }
      process.exitCode = 1;
    }
  });
}
