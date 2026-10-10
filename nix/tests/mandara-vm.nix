# End-to-end test of the modular service: the server boots, serves the bundled
# frontend, authenticates with the credential-held JWT secret, loads the
# packaged wasm plugins (wasmtime JIT-compiles them at startup, under the
# module's sandbox) and parses an uploaded txt book.
#
# `module` is the package's services.default, so this test uses what a user
# writes in configuration.nix: the package default, the packaged components
# and the module's environment wiring.
{
  pkgs,
  module,
  # One package per plugin component; the caller passes them as the list a
  # deployment gets when it lists `mandara-plugin-<name>` packages.
  plugins,
}:
pkgs.testers.runNixOSTest {
  name = "mandara";

  nodes = {
    machine = { config, ... }: {
      assertions = [
        {
          assertion = config.systemd.services.mandara.serviceConfig.StateDirectory == "mandara";
          message = "The modular service must configure its primary unit.";
        }
        {
          assertion =
            config.systemd.services.mandara.serviceConfig.User
            != config.systemd.services.library.serviceConfig.User;
          message = "Mandara instances must use separate service users by default.";
        }
      ];

      system.services.mandara = {
        imports = [ module ];
        mandara = {
          inherit plugins;
          jwtSecretFile = "/etc/mandara-jwt-secret";
          # session cookies must carry Secure (asserted below)
          cookieSecure = true;
        };
      };
      system.services.library = {
        imports = [ module ];
        mandara = {
          port = 8081;
          allowRegister = false;
        };
      };
      environment.etc."mandara-jwt-secret".text = "test-jwt-secret-not-for-production";
    };

    # Second instance with the opposite boolean options: the module passes
    # them as environment variables, which needs its own coverage.
    locked = {
      system.services.mandara = {
        imports = [ module ];
        mandara = {
          jwtSecretFile = "/etc/mandara-jwt-secret";
          allowRegister = false;
        };
      };
      environment.etc."mandara-jwt-secret".text = "test-jwt-secret-not-for-production";
    };
  };

  testScript = ''
    import base64
    import json

    start_all()
    machine.wait_for_unit("mandara.service")
    machine.wait_for_open_port(8080)
    machine.wait_for_unit("library.service")
    machine.wait_for_open_port(8081)
    locked.wait_for_unit("mandara.service")
    locked.wait_for_open_port(8080)

    with subtest("health endpoint"):
        health = json.loads(machine.succeed("curl -fsS http://127.0.0.1:8080/api/health"))
        assert health["status"] == "ok", health
        assert health["allow_register"] is True, health

    with subtest("bundled frontend is served"):
        machine.succeed("curl -fsS http://127.0.0.1:8080/ | grep -q Mandara")

    with subtest("state directory"):
        machine.succeed("test -f /var/lib/mandara/mandara.db")

    with subtest("multiple service instances are isolated"):
        machine.succeed("test -f /var/lib/library/mandara.db")
        health = json.loads(machine.succeed("curl -fsS http://127.0.0.1:8081/api/health"))
        assert health["allow_register"] is False, health
        machine.succeed("systemctl start library-reparse-originals.service")
        machine.succeed("systemctl is-active mandara.service library.service")

    with subtest("first account becomes admin"):
        credentials = json.dumps({"username": "admin", "password": "admin-password"})
        # keep the response headers: the session cookie of a cookieSecure
        # instance must be Secure + HttpOnly
        body = machine.succeed(
            "curl -fsS -D /tmp/register.headers -X POST http://127.0.0.1:8080/api/auth/register "
            f"-H 'Content-Type: application/json' -d '{credentials}'"
        )
        headers = machine.succeed("cat /tmp/register.headers").lower()
        assert "set-cookie: mandara_token=" in headers, headers
        assert "secure" in headers and "httponly" in headers, headers
        auth = json.loads(body)
        assert auth["user"]["role"] == "admin", auth
        token = auth["token"]

    with subtest("allowRegister = false is honoured"):
        health = json.loads(locked.succeed("curl -fsS http://127.0.0.1:8080/api/health"))
        assert health["allow_register"] is False, health
        nobody = json.dumps({"username": "nobody", "password": "x"})
        locked.fail(
            "curl -fsS -X POST http://127.0.0.1:8080/api/auth/register "
            f"-H 'Content-Type: application/json' -d '{nobody}'"
        )

    with subtest("wasm plugin components loaded"):
        wasm_files = json.loads(machine.succeed(
            f"curl -fsS -H 'Authorization: Bearer {token}' "
            "http://127.0.0.1:8080/api/plugins/wasm-files"
        ))
        expected = ["bangumi.wasm", "hello.wasm", "reader.wasm", "wenku8.wasm", "wiki.wasm"]
        assert wasm_files == expected, wasm_files

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

    with subtest("re-parse task shares the server state"):
        machine.succeed("systemctl start mandara-reparse-originals.service")
        reparsed = json.loads(machine.succeed(
            f"curl -fsS -H 'Authorization: Bearer {token}' "
            f"http://127.0.0.1:8080/api/files/{file_id}/chapters/0"
        ))
        assert "第一段正文" in reparsed["content"], reparsed
  '';
}
