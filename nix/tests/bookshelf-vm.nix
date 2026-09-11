# End-to-end test of the NixOS module: the server boots, serves the bundled
# frontend, authenticates with the credential-held JWT secret, loads a wasm
# plugin (wasmtime JIT-compiles it at startup, under the module's sandbox)
# and parses an uploaded txt book.
{
  pkgs,
  bookshelf,
  plugins,
}:
pkgs.testers.runNixOSTest {
  name = "bookshelf";

  nodes.machine = {
    imports = [ ../module.nix ];
    services.bookshelf = {
      enable = true;
      package = bookshelf;
      plugins = [ "${plugins}/lib/bookshelf/plugins/hello.wasm" ];
      jwtSecretFile = "/etc/bookshelf-jwt-secret";
    };
    environment.etc."bookshelf-jwt-secret".text = "test-jwt-secret-not-for-production";
  };

  testScript = ''
    import base64
    import json

    start_all()
    machine.wait_for_unit("bookshelf.service")
    machine.wait_for_open_port(8080)

    with subtest("health endpoint"):
        health = json.loads(machine.succeed("curl -fsS http://127.0.0.1:8080/api/health"))
        assert health["status"] == "ok", health
        assert health["allow_register"] is True, health

    with subtest("bundled frontend is served"):
        machine.succeed("curl -fsS http://127.0.0.1:8080/ | grep -q Bookshelf")

    with subtest("state directory"):
        machine.succeed("test -f /var/lib/bookshelf/bookshelf.db")

    with subtest("first account becomes admin"):
        credentials = json.dumps({"username": "admin", "password": "admin-password"})
        auth = json.loads(machine.succeed(
            "curl -fsS -X POST http://127.0.0.1:8080/api/auth/register "
            f"-H 'Content-Type: application/json' -d '{credentials}'"
        ))
        assert auth["user"]["role"] == "admin", auth
        token = auth["token"]

    with subtest("wasm plugin component loaded"):
        wasm_files = json.loads(machine.succeed(
            f"curl -fsS -H 'Authorization: Bearer {token}' "
            "http://127.0.0.1:8080/api/plugins/wasm-files"
        ))
        assert "hello.wasm" in wasm_files, wasm_files

    with subtest("txt upload is parsed and readable"):
        content = "第一章 开端\n第一段正文\n第二章 后续\n第二段正文\n"
        encoded = base64.b64encode(content.encode()).decode()
        machine.succeed(f"echo {encoded} | base64 -d > /tmp/book.txt")
        upload = json.loads(machine.succeed(
            "curl -fsS -X POST http://127.0.0.1:8080/api/books "
            f"-H 'Authorization: Bearer {token}' "
            "-F file=@/tmp/book.txt -F visibility=public -F title=测试书"
        ))
        detail = upload["books"][0]
        assert detail["book"]["title"] == "测试书", detail
        assert detail["files"][0]["chapter_count"] == 2, detail
        file_id = detail["files"][0]["id"]
        chapter = json.loads(machine.succeed(
            f"curl -fsS -H 'Authorization: Bearer {token}' "
            f"http://127.0.0.1:8080/api/files/{file_id}/chapters/0"
        ))
        assert "第一段正文" in chapter["content"], chapter
  '';
}
