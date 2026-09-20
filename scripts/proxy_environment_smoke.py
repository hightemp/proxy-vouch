#!/usr/bin/env python3
"""Verify the development WebView with a hostile ambient HTTP proxy."""
import argparse
import asyncio
import contextlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from native_smoke import Driver, ROOT


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def reachable(url):
    try:
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(url, timeout=.5) as response:
            return response.status == 200
    except Exception:
        return False


class Trap(BaseHTTPRequestHandler):
    hits = 0

    def reject(self):
        type(self).hits += 1
        body = b"Proxy trap: the local UI was sent to the environment proxy."
        self.send_response(503)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_GET = reject
    do_CONNECT = reject

    def log_message(self, *_args):
        pass


async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default=str(ROOT / "target/debug/proxy-vouch"))
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    tools = ROOT / "artifacts"
    driver_path = shutil.which("tauri-driver") or str(tools / "tools/bin/tauri-driver")
    webkit_path = shutil.which("WebKitWebDriver") or str(tools / "webdriver/extracted/usr/bin/WebKitWebDriver")
    environment = {key: value for key, value in os.environ.items() if key.lower() not in ("http_proxy", "https_proxy", "all_proxy", "no_proxy")}
    processes = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), Trap)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    proxy = f"http://127.0.0.1:{server.server_port}"
    cases = [
        ("uppercase", {"HTTP_PROXY": proxy, "HTTPS_PROXY": proxy}),
        ("lowercase", {"http_proxy": proxy, "https_proxy": proxy, "all_proxy": proxy}),
        ("existing_exclusions", {"HTTP_PROXY": proxy, "http_proxy": proxy, "NO_PROXY": "keep.example", "no_proxy": "also-keep.example"}),
    ]
    results = []
    with tempfile.TemporaryDirectory(prefix="proxy-vouch-startup-") as directory:
        directory = Path(directory)
        log_path = ROOT / "artifacts/proxy-environment-smoke.log"
        log_path.parent.mkdir(exist_ok=True)
        with log_path.open("w") as log:
            try:
                if not reachable("http://127.0.0.1:1420/"):
                    processes.append(subprocess.Popen([shutil.which("pnpm") or "pnpm", "dev"], cwd=ROOT, env=environment, stdout=log, stderr=log, start_new_session=True))
                    for _ in range(100):
                        if reachable("http://127.0.0.1:1420/"):
                            break
                        await asyncio.sleep(.1)
                    else:
                        raise RuntimeError("Vite did not start")
                port, native_port = free_port(), free_port()
                processes.append(subprocess.Popen(["xvfb-run", "-a", "-s", "-screen 0 1440x1000x24", driver_path, "--native-driver", webkit_path, "--port", str(port), "--native-port", str(native_port)], cwd=ROOT, env=environment, stdout=log, stderr=log, start_new_session=True))
                for _ in range(100):
                    if reachable(f"http://127.0.0.1:{port}/status"):
                        break
                    await asyncio.sleep(.1)
                else:
                    raise RuntimeError("WebDriver did not start")
                for name, variables in cases:
                    Trap.hits = 0
                    wrapper = directory / "launch-app"
                    wrapper.write_text(f"#!{sys.executable}\nimport os,sys\nenv=dict(os.environ)\nfor key in list(env):\n if key.lower() in ('http_proxy','https_proxy','all_proxy','no_proxy'):env.pop(key)\nenv.update({variables!r})\nenv['XDG_CONFIG_HOME']={str(directory / 'config' / name)!r}\nenv['XDG_DATA_HOME']={str(directory / 'data' / name)!r}\nos.execve({binary!r},[{binary!r},*sys.argv[1:]],env)\n")
                    wrapper.chmod(0o700)
                    driver = Driver(port)
                    try:
                        result = await asyncio.to_thread(driver.request, "POST", "/session", {"capabilities": {"alwaysMatch": {"tauri:options": {"application": str(wrapper)}}}})
                        driver.session = result["sessionId"]
                        await driver.wait("return document.readyState === 'complete';")
                        location = await driver.js("return location.href;")
                        assert location.startswith("http://127.0.0.1:1420/"), f"Expected development URL, got {location}"
                        await driver.wait("return document.querySelector('h1')?.textContent.includes('Proxy checker');")
                        await driver.ipc("snapshot", {"since": 0})
                        await driver.click("Settings")
                        await driver.wait("return Boolean(document.querySelector('dialog'));")
                        ui_proxy_requests = Trap.hits
                        assert ui_proxy_requests == 0, f"Unexpected UI proxy requests: {ui_proxy_requests}"
                        await driver.screenshot(f"proxy-env-{name}.png")
                        # NO_PROXY must bypass the local UI, but cannot bypass an
                        # explicitly selected proxy in the Rust checking engine.
                        await driver.ipc("preview_import", {"text": proxy, "options": {}})
                        await driver.ipc("commit_import", {"replace": False, "keepDuplicates": False, "includeInvalid": False})
                        snapshot = await driver.ipc("snapshot", {"since": 0})
                        settings = dict(url="http://127.0.0.1:1420/", fallbackUrl="", ipEcho=False, countryLookup=False, anonymityCheck=False, expectedStatus=200, bodyContains="ProxyVouch", concurrency=1, rateLimit=10, connectTimeoutMs=1000, attemptTimeoutMs=2000, totalTimeoutMs=5000, retries=0)
                        await driver.ipc("start_check", {"ids": [row["id"] for row in snapshot["rows"]], "settings": settings, "detectAgain": False})
                        for _ in range(100):
                            snapshot = await driver.ipc("snapshot", {"since": 0})
                            if not snapshot["running"]:
                                break
                            await asyncio.sleep(.05)
                        else:
                            raise AssertionError("The selected-proxy check did not finish")
                        result = snapshot["rows"][0]["result"]
                        assert result["code"] == "TARGET_HTTP_ERROR" and result["status"] == "Inconclusive", result
                        assert Trap.hits == 1, f"Expected exactly one explicitly proxied check, got {Trap.hits}"
                        results.append({"case": name, "dev_url": location, "ui_loaded": True, "ui_proxy_requests": ui_proxy_requests, "selected_proxy_requests": Trap.hits, "checker_status": result["status"]})
                        print(json.dumps(results[-1]), flush=True)
                    finally:
                        if driver.session:
                            with contextlib.suppress(Exception):
                                await driver.command("DELETE", "")
                (ROOT / "artifacts/proxy-environment-results.json").write_text(json.dumps(results, indent=2) + "\n")
            finally:
                for process in reversed(processes):
                    with contextlib.suppress(ProcessLookupError):
                        os.killpg(process.pid, signal.SIGTERM)
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=5)
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    asyncio.run(main())
