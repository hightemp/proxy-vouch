#!/usr/bin/env python3
"""Native Linux persistence and backup acceptance, using isolated XDG directories."""
import asyncio
import contextlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile

from native_smoke import Driver, isolated_application
from network_fixtures import Fixtures, ROOT
from proxy_environment_smoke import free_port, reachable


async def main():
    environment = {key: value for key, value in os.environ.items() if key.lower() not in ("http_proxy", "https_proxy", "all_proxy", "no_proxy")}
    driver_path = shutil.which("tauri-driver") or str(ROOT / "artifacts/tools/bin/tauri-driver")
    webkit_path = shutil.which("WebKitWebDriver") or str(ROOT / "artifacts/webdriver/extracted/usr/bin/WebKitWebDriver")
    xdotool = shutil.which("xdotool") or str(ROOT / "artifacts/xdotool/extracted/usr/bin/xdotool")
    xclip = shutil.which("xclip")
    if not xclip or not Path(xdotool).is_file():
        raise RuntimeError("xdotool and xclip are required to test real backup file dialogs")
    with tempfile.TemporaryDirectory(prefix="proxy-vouch-storage-") as directory:
        directory = Path(directory)
        fixtures = Fixtures(directory)
        target = await fixtures.listen("target", fixtures.endpoint)
        proxy = await fixtures.listen("proxy", fixtures.http_proxy())
        wrapper = isolated_application(directory)
        port, native_port = free_port(), free_port()
        driver = Driver(port)
        log_path = ROOT / "artifacts/storage-smoke.log"
        with log_path.open("w") as log:
            process = subprocess.Popen(["xvfb-run", "-a", "-s", "-screen 0 1440x1000x24", driver_path, "--native-driver", webkit_path, "--port", str(port), "--native-port", str(native_port)], cwd=ROOT, env=environment, stdout=log, stderr=log, start_new_session=True)
            try:
                for _ in range(100):
                    if reachable(f"http://127.0.0.1:{port}/status"):
                        break
                    await asyncio.sleep(.1)
                else:
                    raise RuntimeError("WebDriver did not start")
                display_environment = dict(environment)
                for proc in Path("/proc").iterdir():
                    if not proc.name.isdigit():
                        continue
                    try:
                        args = (proc / "cmdline").read_bytes().split(b"\0")
                        if args and args[0].endswith(b"/tauri-driver") and str(port).encode() in args:
                            variables = dict(item.split(b"=", 1) for item in (proc / "environ").read_bytes().split(b"\0") if b"=" in item)
                            for key in ("DISPLAY", "XAUTHORITY"):
                                if key.encode() in variables:
                                    display_environment[key] = variables[key.encode()].decode()
                            break
                    except (PermissionError, FileNotFoundError, ProcessLookupError):
                        continue
                if "DISPLAY" not in display_environment:
                    raise RuntimeError("Could not locate the isolated test display")
                if "/artifacts/" in xdotool:
                    display_environment["LD_LIBRARY_PATH"] = str(ROOT / "artifacts/xdotool/extracted/usr/lib/x86_64-linux-gnu")

                async def start():
                    await driver.create(wrapper)
                    await driver.wait("return document.querySelector('h1') && [...document.querySelectorAll('button')].some(e=>e.textContent.trim()==='Settings' && !e.disabled);")

                async def choose_path(path):
                    await asyncio.sleep(.5)
                    await asyncio.to_thread(subprocess.run, [xclip, "-selection", "clipboard"], input=str(path).encode(), env=display_environment, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
                    for key in ("ctrl+l", "ctrl+a", "ctrl+v", "Return"):
                        await asyncio.to_thread(subprocess.run, [xdotool, "key", "--clearmodifiers", key], env=display_environment, check=True, capture_output=True)
                        await asyncio.sleep(.25)

                async def select(label, value):
                    await driver.js("const label=[...document.querySelectorAll('dialog label')].find(e=>e.textContent.trim().startsWith(arguments[0]));const el=label.querySelector('select');el.value=arguments[1];el.dispatchEvent(new Event('change',{bubbles:true}));", [label, value])

                async def close_dialog():
                    await driver.js("document.querySelector('dialog button[aria-label=\"Close dialog\"]').click();")

                async def wait_saved():
                    for _ in range(100):
                        status = await driver.ipc("storage_status")
                        snapshot = await driver.ipc("snapshot", {"since": 0})
                        if status["error"] is None and status["savedRevision"] is not None and status["savedRevision"] >= snapshot["revision"]:
                            return
                        await asyncio.sleep(.1)
                    raise AssertionError("Workspace was not saved automatically")

                async def close_app():
                    await driver.js("window.__TAURI_INTERNALS__.invoke('plugin:window|close',{label:'main'}).catch(()=>{});return true;")
                    await asyncio.sleep(.5)
                    with contextlib.suppress(Exception):
                        await driver.command("DELETE", "")
                    driver.session = None

                async def import_backup(path):
                    await driver.click("Choose backup file")
                    await choose_path(path)
                    await driver.wait("return Boolean(document.querySelector('.backup-preview'));")

                await start()
                await driver.click("Settings")
                check_url = f"http://127.0.0.1:{target}/?token=fixture-url-token"
                await driver.fill(".modal .full-label input", check_url)
                await select("Appearance", "dark")
                await driver.js("const checkbox=[...document.querySelectorAll('.checkbox-label')].find(el=>el.textContent.includes('Check proxy anonymity')).querySelector('input'); if(checkbox.checked) checkbox.click();")
                await driver.click("Save settings")
                await driver.wait("return !document.querySelector('dialog');")
                preferences = await driver.ipc("load_preferences")
                text = f"http://demo:fixture-password@127.0.0.1:{proxy}\n[2001:db8::1]:1080\ninvalid-record"
                await driver.click("Add proxies")
                await driver.fill("textarea[aria-label='Proxy list input']", text)
                await driver.click("Preview import")
                await driver.wait("return Boolean(document.querySelector('.preview-summary'));")
                await driver.click("Import 3")
                await driver.wait("return !document.querySelector('dialog') && document.querySelectorAll('.proxy-row').length===3;")
                snapshot = await driver.ipc("snapshot", {"since": 0})
                await driver.ipc("start_check", {"ids": [snapshot["rows"][0]["id"]], "settings": preferences["check"], "detectAgain": False})
                await driver.wait("return document.querySelectorAll('.status-working').length===1;")
                await wait_saved()
                state_dir = Path((await driver.ipc("storage_status"))["directory"])
                assert state_dir.is_relative_to(directory), "The test must not use personal data"
                state_path = state_dir / "workspace.json"
                assert state_path.is_file()
                disk = json.loads(state_path.read_text())
                assert disk["entries"][0]["parsed"]["proxy"]["credentials"]["password"] == "fixture-password"
                assert disk["preferences"] == preferences
                await close_app()
                await start()
                await driver.wait("return document.querySelectorAll('.proxy-row').length===3 && document.querySelectorAll('.status-working').length===1;")
                assert await driver.ipc("load_preferences") == preferences
                assert await driver.js("return document.documentElement.dataset.theme;") == "dark"
                snapshot = await driver.ipc("snapshot", {"since": 0})
                assert "fixture-password" not in json.dumps(snapshot)
                assert "fixture-url-token" not in json.dumps(snapshot)
                assert await driver.ipc("reveal_entry", {"id": snapshot["rows"][0]["id"]}) == text.splitlines()[0]
                print("PASS native restart: proxies, credentials, settings, theme and result restored", flush=True)

                await driver.click("Backup & restore")
                await driver.command("POST", "/window/rect", {"width": 1000, "height": 650})
                await driver.screenshot("native-storage-backup.png")
                bounds = await driver.js("const d=document.querySelector('dialog');return {bottom:d.getBoundingClientRect().bottom,height:innerHeight,width:d.scrollWidth,visible:d.clientWidth};")
                assert bounds["bottom"] <= bounds["height"] and bounds["width"] <= bounds["visible"], bounds
                await driver.command("POST", "/window/rect", {"width": 1320, "height": 860})
                for scope in ("full", "proxies", "settings"):
                    await select("Include in backup", scope)
                    await driver.click("Export backup")
                    await choose_path(directory / f"{scope}.json")
                    for _ in range(50):
                        if (directory / f"{scope}.json").exists():
                            break
                        await asyncio.sleep(.1)
                    exported = json.loads((directory / f"{scope}.json").read_text())
                    assert exported["format"] == "proxy-pulse-backup"
                    assert (exported["entries"] is not None) == (scope != "settings")
                    assert (exported["preferences"] is not None) == (scope != "proxies")
                print("PASS native backup dialogs: full, proxies and settings exported", flush=True)

                await close_dialog()
                await driver.click("Settings")
                await driver.fill(".modal .full-label input", f"http://127.0.0.1:{target}/different")
                await select("Appearance", "light")
                await driver.click("Save settings")
                await driver.wait("return !document.querySelector('dialog') && document.documentElement.dataset.theme==='light';")
                await driver.click("Backup & restore")
                await driver.ipc("clear_entries", {"ids": []})
                await wait_saved()
                await import_backup(directory / "settings.json")
                await driver.click("Import backup")
                await driver.wait("return !document.querySelector('.backup-preview');")
                assert (await driver.ipc("snapshot", {"since": 0}))["total"] == 0
                assert await driver.ipc("load_preferences") == preferences
                assert await driver.js("return document.documentElement.dataset.theme;") == "dark"
                await import_backup(directory / "proxies.json")
                await driver.click("Import backup")
                await driver.wait("return !document.querySelector('.backup-preview');")
                assert (await driver.ipc("snapshot", {"since": 0}))["total"] == 3
                await import_backup(directory / "full.json")
                await driver.click("Import backup")
                await driver.wait("return !document.querySelector('.backup-preview');")
                assert (await driver.ipc("snapshot", {"since": 0}))["total"] == 3
                await import_backup(directory / "full.json")
                await select("Proxy list", "replace")
                await driver.click("Replace list and import")
                await driver.wait("return !document.querySelector('.backup-preview');")
                assert (await driver.ipc("snapshot", {"since": 0}))["total"] == 3
                assert await driver.ipc("load_preferences") == preferences
                (directory / "invalid.json").write_text('{"version":99}')
                await driver.click("Choose backup file")
                await choose_path(directory / "invalid.json")
                await driver.wait("return document.querySelector('dialog .inline-error')?.textContent.includes('valid ProxyVouch backup');")
                assert (await driver.ipc("snapshot", {"since": 0}))["total"] == 3
                await close_dialog()
                print("PASS native restore: selective import, merge, replace and invalid-file rejection", flush=True)

                await wait_saved()
                previous_path = state_dir / "workspace.previous.json"
                previous_path.unlink()
                previous_path.mkdir()
                before_error = state_path.read_bytes()
                snapshot = await driver.ipc("snapshot", {"since": 0})
                await driver.ipc("edit_entry", {"id": snapshot["rows"][1]["id"], "text": "changed.example:1080"})
                await driver.wait("return document.querySelector('.session-label')?.textContent==='Not saved';")
                assert state_path.read_bytes() == before_error
                previous_path.rmdir()
                await wait_saved()
                print("PASS native autosave failure: visible error, previous file preserved, retry successful", flush=True)

                await close_app()
                await start()
                await driver.wait("return document.querySelectorAll('.proxy-row').length===3;")
                snapshot = await driver.ipc("snapshot", {"since": 0})
                assert snapshot["rows"][1]["host"] == "changed.example"
                first_checkbox = await driver.find(f"input[aria-label={json.dumps('Select ' + snapshot['rows'][0]['address'])}]")
                await driver.command("POST", f"/element/{first_checkbox}/click", {})
                slow = dict(preferences["check"], url=f"http://127.0.0.1:{target}/slow")
                await driver.ipc("start_check", {"ids": [snapshot["rows"][0]["id"]], "settings": slow, "detectAgain": False})
                await driver.wait("return Boolean(document.querySelector('.status-checking'));")
                assert await driver.js("return [...document.querySelectorAll('button')].find(e=>e.textContent.trim()==='Remove selected (1)')?.disabled;")
                await driver.js("window.__TAURI_INTERNALS__.invoke('plugin:window|close',{label:'main'}).catch(()=>{});return true;")
                await driver.wait("return document.querySelector('dialog h2')?.textContent==='Stop checking and quit?';")
                await driver.click("Stop, save and quit")
                await asyncio.sleep(.5)
                with contextlib.suppress(Exception):
                    await driver.command("DELETE", "")
                driver.session = None
                await start()
                await driver.wait("return document.querySelectorAll('.status-cancelled').length===1;")
                assert not (await driver.ipc("snapshot", {"since": 0}))["running"]
                print("PASS native close: flush on exit and interrupted check restored as Cancelled", flush=True)

                # Select a completed/cancelled row and an invalid row, leaving one
                # record unselected. Filtering must not change the removal scope.
                snapshot = await driver.ipc("snapshot", {"since": 0})
                for row in (snapshot["rows"][0], snapshot["rows"][2]):
                    checkbox = await driver.find(f"input[aria-label={json.dumps('Select ' + row['address'])}]")
                    await driver.command("POST", f"/element/{checkbox}/click", {})
                await driver.command("POST", "/window/rect", {"width": 1000, "height": 650})
                await driver.wait("return [...document.querySelectorAll('button')].some(e=>e.textContent.trim()==='Remove selected (2)' && !e.disabled);")
                assert await driver.js("return document.documentElement.scrollWidth <= innerWidth;")
                await driver.screenshot("native-remove-selected.png")
                await driver.fill("input[aria-label='Search proxies']", "changed.example")
                await driver.wait("return document.querySelectorAll('.proxy-row').length===1;")
                await driver.click("Remove selected (2)")
                await driver.wait("return document.querySelector('dialog h2')?.textContent==='Remove selected proxies?';")
                assert await driver.js("return document.querySelector('dialog').textContent.includes('2 records will be removed.') && document.querySelector('dialog').textContent.includes('hidden by the current filter');")
                await driver.click("Cancel")
                assert (await driver.ipc("snapshot", {"since": 0}))["total"] == 3
                await driver.click("Remove selected (2)")
                await driver.click("Remove selected")
                await driver.wait("return !document.querySelector('dialog') && ![...document.querySelectorAll('button')].some(e=>e.textContent.includes('Remove selected'));")
                await driver.fill("input[aria-label='Search proxies']", "")
                remaining = await driver.ipc("snapshot", {"since": 0})
                assert remaining["total"] == 1 and remaining["rows"][0]["host"] == "changed.example"
                await wait_saved()
                await close_app()
                await start()
                await driver.wait("return document.querySelectorAll('.proxy-row').length===1;")
                remaining = await driver.ipc("snapshot", {"since": 0})
                assert remaining["total"] == 1 and remaining["rows"][0]["host"] == "changed.example"
                await close_app()
                print("PASS native selected removal: blocked during checks, confirmation/cancel, hidden selections, remaining row and restart", flush=True)
                (ROOT / "artifacts/storage-results.json").write_text(json.dumps({"native_restart": True, "native_backup_export": ["full", "proxies", "settings"], "native_restore": ["settings", "proxies", "merge", "replace"], "invalid_backup_rejected": True, "save_failure_recovery": True, "close_during_run": True, "selected_removal": True, "selected_removal_restart": True, "isolated_user_data": True}, indent=2) + "\n")
            finally:
                if driver.session:
                    with contextlib.suppress(Exception):
                        await driver.command("DELETE", "")
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGTERM)
                await asyncio.to_thread(process.wait, timeout=10)
                await fixtures.close()


if __name__ == "__main__":
    asyncio.run(main())
