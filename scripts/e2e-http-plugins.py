#!/usr/bin/env python3
"""Exercise real HTTP plugins against isolated upstream and host servers."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--build", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    if args.build:
        subprocess.run(["bash", "scripts/build-plugins.sh"], cwd=root, check=True)
    subprocess.run(["cargo", "build", "-p", "mandara-server"], cwd=root, check=True)
    mock_port, host_port = port(), port()
    base = f"http://127.0.0.1:{host_port}/api"
    token = ""
    children = []
    with tempfile.TemporaryDirectory(prefix="mandara-http-e2e-") as tmp:
        try:
            children.append(subprocess.Popen(["python3", "scripts/mock-source.py", str(mock_port)], cwd=root, stdout=subprocess.DEVNULL))
            env = os.environ | {"MANDARA_ADDR": f"127.0.0.1:{host_port}", "MANDARA_DB": f"{tmp}/library.db", "MANDARA_DATA_DIR": tmp, "MANDARA_JWT_SECRET": "isolated-e2e-secret-not-for-production", "MANDARA_PLUGINS_DIR": str(root / "plugins-built")}
            children.append(subprocess.Popen([str(root / "target/debug/mandara-server")], cwd=root, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))

            def request(method, path, body=None, content_type="application/json", expected=None):
                data = json.dumps(body).encode() if isinstance(body, (dict, list)) else body
                headers = {"Content-Type": content_type, "Authorization": f"Bearer {token}"}
                req = urllib.request.Request(base + path, data=data, method=method, headers=headers)
                try:
                    with urllib.request.urlopen(req, timeout=30) as response:
                        result = response.read()
                        assert expected is None, f"{method} {path}: expected HTTP {expected}, got success"
                        return json.loads(result) if result else None
                except urllib.error.HTTPError as error:
                    if expected == error.code:
                        return None
                    raise AssertionError(f"{method} {path}: {error.code}: {error.read().decode()}") from error

            def acquire(fields, upload=False, expected=None):
                boundary = "mandara-test-boundary"
                data = bytearray()
                for name, value in fields.items():
                    data.extend(f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode())
                if upload:
                    data.extend(f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="sample.txt"\r\nContent-Type: text/plain\r\n\r\n第一章 测试\n正文\r\n'.encode())
                data.extend(f"--{boundary}--\r\n".encode())
                return request("POST", "/books", bytes(data), f"multipart/form-data; boundary={boundary}", expected)

            # Readiness is bounded; these are test-owned processes, not harness jobs.
            deadline = time.monotonic() + 30
            while True:
                assert all(p.poll() is None for p in children), "test server exited before readiness"
                try:
                    with urllib.request.urlopen(f"http://127.0.0.1:{mock_port}/wiki/api/search", timeout=1):
                        pass
                    token = request("POST", "/auth/register", {"username": "e2e-admin", "password": "e2e-password"})["token"]
                    break
                except urllib.error.URLError:
                    if time.monotonic() > deadline:
                        raise AssertionError("servers did not become ready")
                    time.sleep(0.1)

            for source in ["wiki", "reader"]:
                config = {"base-url": f"http://127.0.0.1:{mock_port}/{source}"}
                config["page-size" if source == "wiki" else "max-chapters"] = 20
                request("POST", "/plugins/instances", {"id": source, "wasm_file": source + ".wasm", "config": config})
            search = request("GET", "/plugins/wiki/search?limit=3")
            assert len(search["items"]) == 3 and all(i["kind"] == "book" for i in search["items"])
            lookup = request("GET", "/plugins/reader/lookup?q=r-9")
            assert [b["id"] for b in lookup["items"]] == ["r-alpha", "r-beta"]
            acquire({"plugin_source": "reader", "plugin_book_id": "r-9"}, expected=400)
            print("ok: series lookup exposes real book IDs, not one combined book")

            result = acquire({"plugin_source": "reader", "plugin_book_id": "r-beta", "meta_plugin_source": "wiki", "meta_plugin_book_id": "w-beta", "visibility": "public"})
            assert len(result["books"]) == 1 and result["series"] is not None
            detail = result["books"][0]
            book_id, file = detail["book"]["id"], detail["files"][0]
            assert detail["book"]["volume_no"] == 2
            assert file["external_id"] == "r-beta" and file["content_mode"] == "chapters"
            assert "volume_offset" not in file and "volume_no" not in file
            assert file["format"] == "plugin"  # Guest declares chapter content despite both capabilities.
            assert file["chapter_count"] == 2
            chapter = request("GET", f'/files/{file["id"]}/chapters/0')
            assert chapter["title"] == "第三章 觉醒"
            assert "mock-source.py" in chapter["content"]
            print("ok: selected metadata/content books stay independent; chapter zero is volume-local")

            request("POST", "/plugins/instances", {"id": "hello", "wasm_file": "hello.wasm", "config": {"site-name": "独立内容源"}})
            attached = acquire({"plugin_source": "hello", "plugin_book_id": "hello-content-2", "book_id": book_id, "visibility": "public"})["books"][0]
            assert attached["book"]["id"] == book_id
            assert attached["book"]["title"] == detail["book"]["title"]
            content_only = next(f for f in attached["files"] if f["source"] == "hello")
            assert content_only["external_id"] == "hello-content-2"
            assert content_only["content_mode"] == "chapters"
            assert request("GET", f'/files/{content_only["id"]}/chapters/2')["content"] == ""
            request("DELETE", f'/files/{content_only["id"]}')
            print("ok: content IDs attach without provider metadata lookup")

            first = acquire({"plugin_source": "wiki", "plugin_book_id": "w-alpha", "metadata_only": "true"})
            assert first["series"]["id"] == result["series"]["id"]
            assert first["books"][0]["files"] == []
            request("POST", f'/books/{book_id}/refresh')
            assert request("GET", f'/files/{file["id"]}/chapters/0')["content"] == chapter["content"]
            print("ok: separately acquired metadata-only volumes share their explicit parent series")

            second = acquire({"plugin_source": "reader", "plugin_book_id": "r-alpha", "book_id": book_id})["books"][0]["files"][0]
            assert second["id"] != file["id"]
            assert request("GET", f'/files/{second["id"]}/chapters/0')["title"] == "第一章 相遇"
            request("POST", f'/books/{book_id}/refresh')
            details = request("GET", f'/books/{book_id}')
            assert {f["external_id"] for f in details["files"]} == {"r-alpha", "r-beta"}
            request("POST", f'/files/{second["id"]}/rematerialize')
            assert request("GET", f'/files/{file["id"]}/chapters/0')["content"] == chapter["content"]
            request("DELETE", f'/files/{second["id"]}')
            assert request("GET", f'/files/{file["id"]}/chapters/0')["content"] == chapter["content"]
            request("POST", f'/files/{file["id"]}/content-source', {"content_source": "reader"}, expected=405)
            print("ok: multiple content items are independently selectable, refreshable and deletable")

            uploaded = acquire({"visibility": "public"}, upload=True)["books"][0]
            local = uploaded["files"][0]
            assert local["content_mode"] == "file"
            before = request("GET", f'/files/{local["id"]}/chapters/0')
            request("POST", f'/files/{local["id"]}/rematerialize')
            assert request("GET", f'/files/{local["id"]}/chapters/0') == before
            attached = acquire({"book_id": uploaded["book"]["id"], "visibility": "public"}, upload=True)
            assert attached["books"][0]["book"]["id"] == uploaded["book"]["id"]
            plugin = acquire({"plugin_source": "reader", "plugin_book_id": "r-2", "book_id": uploaded["book"]["id"]})
            assert plugin["books"][0]["files"][0]["format"] == "epub"
            another = acquire({"plugin_source": "reader", "plugin_book_id": "r-2", "book_id": book_id})
            assert another["books"][0]["files"][0]["id"] != plugin["books"][0]["files"][0]["id"]
            print("ok: upload/attach/file-mode acquisition remains supported")
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()


if __name__ == "__main__":
    main()
