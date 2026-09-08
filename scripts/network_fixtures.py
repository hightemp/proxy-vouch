#!/usr/bin/env python3
"""Offline proxy protocol acceptance. All credentials and addresses are synthetic."""
import asyncio
import base64
import contextlib
import json
import os
from pathlib import Path
import socket
import ssl
import struct
import subprocess
import tempfile
import time
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
DEFAULT = dict(url="", fallbackUrl="", ipEcho=True, expectedStatus=200,
               bodyContains="", concurrency=4, rateLimit=100, connectTimeoutMs=1000,
               attemptTimeoutMs=2000, totalTimeoutMs=15000, retries=0)


class Fixtures:
    def __init__(self, directory):
        self.directory = Path(directory)
        self.servers = []
        self.connections = {}
        self.destinations = []
        self.tasks = set()
        self.writers = set()

    async def listen(self, name, handler, tls=False, host="127.0.0.1"):
        async def tracked(reader, writer):
            task = asyncio.current_task()
            self.tasks.add(task)
            self.writers.add(writer)
            self.connections[name] = self.connections.get(name, 0) + 1
            try:
                await asyncio.wait_for(handler(reader, writer), timeout=20)
            except (asyncio.IncompleteReadError, ConnectionError, TimeoutError, OSError, ValueError):
                pass
            finally:
                writer.close()
                with contextlib.suppress(Exception):
                    await writer.wait_closed()
                self.writers.discard(writer)
                self.tasks.discard(task)
        server = await asyncio.start_server(tracked, host, 0, ssl=self.ssl_context if tls else None)
        self.servers.append(server)
        return server.sockets[0].getsockname()[1]

    def certificates(self):
        config = self.directory / "openssl.cnf"
        config.write_text("[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=localhost\n[ext]\nsubjectAltName=DNS:localhost,DNS:remote-only.invalid,IP:127.0.0.1,IP:::1\nbasicConstraints=critical,CA:TRUE\nkeyUsage=digitalSignature,keyEncipherment,keyCertSign\n")
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-keyout", str(self.directory / "key.pem"), "-out", str(self.directory / "ca.pem"), "-config", str(config)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.ssl_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self.ssl_context.load_cert_chain(self.directory / "ca.pem", self.directory / "key.pem")

    async def endpoint(self, reader, writer):
        request = await reader.readuntil(b"\r\n\r\n")
        path = request.split(b" ")[1].decode()
        status = 200
        body = b'{"ip":"198.51.100.99"}'
        if path.startswith("/status/"):
            status = int(path.split("/")[2])
        elif path == "/invalid":
            body = b"not an IP echo response"
        elif path == "/large":
            body = b"x" * 70000
        elif path == "/slow":
            await asyncio.sleep(10)
        writer.write(f"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body)
        await writer.drain()

    async def silent(self, reader, writer):
        # Accept TCP and consume the greeting, but never answer any protocol.
        # Exit on client close so timeout/cancellation does not leave handlers alive.
        while await reader.read(4096):
            pass

    async def relay(self, reader, writer, upstream_reader, upstream_writer):
        async def pump(source, target):
            try:
                while data := await source.read(65536):
                    target.write(data)
                    await target.drain()
            except (ConnectionError, OSError):
                pass
        pumps = [asyncio.create_task(pump(reader, upstream_writer)), asyncio.create_task(pump(upstream_reader, writer))]
        try:
            await asyncio.wait(pumps, return_when=asyncio.FIRST_COMPLETED)
        finally:
            for task in pumps:
                task.cancel()
            await asyncio.gather(*pumps, return_exceptions=True)
            upstream_writer.close()
            with contextlib.suppress(Exception):
                await upstream_writer.wait_closed()

    async def connect(self, host, port):
        self.destinations.append(host)
        if host in ("remote-only.invalid", "wrong-name.invalid"):
            host = "127.0.0.1"
        if host not in ("127.0.0.1", "localhost", "::1"):
            raise ValueError("Fixture destinations must stay on loopback")
        return await asyncio.open_connection(host, port)

    def http_proxy(self, authentication=False, deny=False, digest=False):
        async def handler(reader, writer):
            request = await reader.readuntil(b"\r\n\r\n")
            if len(request) > 32768:
                return
            first, *headers = request.decode("latin1").split("\r\n")
            method, target, _ = first.split(" ", 2)
            expected = "Proxy-Authorization: Basic " + base64.b64encode(b"demo:fixture-secret").decode()
            if digest or (authentication and expected.lower() not in [h.lower() for h in headers]):
                challenge = 'Digest realm="fixture"' if digest else 'Basic realm="fixture"'
                writer.write(f"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: {challenge}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".encode())
                await writer.drain()
                return
            if deny:
                writer.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                await writer.drain()
                return
            if method == "CONNECT":
                host, port = target.rsplit(":", 1)
                upstream_reader, upstream_writer = await self.connect(host.strip("[]"), int(port))
                writer.write(b"HTTP/1.1 200 Connection established\r\n\r\n")
                await writer.drain()
            else:
                parsed = urlsplit(target)
                upstream_reader, upstream_writer = await self.connect(parsed.hostname, parsed.port or 80)
                path = parsed.path or "/"
                if parsed.query:
                    path += "?" + parsed.query
                upstream_writer.write(f"{method} {path} HTTP/1.1\r\nHost: {parsed.hostname}\r\nConnection: close\r\n\r\n".encode())
                await upstream_writer.drain()
            await self.relay(reader, writer, upstream_reader, upstream_writer)
        return handler

    def socks(self, version, authentication=False, unsupported=False, deny=False):
        async def handler(reader, writer):
            received_version = (await reader.readexactly(1))[0]
            if received_version != version:
                return
            if version == 5:
                count = (await reader.readexactly(1))[0]
                methods = await reader.readexactly(count)
                assert 1 not in methods, "GSSAPI must not be offered by this client"
                chosen = 0xFF if unsupported else (2 if authentication else 0)
                writer.write(bytes([5, chosen]))
                await writer.drain()
                if chosen == 0xFF:
                    return
                if authentication:
                    auth_version, length = await reader.readexactly(2)
                    username = await reader.readexactly(length)
                    length = (await reader.readexactly(1))[0]
                    password = await reader.readexactly(length)
                    accepted = auth_version == 1 and username == b"demo" and password == b"fixture-secret"
                    writer.write(bytes([1, 0 if accepted else 1]))
                    await writer.drain()
                    if not accepted:
                        return
                version_byte, command, reserved, kind = await reader.readexactly(4)
                if kind == 1:
                    host = socket.inet_ntop(socket.AF_INET, await reader.readexactly(4))
                elif kind == 4:
                    host = socket.inet_ntop(socket.AF_INET6, await reader.readexactly(16))
                else:
                    length = (await reader.readexactly(1))[0]
                    host = (await reader.readexactly(length)).decode()
                port = struct.unpack("!H", await reader.readexactly(2))[0]
                if deny:
                    writer.write(b"\x05\x02\x00\x01" + b"\x00" * 6)
                    await writer.drain()
                    return
                upstream_reader, upstream_writer = await self.connect(host, port)
                writer.write(b"\x05\x00\x00\x01" + b"\x00" * 6)
            else:
                command = await reader.readexactly(1)
                port = struct.unpack("!H", await reader.readexactly(2))[0]
                address = await reader.readexactly(4)
                user = await reader.readuntil(b"\x00")
                if address[:3] == b"\0\0\0" and address[3] != 0:
                    host = (await reader.readuntil(b"\0"))[:-1].decode()
                else:
                    host = socket.inet_ntop(socket.AF_INET, address)
                if deny:
                    writer.write(b"\0\x5b" + b"\0" * 6)
                    await writer.drain()
                    return
                upstream_reader, upstream_writer = await self.connect(host, port)
                writer.write(b"\0\x5a" + b"\0" * 6)
            await writer.drain()
            await self.relay(reader, writer, upstream_reader, upstream_writer)
        return handler

    async def close(self):
        for server in self.servers:
            server.close()
        await asyncio.gather(*(s.wait_closed() for s in self.servers))
        for writer in list(self.writers):
            writer.close()
        for task in list(self.tasks):
            task.cancel()
        await asyncio.gather(*list(self.tasks), return_exceptions=True)


async def acceptance():
    checks = []
    with tempfile.TemporaryDirectory(prefix="proxy-vouch-fixtures-") as directory:
        fixtures = Fixtures(directory)
        fixtures.certificates()
        http_target = await fixtures.listen("http_target", fixtures.endpoint)
        https_target = await fixtures.listen("https_target", fixtures.endpoint, tls=True)
        http = await fixtures.listen("http", fixtures.http_proxy())
        https = await fixtures.listen("https", fixtures.http_proxy(), tls=True)
        auth_http = await fixtures.listen("auth_http", fixtures.http_proxy(authentication=True))
        socks4 = await fixtures.listen("socks4", fixtures.socks(4))
        socks5 = await fixtures.listen("socks5", fixtures.socks(5))
        auth_socks5 = await fixtures.listen("auth_socks5", fixtures.socks(5, authentication=True))
        unsupported = await fixtures.listen("unsupported", fixtures.socks(5, unsupported=True))
        digest = await fixtures.listen("digest", fixtures.http_proxy(digest=True))
        socks_denied = await fixtures.listen("socks_denied", fixtures.socks(5, deny=True))
        deny = await fixtures.listen("deny", fixtures.http_proxy(deny=True))
        trap = await fixtures.listen("trap", fixtures.http_proxy())
        silent = await fixtures.listen("silent", fixtures.silent)
        closed_proxy = socket.socket()
        closed_proxy.bind(("127.0.0.1", 0))
        closed_port = closed_proxy.getsockname()[1]
        env = dict(os.environ, HTTP_PROXY=f"http://127.0.0.1:{trap}", HTTPS_PROXY=f"http://127.0.0.1:{trap}", ALL_PROXY=f"http://127.0.0.1:{trap}", NO_PROXY="*", http_proxy=f"http://127.0.0.1:{trap}", https_proxy=f"http://127.0.0.1:{trap}", all_proxy=f"http://127.0.0.1:{trap}", no_proxy="*")

        async def run(name, proxy, url=None, expected="Working", code=None, trust=True, extra=None, stage=None):
            settings = dict(DEFAULT, url=url or f"https://127.0.0.1:{https_target}/")
            settings.update(extra or {})
            process = await asyncio.create_subprocess_exec(str(ROOT / "target/debug/examples/check"), stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env)
            payload = dict(proxies=[proxy], settings=settings, ca_file=str(Path(directory)/"ca.pem") if trust else None)
            stdout, stderr = await asyncio.wait_for(process.communicate(json.dumps(payload).encode()), timeout=25)
            assert process.returncode == 0, (name, stderr.decode())
            result = json.loads(stdout)[0]
            assert result["status"] == expected, (name, result)
            if code:
                assert result["code"] == code, (name, result)
            if stage:
                assert result["stage"] == stage, (name, result)
            assert b"fixture-secret" not in stdout + stderr, name
            checks.append(dict(name=name, status=result["status"], code=result["code"], duration_ms=result["totalDurationMs"]))
            print(f"PASS {name}: {result['status']} {result['code']}", flush=True)
            return result

        try:
            for scheme, port in [("http", http), ("https", https), ("socks4", socks4), ("socks4a", socks4), ("socks5", socks5), ("socks5h", socks5)]:
                await run(f"explicit {scheme}", f"{scheme}://127.0.0.1:{port}")
            for name, port in [("HTTP", http), ("HTTPS", https), ("SOCKS4a", socks4), ("SOCKS5", socks5)]:
                await run(f"Auto {name}", f"127.0.0.1:{port}")
            await run("HTTP Basic", f"http://demo:fixture-secret@127.0.0.1:{auth_http}")
            await run("SOCKS5 password", f"socks5h://demo:fixture-secret@127.0.0.1:{auth_socks5}")
            await run("HTTP missing credentials", f"http://127.0.0.1:{auth_http}", expected="Failed", code="AUTH_REQUIRED")
            await run("HTTP wrong password", f"http://demo:wrong@127.0.0.1:{auth_http}", expected="Failed", code="AUTH_FAILED")
            await run("SOCKS5 wrong password", f"socks5h://demo:wrong@127.0.0.1:{auth_socks5}", expected="Failed", code="AUTH_FAILED")
            await run("unsupported SOCKS5 auth", f"socks5://127.0.0.1:{unsupported}", expected="Inconclusive", code="AUTH_METHOD_UNSUPPORTED")
            await run("unsupported HTTP Digest", f"http://127.0.0.1:{digest}", expected="Inconclusive", code="AUTH_METHOD_UNSUPPORTED")
            await run("SOCKS5 destination denied", f"socks5h://127.0.0.1:{socks_denied}", expected="Failed", code="SOCKS_REQUEST_REJECTED")
            await run("SOCKS4 USERID", f"socks4a://userid@127.0.0.1:{socks4}")
            await run("remote DNS", f"socks5h://127.0.0.1:{socks5}", url=f"https://remote-only.invalid:{https_target}/")
            await run("local DNS failure", f"socks5://127.0.0.1:{socks5}", url=f"https://remote-only.invalid:{https_target}/", expected="Inconclusive", code="LOCAL_DNS_FAILED")
            await run("untrusted proxy TLS", f"https://127.0.0.1:{https}", trust=False, expected="Failed", code="PROXY_TLS_INVALID")
            await run("untrusted target TLS", f"http://127.0.0.1:{http}", trust=False, expected="Inconclusive", code="TARGET_TLS_INVALID")
            await run("target hostname mismatch", f"socks5h://127.0.0.1:{socks5}", url=f"https://wrong-name.invalid:{https_target}/", expected="Inconclusive", code="TARGET_TLS_INVALID")
            ipv6_target = await fixtures.listen("ipv6_target", fixtures.endpoint, tls=True, host="::1")
            ipv6_proxy = await fixtures.listen("ipv6_proxy", fixtures.socks(5), host="::1")
            await run("IPv6 proxy endpoint", f"socks5h://[::1]:{ipv6_proxy}")
            await run("IPv6 destination", f"socks5://127.0.0.1:{socks5}", url=f"https://[::1]:{ipv6_target}/")
            await run("HTTP forward", f"http://127.0.0.1:{http}", url=f"http://localhost:{http_target}/")
            await run("CONNECT denied", f"http://127.0.0.1:{deny}", expected="Failed", code="CONNECT_DENIED")
            for status in (302, 403, 429, 503):
                await run(f"target HTTP {status}", f"http://127.0.0.1:{http}", url=f"https://localhost:{https_target}/status/{status}", expected="Inconclusive", code="TARGET_HTTP_ERROR")
            await run("invalid IP response", f"http://127.0.0.1:{http}", url=f"https://localhost:{https_target}/invalid", expected="Inconclusive", code="UNEXPECTED_RESPONSE")
            await run("response limit", f"http://127.0.0.1:{http}", url=f"https://localhost:{https_target}/large", expected="Inconclusive", code="RESPONSE_TOO_LARGE")
            await run("fallback via same proxy", f"http://127.0.0.1:{http}", url=f"https://localhost:{https_target}/status/503", extra={"fallbackUrl": f"https://localhost:{https_target}/"})
            await run("non-proxy server", f"127.0.0.1:{http_target}", expected="Inconclusive")
            wrong_protocol = await run("explicit wrong protocol", f"socks5://127.0.0.1:{http}", expected="Inconclusive", code="PROXY_HANDSHAKE_TIMEOUT", stage="protocol")
            assert wrong_protocol["detected"] is None, wrong_protocol
            assert [attempt["protocol"] for attempt in wrong_protocol["attempts"]] == ["socks5"], wrong_protocol
            for scheme in ("http", "https", "socks4", "socks4a", "socks5", "socks5h"):
                result = await run(f"silent {scheme} handshake", f"{scheme}://127.0.0.1:{silent}", expected="Inconclusive", code="PROXY_HANDSHAKE_TIMEOUT", stage="protocol")
                assert result["detected"] is None, result
            for scheme, port in [("http", http), ("socks5h", socks5)]:
                await run(f"{scheme} target TLS timeout", f"{scheme}://127.0.0.1:{port}", url=f"https://127.0.0.1:{silent}/", expected="Inconclusive", code="TARGET_TIMEOUT", stage="target")
                await run(f"{scheme} target response timeout", f"{scheme}://127.0.0.1:{port}", url=f"https://127.0.0.1:{https_target}/slow", expected="Inconclusive", code="TARGET_TIMEOUT", stage="target")
            await run("HTTP forward target timeout", f"http://127.0.0.1:{http}", url=f"http://127.0.0.1:{http_target}/slow", expected="Inconclusive", code="TARGET_TIMEOUT", stage="target")
            await run("proxy connection refused", f"http://127.0.0.1:{closed_port}", expected="Failed", code="CONNECTION_REFUSED")
            for scheme in ("https", "socks4", "socks4a", "socks5", "socks5h"):
                await run(f"{scheme} connection refused", f"{scheme}://127.0.0.1:{closed_port}", expected="Failed", code="CONNECTION_REFUSED", stage="connect")
            await run("proxy hostname failure", "http://missing-proxy.invalid:8080", expected="Failed", code="PROXY_DNS_FAILED")
            assert fixtures.connections.get("trap", 0) == 0, "Environment proxy was used"
            assert "remote-only.invalid" in fixtures.destinations, "Remote DNS was not observed"
            output = ROOT / "artifacts/network-results.json"
            output.parent.mkdir(exist_ok=True)
            output.write_text(json.dumps({"checks": checks, "environment_proxy_connections": 0, "proxy_connections": fixtures.connections}, indent=2)+"\n")
            print(f"{len(checks)} network cases passed; report: {output}")
        finally:
            closed_proxy.close()
            await fixtures.close()


if __name__ == "__main__":
    asyncio.run(acceptance())
