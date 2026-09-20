#!/usr/bin/env python3
"""Exercise the real Tauri WebView and Rust IPC through a local WebDriver."""
import asyncio
import base64
import json
import os
from pathlib import Path
import tempfile
import subprocess
import shutil
import sys
import urllib.error
import urllib.request
from network_fixtures import Fixtures, ROOT

ELEMENT = "element-6066-11e4-a52e-4f735466cecf"


class Driver:
    def __init__(self, port):
        self.port = port
        self.base = f"http://127.0.0.1:{port}"
        self.session = None
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(self, method, path, payload=None):
        request = urllib.request.Request(self.base + path, method=method,
            data=json.dumps(payload).encode() if payload is not None else None,
            headers={"Content-Type": "application/json"})
        try:
            with self.opener.open(request, timeout=40) as response:
                return json.load(response)["value"]
        except urllib.error.HTTPError as error:
            raise RuntimeError(error.read().decode()) from error

    async def command(self, method, path, payload=None):
        return await asyncio.to_thread(self.request, method, f"/session/{self.session}{path}", payload)

    async def create(self, binary=None):
        binary = binary or os.environ.get("PROXY_VOUCH_BINARY", str(ROOT / "target/debug/proxy-vouch"))
        result = await asyncio.to_thread(self.request, "POST", "/session", {"capabilities": {"alwaysMatch": {"tauri:options": {"application": binary}}}})
        self.session = result["sessionId"]

    async def js(self, script, args=None):
        return await self.command("POST", "/execute/sync", {"script": script, "args": args or []})

    async def ipc(self, command, args=None):
        result = await self.command("POST", "/execute/async", {"script": "const done=arguments[arguments.length-1];window.__TAURI_INTERNALS__.invoke(arguments[0],arguments[1]).then(result=>done({ok:true,result}),error=>done({ok:false,error}));", "args": [command, args or {}]})
        assert result["ok"], result
        return result.get("result")

    async def find(self, value, strategy="css selector"):
        return (await self.command("POST", "/element", {"using": strategy, "value": value}))[ELEMENT]

    async def click(self, text):
        element = await self.find(f"//button[normalize-space(.)={json.dumps(text)}]", "xpath")
        await self.command("POST", f"/element/{element}/click", {})

    async def fill(self, selector, text):
        # WebKit's native send-keys implementation can drop LF characters.
        # Dispatch the same input event as a paste, through React's public DOM boundary.
        await self.js("const el=document.querySelector(arguments[0]); const proto=el.tagName==='TEXTAREA'?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(proto,'value').set.call(el,arguments[1]);el.dispatchEvent(new Event('input',{bubbles:true}));", [selector, text])
        await asyncio.sleep(.1)

    async def wait(self, script, timeout=20):
        deadline = asyncio.get_running_loop().time() + timeout
        while asyncio.get_running_loop().time() < deadline:
            if await self.js(script):
                return
            await asyncio.sleep(.1)
        raise AssertionError(f"UI wait timed out: {script}")

    async def screenshot(self, name):
        data = await self.command("GET", "/screenshot")
        (ROOT / "artifacts" / name).write_bytes(base64.b64decode(data))


def isolated_application(directory):
    if sys.platform != "linux":
        raise RuntimeError("This WebKit smoke runner requires Linux and isolated XDG folders")
    directory = Path(directory)
    binary = os.environ.get("PROXY_VOUCH_BINARY", str(ROOT / "target/debug/proxy-vouch"))
    wrapper = directory / "launch-app"
    wrapper.write_text(f"#!{sys.executable}\nimport os,sys\nenv=dict(os.environ)\nenv['XDG_DATA_HOME']={str(directory / 'data')!r}\nenv['XDG_CONFIG_HOME']={str(directory / 'config')!r}\nos.execve({binary!r},[{binary!r},*sys.argv[1:]],env)\n")
    wrapper.chmod(0o700)
    return str(wrapper)


async def main():
    driver = Driver(int(os.environ.get("TAURI_DRIVER_PORT", "4457")))
    with tempfile.TemporaryDirectory(prefix="proxy-vouch-native-") as directory:
        fixtures = Fixtures(directory)
        fixtures.certificates()
        target = await fixtures.listen("target", fixtures.endpoint)
        http = await fixtures.listen("http", fixtures.http_proxy())
        auth = await fixtures.listen("auth", fixtures.http_proxy(authentication=True))
        socks = await fixtures.listen("socks", fixtures.socks(5))
        try:
            await driver.create(isolated_application(directory))
            await driver.wait("return Boolean(document.querySelector('h1'));")
            assert await driver.js("return window.isTauri;")
            await driver.click("Settings")
            await driver.fill(".modal .full-label input", f"http://127.0.0.1:{target}/")
            await driver.js("const select=[...document.querySelectorAll('.modal select')].find(e=>e.querySelector('option[value=light]')); select.value='light';select.dispatchEvent(new Event('change',{bubbles:true}));")
            await driver.js("const checkbox=[...document.querySelectorAll('.checkbox-label')].find(el=>el.textContent.includes('Detect proxy country')).querySelector('input'); if(checkbox.checked) checkbox.click();")
            await driver.js("const checkbox=[...document.querySelectorAll('.checkbox-label')].find(el=>el.textContent.includes('Check proxy anonymity')).querySelector('input'); if(checkbox.checked) checkbox.click();")
            await driver.click("Save settings")
            await driver.wait("return !document.querySelector('dialog');")
            await driver.click("Add proxies")
            text = "\n".join([f"http://127.0.0.1:{http}", f"socks5h://127.0.0.1:{socks}", f"127.0.0.1:{socks}", f"http://demo:wrong@127.0.0.1:{auth}", f"http://demo:fixture-secret@127.0.0.1:{auth}", "bad-record", f"http://127.0.0.1:{http}"])
            await driver.fill("textarea[aria-label='Proxy list input']", text)
            await driver.click("Preview import")
            await driver.wait("return Boolean(document.querySelector('.preview-summary'));")
            summary = await driver.js("return document.querySelector('.preview-summary').textContent;")
            assert "6 valid" in summary and "1 invalid" in summary and "1 duplicates" in summary, summary
            assert not await driver.js("return document.querySelector('.preview').textContent.includes('fixture-secret');")
            await driver.click("Import 7")
            await driver.wait("return !document.querySelector('dialog') && document.querySelectorAll('.proxy-row').length===6;")
            await driver.click("Check all")
            await driver.wait("return document.querySelectorAll('.status-working').length===4 && document.querySelectorAll('.status-failed').length===1;", timeout=20)
            snapshot = await driver.ipc("snapshot", {"since": 0})
            assert snapshot["counts"] == {"Working": 4, "Failed": 1, "Invalid": 1}, snapshot["counts"]
            assert not snapshot["running"]
            await driver.screenshot("native-results.png")
            await driver.js("const rows=[...document.querySelectorAll('.proxy-row')]; for(const index of [0,1,4]) rows[index].querySelector('input[type=checkbox]').click();")
            await driver.fill("input[aria-label='Search proxies']", str(http))
            await driver.wait("return document.querySelectorAll('.proxy-row').length===1;")
            await driver.click("Copy selected (3)")
            await driver.wait("return document.querySelector('.toast')?.textContent.includes('3 records copied');")
            selected_clipboard = await driver.ipc("read_clipboard")
            assert len(selected_clipboard.strip().splitlines()) == 3
            assert "socks5h://" in selected_clipboard and "fixture%2Dsecret" in selected_clipboard
            assert "wrong" not in selected_clipboard
            await driver.fill("input[aria-label='Search proxies']", "")
            await driver.wait("return document.querySelectorAll('.proxy-row').length===6;")
            await driver.js("for(const checkbox of document.querySelectorAll('.proxy-row input:checked')) checkbox.click();")
            await driver.click("Copy working")
            await driver.wait("return Boolean(document.querySelector('.toast'));")
            clipboard = await driver.ipc("read_clipboard")
            assert len(clipboard.strip().splitlines()) == 4 and "wrong" not in clipboard and "socks5h://" in clipboard
            await driver.click("Copy failed")
            await asyncio.sleep(.2)
            clipboard = await driver.ipc("read_clipboard")
            assert clipboard.strip() == f"http://demo:wrong@127.0.0.1:{auth}"
            await driver.click("More")
            await driver.js("const selects=document.querySelectorAll('.modal select');selects[0].value='Checked';selects[0].dispatchEvent(new Event('change',{bubbles:true}));")
            await driver.js("const select=document.querySelectorAll('.modal select')[1];select.value='json';select.dispatchEvent(new Event('change',{bubbles:true}));")
            await driver.click("Copy to clipboard")
            await driver.wait("return !document.querySelector('dialog');")
            report = json.loads(await driver.ipc("read_clipboard"))
            assert len(report["records"]) == 5
            assert "fixture-secret" not in json.dumps(report)
            file_dialogs = False
            local_xdotool = ROOT / "artifacts/xdotool/extracted/usr/bin/xdotool"
            xdotool = str(local_xdotool) if local_xdotool.exists() else shutil.which("xdotool")
            environment = None
            xclip = shutil.which("xclip")
            if xdotool and xclip and Path("/proc").exists():
                for process in Path("/proc").iterdir():
                    if not process.name.isdigit():
                        continue
                    try:
                        command = (process / "cmdline").read_bytes().split(b"\0")
                        if command and command[0].endswith(b"/tauri-driver") and str(driver.port).encode() in command:
                            values = dict(item.split(b"=", 1) for item in (process / "environ").read_bytes().split(b"\0") if b"=" in item)
                            environment = dict(os.environ)
                            for key in ("DISPLAY", "XAUTHORITY"):
                                if key.encode() in values:
                                    environment[key] = values[key.encode()].decode()
                            if local_xdotool.exists():
                                environment["LD_LIBRARY_PATH"] = str(ROOT / "artifacts/xdotool/extracted/usr/lib/x86_64-linux-gnu")
                            break
                    except (PermissionError, FileNotFoundError, ProcessLookupError):
                        continue
            if environment is not None:
                output_path = Path(directory) / "native proxy results.txt"
                async def choose_path(path):
                    await asyncio.sleep(.8)
                    # Paste the complete path: GTK completion can rewrite a path
                    # while individual characters are injected through X11.
                    await asyncio.to_thread(subprocess.run, [xclip, "-selection", "clipboard"], input=str(path).encode(), env=environment, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    for args in [["key", "--clearmodifiers", "ctrl+l"], ["key", "--clearmodifiers", "ctrl+a"], ["key", "--clearmodifiers", "ctrl+v"], ["key", "--clearmodifiers", "Return"]]:
                        await asyncio.to_thread(subprocess.run, [str(xdotool), *args], env=environment, check=True, capture_output=True)
                        await asyncio.sleep(.3)
                await driver.click("Save working")
                await choose_path(output_path)
                for _ in range(40):
                    if output_path.exists():
                        break
                    await asyncio.sleep(.1)
                if not output_path.exists():
                    await asyncio.to_thread(subprocess.run, ["import", "-window", "root", str(ROOT / "artifacts/native-file-dialog.png")], env=environment, capture_output=True)
                assert output_path.exists(), "Native save dialog did not produce the selected file"
                saved = output_path.read_text()
                assert len(saved.strip().splitlines()) == 4 and "wrong" not in saved
                await driver.wait("return [...document.querySelectorAll('button')].some(e=>e.textContent.trim()==='Import file' && !e.disabled);")
                await driver.click("Import file")
                await choose_path(output_path)
                await asyncio.sleep(.5)
                await asyncio.to_thread(subprocess.run, ["import", "-window", "root", str(ROOT / "artifacts/native-open-dialog.png")], env=environment, capture_output=True)
                await driver.wait("return Boolean(document.querySelector('textarea[aria-label=\"Proxy list input\"]')); ")
                assert await driver.js("return document.querySelector('textarea').value; ") == saved
                await driver.click("Preview import")
                await driver.wait("return Boolean(document.querySelector('.preview-summary')); ")
                assert "4 valid" in await driver.js("return document.querySelector('.preview-summary').textContent;")
                await driver.js("document.querySelector('button[aria-label=\"Close dialog\"]').click();")
                file_dialogs = True
            await driver.fill("input[aria-label='Search proxies']", str(socks))
            await driver.wait("return document.querySelectorAll('.proxy-row').length===2;")
            await driver.fill("input[aria-label='Search proxies']", "")
            await driver.wait("return document.querySelectorAll('.proxy-row').length===6;")
            # Use actual IPC for a controlled hanging run, then the visible Stop button.
            settings = dict(url=f"http://127.0.0.1:{target}/slow",fallbackUrl="",ipEcho=True,countryLookup=False,anonymityCheck=False,expectedStatus=200,bodyContains="",concurrency=2,rateLimit=100,connectTimeoutMs=1000,attemptTimeoutMs=8000,totalTimeoutMs=15000,retries=0)
            await driver.ipc("start_check", {"ids":[row["id"] for row in snapshot["rows"] if row["status"] == "Working"],"settings":settings,"detectAgain":False})
            await driver.wait("return [...document.querySelectorAll('button')].some(e=>e.textContent.trim()==='Stop checking');")
            start = asyncio.get_running_loop().time()
            await driver.click("Stop checking")
            await driver.wait("return ![...document.querySelectorAll('button')].some(e=>e.textContent.trim()==='Stop checking');")
            elapsed = asyncio.get_running_loop().time() - start
            assert elapsed < 1, elapsed
            stopped = await driver.ipc("snapshot", {"since": 0})
            assert stopped["counts"].get("Cancelled") == 4, stopped["counts"]
            output = {"native_ui":True,"initial_counts":snapshot["counts"],"clipboard_groups_verified":True,"selected_clipboard_verified":True,"json_report_verified":True,"native_file_dialogs_verified":file_dialogs,"search_verified":True,"cancellation_seconds":round(elapsed,3),"fixture_connections":fixtures.connections}
            (ROOT / "artifacts/native-results.json").write_text(json.dumps(output,indent=2)+"\n")
            print(json.dumps(output,indent=2))
        finally:
            if driver.session:
                with contextlib.suppress(Exception):
                    await driver.command("DELETE", "")
            await fixtures.close()


if __name__ == "__main__":
    import contextlib
    asyncio.run(main())
