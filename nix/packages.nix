# Build recipes for mandara: the server binary (bundled with the built
# web frontend) and the wasm plugin components.
#
# Used by the flake (`packages.<system>`) and by `overlays.default`, so
# applying the overlay gives a consumer's own nixpkgs `pkgs.mandara`.
#
# The server and frontend build with plain nixpkgs (whatever rustc that
# nixpkgs pins); only `mandara-plugins` needs the rust-overlay toolchain,
# because nixpkgs' rustc ships no `wasm32-unknown-unknown` std.
{
  pkgs,
  lib ? pkgs.lib,
  # Repository root; override to build from another checkout.
  src ? ../.,
}:

let
  workspace = builtins.fromTOML (builtins.readFile (src + "/Cargo.toml"));
  version = workspace.workspace.package.version;
  repository = workspace.workspace.package.repository;

  # Keep build artifacts (target/, node_modules/, dist/) and runtime data
  # (data/) out of the store; the flake already filters by git, this makes
  # the file work with a plain `callPackage` too.
  cleanSource =
    dir:
    lib.cleanSourceWith {
      src = dir;
      filter =
        path: type:
        lib.cleanSourceFilter path type
        && !(builtins.elem (baseNameOf (toString path)) [
          "target"
          "node_modules"
          "dist"
          "data"
          "plugins-built"
        ]);
    };

  # Source of the Rust builds: only the cargo workspace (the manifests plus
  # the member crates), so touching docs, the frontend or these nix
  # expressions never invalidates a Rust build.
  rustSource = lib.fileset.toSource {
    root = src;
    fileset = lib.fileset.unions [
      (src + "/Cargo.toml")
      (src + "/Cargo.lock")
      (src + "/crates")
      (src + "/plugins")
    ];
  };

  source = cleanSource src;

  meta = {
    description = "Self-hosted light-novel reading website (epub/txt, multi-user, wasm plugins)";
    homepage = repository;
    # The workspace declares `MIT OR Apache-2.0`.
    license = with lib.licenses; [
      mit
      asl20
    ];
    platforms = lib.platforms.unix;
    maintainers = [ ];
  };

  # Every `plugins/*-plugin` crate, so adding a plugin never needs a change
  # here (each is a cdylib built for wasm32-unknown-unknown).
  pluginCrates = lib.mapAttrsToList (name: _: name) (
    lib.filterAttrs (name: type: type == "directory" && lib.hasSuffix "-plugin" name) (
      builtins.readDir (src + "/plugins")
    )
  );

  # --- web frontend ---------------------------------------------------------
  # Vite build output, laid out where the server expects it: the server
  # serves `<frontend-dir>/dist`.
  frontend = pkgs.buildNpmPackage {
    pname = "mandara-frontend";
    inherit version;
    src = cleanSource (src + "/frontend");
    npmDepsHash = "sha256-sjZFG4aJhN63GFYzKb0VaoJY9kTyYWmdUh3LDM0f3Lg=";
    installPhase = ''
      runHook preInstall
      mkdir -p $out/share/mandara/frontend
      cp -r dist $out/share/mandara/frontend/
      runHook postInstall
    '';
    meta = meta // {
      description = "mandara web frontend (Vite build output)";
    };
  };

  # --- server binary --------------------------------------------------------
  server = pkgs.rustPlatform.buildRustPackage {
    pname = "mandara-server";
    inherit version;
    src = rustSource;
    cargoLock.lockFile = rustSource + "/Cargo.lock";
    # Only the server: the other workspace members are wasm plugin guests.
    cargoBuildFlags = [
      "-p"
      "mandara-server"
    ];
    # The test suite loads `plugins-built/*.wasm` (a dev build output) and
    # verifies that the components still introspect against this host — a
    # component built from an older WIT version otherwise loads fine but
    # silently loses every capability. Stage the packaged components where
    # the tests expect them; without the wasm toolchain (plain nixpkgs, no
    # rust-overlay) that is impossible, so the suite is skipped.
    doCheck = pkgs ? rust-bin;
    preCheck = lib.optionalString (pkgs ? rust-bin) ''
      mkdir -p plugins-built
      cp ${plugins}/lib/mandara/plugins/*.wasm plugins-built/
    '';
    meta = meta // {
      description = "mandara backend: REST API, auth, library, sessions, shares";
      mainProgram = "mandara-server";
    };
  };

  # --- wasm plugins ---------------------------------------------------------
  # The guest crates are plain core wasm modules; `wasm-tools component new`
  # lifts them into components (the WIT world imports no wasi, so no
  # adapter is involved). Output: $out/lib/mandara/plugins/<name>.wasm.
  rustToolchain = pkgs.rust-bin.stable.latest.default.override {
    targets = [ "wasm32-unknown-unknown" ];
  };
  wasmRustPlatform = pkgs.makeRustPlatform {
    cargo = rustToolchain;
    rustc = rustToolchain;
  };

  plugins =
    assert lib.assertMsg (pkgs ? rust-bin)
      "mandara-plugins needs the rust-overlay toolchain (nixpkgs' rustc has no wasm32-unknown-unknown std); apply `inputs.mandara.overlays.default` or use the flake's packages";
    wasmRustPlatform.buildRustPackage {
      pname = "mandara-plugins";
      inherit version;
      src = rustSource;
      cargoLock.lockFile = rustSource + "/Cargo.lock";
      nativeBuildInputs = [ pkgs.wasm-tools ];
      doCheck = false;

      # buildRustPackage's hooks would force the host target; drive cargo
      # directly for the wasm target (deps are vendored by the setup hook).
      buildPhase = ''
        runHook preBuild
        cargo build --offline --release --target wasm32-unknown-unknown \
          ${lib.concatMapStringsSep " " (name: "-p ${name}") pluginCrates}
        runHook postBuild
      '';

      postBuild = ''
        mkdir -p components
        ${lib.concatMapStrings (name: ''
          wasm-tools component new \
            "target/wasm32-unknown-unknown/release/${lib.replaceStrings [ "-" ] [ "_" ] name}.wasm" \
            -o "components/${lib.removeSuffix "-plugin" name}.wasm"
        '') pluginCrates}
        for f in components/*.wasm; do wasm-tools validate --features component-model "$f"; done
      '';

      installPhase = ''
        runHook preInstall
        mkdir -p $out/lib/mandara/plugins
        cp components/*.wasm $out/lib/mandara/plugins/
        runHook postInstall
      '';

      meta = meta // {
        description = "mandara wasm plugin components (book/metadata sources)";
      };
    };

  # --- combined server package ---------------------------------------------
  # The server binary plus the frontend bundle, with the bundle wired up as
  # the *default* of `MANDARA_FRONTEND_DIR` (an explicitly set variable or
  # `--frontend-dir` still wins, e.g. from the NixOS module).
  mandara =
    pkgs.runCommand "mandara-${version}"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        passthru = {
          inherit
            server
            frontend
            plugins
            ;
        };
        meta = meta // {
          description = "Self-hosted light-novel reading website";
          mainProgram = "mandara-server";
        };
      }
      ''
        mkdir -p $out/bin $out/share/mandara
        ln -s ${frontend}/share/mandara/frontend $out/share/mandara/frontend
        makeWrapper ${server}/bin/mandara-server $out/bin/mandara-server \
          --set-default MANDARA_FRONTEND_DIR $out/share/mandara/frontend
      '';
in
{
  inherit mandara;
  mandara-server = server;
  mandara-frontend = frontend;
  mandara-plugins = plugins;
}
