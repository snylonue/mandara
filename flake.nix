{
  description = "mandara: a self-hosted light-novel reading website (rust backend + web frontend)";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    llm-agents = {
      url = "github:numtide/llm-agents.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Pinned rust toolchain (rustup-dist based). The wasm plugin crates need
    # it (nixpkgs' rustc ships no wasm32-unknown-unknown std); the dev shell
    # uses the same toolchain.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{
      flake-parts,
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      # Packages live in nix/packages.nix so the flake and the overlay share
      # one definition. The overlay composes rust-overlay in: the wasm plugin
      # build needs `pkgs.rust-bin`. It only *adds* attributes, so applying it
      # to a consumer's nixpkgs rebuilds nothing.
      overlay = nixpkgs.lib.composeExtensions rust-overlay.overlays.default (
        final: _prev:
        let
          packages = import ./nix/packages.nix { pkgs = final; };
        in
        {
          inherit (packages)
            mandara
            mandara-server
            mandara-frontend
            mandara-plugins
            ;
        }
      );

      # The module users import. It defaults the package to this flake's own
      # build, so no overlay is needed (and none is injected into the host);
      # `overlays.default` is there for configurations that prefer
      # `pkgs.mandara*` (e.g. `plugins = [ pkgs.mandara-plugins ]`).
      mandaraModule =
        { config, lib, ... }:
        {
          imports = [ ./nix/module.nix ];
          services.mandara.package = lib.mkDefault (
            inputs.self.packages.${config.nixpkgs.hostPlatform.system}.mandara
          );
        };
    in
    flake-parts.lib.mkFlake { inherit inputs; } {
      # x86_64-darwin is not listed: nixpkgs 26.11 dropped that platform.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      flake = {
        overlays.default = overlay;
        nixosModules = {
          mandara = mandaraModule;
          default = mandaraModule;
        };
      };

      perSystem =
        { system, lib, ... }:
        let
          # pkgs with the rust-overlay overlay applied, so that `rust-bin`
          # toolchains are available.
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          packages = import ./nix/packages.nix { inherit pkgs; };
          # Toolchain of the dev shell: same pinned rust, plus every wasm
          # target the plugins might use (a list element must be a value, so
          # the override cannot be inlined into `packages` below).
          devRustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" ];
            targets = [
              # wasm32-unknown-unknown: simple reactor modules (no wasi)
              "wasm32-unknown-unknown"
              # wasm32-wasip1 / wasip2: preview1/preview2 reactor modules
              "wasm32-wasip1"
              "wasm32-wasip2"
            ];
          };
        in
        {
          packages = packages // {
            default = packages.mandara;
          };

          formatter = pkgs.nixfmt-tree;

          checks =
            lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              mandara-vm = import ./nix/tests/mandara-vm.nix {
                inherit pkgs;
                module = mandaraModule;
                plugins = packages.mandara-plugins;
              };
            }
            // {
              # The VM test passes the packages explicitly; make sure the
              # overlay exposes them too, with the layout the module expects.
              overlay =
                let
                  overlaid = pkgs.extend overlay;
                in
                pkgs.runCommand "mandara-overlay-check" { } ''
                  test -f ${overlaid.mandara-plugins}/lib/mandara/plugins/hello.wasm
                  test -f ${overlaid.mandara-frontend}/share/mandara/frontend/dist/index.html
                  ${lib.getExe overlaid.mandara} --version | grep -q '^mandara-server '
                  touch $out
                '';
            };

          devShells.default = pkgs.mkShell {
            packages = [
              devRustToolchain
              # frontend (js dependencies are installed with npm)
              pkgs.nodejs
              # wasm plugin tooling (componentize a core wasm module)
              pkgs.wasm-tools
              # backend dev conveniences
              pkgs.sqlite
              pkgs.pkg-config
              # nix formatting (nixfmt is the RFC 166 style)
              pkgs.nixfmt
              # Diesel schema regeneration (`just schema`): sqlite-only,
              # mirroring `cargo install diesel_cli --no-default-features
              # --features sqlite` (see AGENTS.md — run after every new
              # migration).
              (pkgs.diesel-cli.override {
                sqliteSupport = true;
                postgresqlSupport = false;
                mysqlSupport = false;
              })
              inputs.llm-agents.packages.${system}.pi
              pkgs.just
              pkgs.python3
            ];
            shellHook = ''
              echo "mandara dev shell: $(cargo --version) / node $(node --version) / wasm-tools $(wasm-tools --version)"
            '';
          };
        };
    };
}
