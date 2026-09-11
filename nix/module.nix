# NixOS module for bookshelf.
#
# Everything mutable (SQLite database, retained originals, images, plugin
# drop-ins) lives in one directory under /var/lib, so a backup is a tar of
# that directory. The server itself is stateless: config comes in on the
# command line plus a handful of environment variables.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.bookshelf;
  inherit (lib)
    mkIf
    mkOption
    mkEnableOption
    types
    ;

  dataDir = "/var/lib/${cfg.stateDirectory}";

  # Directory of `*.wasm` components to load. `cfg.plugins` may list
  # component files (a derivation is accepted) and directories/packages
  # containing them; a store directory of symlinks is what the server sees,
  # so the state directory stays untouched.
  pluginsDir =
    if cfg.plugins == [ ] then
      null
    else
      pkgs.runCommand "bookshelf-plugins-dir" { } ''
        mkdir -p $out
        for src in ${lib.escapeShellArgs (map (p: "${p}") cfg.plugins)}; do
          if [ -f "$src" ]; then
            ln -s "$src" "$out/$(basename "$src")"
          else
            find "$src" -name '*.wasm' -exec ln -s {} "$out/" \;
          fi
        done
      '';

  # Command line arguments of the server (clap). Booleans are environment
  # variables instead: `--allow-register` has no negation and defaults to
  # on, so `BOOKSHELF_ALLOW_REGISTER=false` is the only way to turn it off.
  cliArgs = [
    "--addr"
    "${cfg.address}:${toString cfg.port}"
    "--db"
    "${dataDir}/bookshelf.db"
    "--data-dir"
    dataDir
    "--plugins-dir"
    (if pluginsDir != null then "${pluginsDir}" else "${dataDir}/plugins")
    "--max-upload-mb"
    (toString cfg.maxUploadMb)
  ];

  # Shared between the server and the one-shot re-parse task.
  commonServiceConfig = {
    User = cfg.user;
    Group = cfg.group;
    StateDirectory = cfg.stateDirectory;
    StateDirectoryMode = "0750";
    WorkingDirectory = dataDir;

    # --- sandboxing -------------------------------------------------------
    # NoNewPrivileges + no capabilities: the server never needs privileges.
    NoNewPrivileges = true;
    CapabilityBoundingSet = "";
    AmbientCapabilities = [ ];
    PrivateDevices = true;
    PrivateTmp = true;
    # Strict read-only /, except the state directory (ReadWritePaths is
    # implied by StateDirectory).
    ProtectSystem = "strict";
    ProtectHome = true;
    ProtectClock = true;
    ProtectControlGroups = true;
    ProtectHostname = true;
    ProtectKernelLogs = true;
    ProtectKernelModules = true;
    ProtectKernelTunables = true;
    ProtectProc = "invisible";
    ProcSubset = "pid";
    LockPersonality = true;
    RemoveIPC = true;
    RestrictNamespaces = true;
    RestrictRealtime = true;
    RestrictSUIDSGID = true;
    SystemCallArchitectures = "native";
    # AF_NETLINK for getaddrinfo; plugins do their own HTTP fetches.
    RestrictAddressFamilies = [
      "AF_UNIX"
      "AF_INET"
      "AF_INET6"
      "AF_NETLINK"
    ];
    # wasmtime JIT-compiles every plugin at startup: it needs writable and
    # executable mappings, so MemoryDenyWriteExecute must stay off, and
    # cranelift's anonymous memory images use memfd_create (not in
    # @system-service).
    MemoryDenyWriteExecute = false;
    SystemCallFilter = [
      "@system-service"
      "memfd_create"
      "~@privileged"
      "~@obsolete"
    ];
    UMask = "0027";
  };
in
{
  options.services.bookshelf = {
    enable = mkEnableOption "the bookshelf light-novel reading server";

    package = mkOption {
      type = types.package;
      default = pkgs.bookshelf;
      defaultText = lib.literalExpression "pkgs.bookshelf";
      description = ''
        The bookshelf package to run. It must provide the server binary and
        (unless {option}`frontendDir` is set) a bundled web frontend.
      '';
    };

    user = mkOption {
      type = types.str;
      default = "bookshelf";
      description = ''
        User the service runs as. A system user of this name is created
        when it is left at the default.
      '';
    };

    group = mkOption {
      type = types.str;
      default = "bookshelf";
      description = ''
        Group the service runs as. A group of this name is created when it
        is left at the default.
      '';
    };

    stateDirectory = mkOption {
      type = types.str;
      default = "bookshelf";
      description = ''
        Name of the state directory below {file}`/var/lib`. It holds the
        SQLite database, the retained original files, the image store and
        (unless {option}`plugins` is set) the `*.wasm` plugin drop-ins, so
        backing bookshelf up means copying this one directory.
      '';
    };

    address = mkOption {
      type = types.str;
      default = "127.0.0.1";
      example = "0.0.0.0";
      description = "Address to listen on.";
    };

    port = mkOption {
      type = types.port;
      default = 8080;
      description = "Port to listen on.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Open {option}`port` in the firewall.";
    };

    jwtSecretFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = "/run/secrets/bookshelf-jwt-secret";
      description = ''
        File holding the secret session tokens are signed with; its content
        is read at startup and only needs to be readable by root (systemd
        hands it to the service as a credential). Generate one with
        `openssl rand -base64 32`.

        Leave unset only for local experiments: without it the server falls
        back to a built-in development secret and anyone can forge logins.
      '';
    };

    allowRegister = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Allow new users to register. The first registered account becomes
        the admin, so on a public instance this is usually turned off once
        the admin account exists.
      '';
    };

    cookieSecure = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Mark the session cookie `Secure`, so browsers only send it over
        https. Enable this when the server is reachable through TLS (a
        reverse proxy counts).
      '';
    };

    maxUploadMb = mkOption {
      type = types.ints.positive;
      default = 64;
      description = "Maximum upload size in MiB.";
    };

    frontendDir = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = "/srv/www/bookshelf";
      description = ''
        Directory whose `dist/` subdirectory is served at `/`. Defaults to
        the frontend bundled with {option}`package`.
      '';
    };

    plugins = mkOption {
      type = types.listOf (types.either types.path types.package);
      default = [ ];
      example = lib.literalExpression "with pkgs; [ bookshelf-plugins ]";
      description = ''
        wasm plugin components (`*.wasm`) to load at startup. Entries may be
        component files or directories/packages containing components
        (like `pkgs.bookshelf-plugins`), which is also how a plugin built
        from source is deployed.

        When empty, the server loads the `*.wasm` files dropped into
        `lib/plugins/` inside the state directory instead.
      '';
    };

    environment = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example = {
        RUST_LOG = "info";
        BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS = "15000";
      };
      description = "Extra environment variables for the service.";
    };

    environmentFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = ''
        Environment file (systemd `EnvironmentFile=`) with further settings,
        e.g. `BOOKSHELF_JWT_SECRET=...` for deployments that cannot use
        {option}`jwtSecretFile`.
      '';
    };

    extraArgs = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Extra command line arguments passed to the server.";
    };
  };

  config = mkIf cfg.enable {
    warnings =
      lib.optional
        (
          cfg.jwtSecretFile == null
          && !(cfg.environment ? BOOKSHELF_JWT_SECRET)
          && cfg.environmentFile == null
        )
        ''
          services.bookshelf.jwtSecretFile is not set, so the server signs session
          tokens with a built-in development secret and anyone can forge logins.
          Set services.bookshelf.jwtSecretFile (e.g. `openssl rand -base64 32`).
        '';

    users.users = mkIf (cfg.user == "bookshelf") {
      bookshelf = {
        isSystemUser = true;
        group = cfg.group;
        home = dataDir;
        description = "bookshelf service user";
      };
    };
    users.groups = mkIf (cfg.group == "bookshelf") { bookshelf = { }; };

    networking.firewall.allowedTCPPorts = mkIf cfg.openFirewall [ cfg.port ];

    systemd.services.bookshelf = {
      description = "Bookshelf light-novel reading server";
      documentation = [ "https://github.com/example/bookshelf" ];
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      environment = {
        BOOKSHELF_ALLOW_REGISTER = lib.boolToString cfg.allowRegister;
        BOOKSHELF_COOKIE_SECURE = lib.boolToString cfg.cookieSecure;
      }
      // lib.optionalAttrs (cfg.jwtSecretFile != null) {
        BOOKSHELF_JWT_SECRET_FILE = "%d/jwt-secret";
      }
      // lib.optionalAttrs (cfg.frontendDir != null) {
        BOOKSHELF_FRONTEND_DIR = toString cfg.frontendDir;
      }
      // cfg.environment;

      serviceConfig = commonServiceConfig // {
        EnvironmentFile = lib.mkIf (cfg.environmentFile != null) cfg.environmentFile;
        LoadCredential = lib.mkIf (cfg.jwtSecretFile != null) "jwt-secret:${cfg.jwtSecretFile}";
        ExecStart = lib.escapeShellArgs ([ (lib.getExe cfg.package) ] ++ cliArgs ++ cfg.extraArgs);
        Restart = "on-failure";
        RestartSec = 5;
      };
    };

    # One-shot upgrade task: re-run the current parser over every retained
    # original (e.g. after a bookshelf upgrade that improves chapter
    # splitting): `systemctl start bookshelf-reparse-originals`.
    systemd.services.bookshelf-reparse-originals = {
      description = "Bookshelf: re-parse every retained original book file";
      documentation = [ "https://github.com/example/bookshelf" ];
      after = [ "bookshelf.service" ];
      serviceConfig = commonServiceConfig // {
        Type = "oneshot";
        ExecStart = lib.escapeShellArgs (
          [ (lib.getExe cfg.package) ] ++ cliArgs ++ [ "--reparse-originals" ] ++ cfg.extraArgs
        );
      };
    };
  };
}
