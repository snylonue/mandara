{
  description = "bookshelf: a self-hosted light-novel reading website (rust backend + web frontend)";

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
    # Pinned rust toolchain (rustup-dist based). Used to build the server and
    # the wasm plugins (wasm32 targets are installed via this overlay).
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
    flake-parts.lib.mkFlake { inherit inputs; } (
      {
        ...
      }:
      let
        systems = [
          "x86_64-linux"
          "aarch64-linux"
          "x86_64-darwin"
          "aarch64-darwin"
        ];
      in
      {
        inherit systems;
        perSystem =
          { system, ... }:
          let
            # pkgs with the rust-overlay overlay applied, so that
            # `rust-bin` toolchains are available.
            pkgs = import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            };
            rustToolchain = pkgs.rust-bin.stable.latest.default.override {
              # targets needed to build wasm plugins:
              #   wasm32-unknown-unknown: simple reactor modules (no wasi)
              #   wasm32-wasip1 / wasip2:    preview1/preview2 reactor modules
              extensions = [ "rust-src" ];
              targets = [
                "wasm32-unknown-unknown"
                "wasm32-wasip1"
                "wasm32-wasip2"
              ];
            };
          in
          {
            devShells.default = pkgs.mkShell {
              packages = [
                rustToolchain
                # frontend (js dependencies are installed with npm)
                pkgs.nodejs
                # wasm plugin tooling (componentize a core wasm module)
                pkgs.wasm-tools
                # backend dev conveniences
                pkgs.sqlite
                pkgs.pkg-config
                inputs.llm-agents.packages.${system}.pi
                pkgs.just
                pkgs.python3
              ];
              shellHook = ''
                echo "bookshelf dev shell: $(cargo --version) / node $(node --version) / wasm-tools $(wasm-tools --version)"
              '';
            };

            # Convenience: nix run .#server  /  nix run .#frontend-native
            packages.server = pkgs.rustPlatform.buildRustPackage {
              pname = "bookshelf-server";
              version = "0.1.0";
              src = ./.;
              cargoLock.lockFile = ./Cargo.lock;
              cargoBuildFlags = [ "-p" "bookshelf-server" ];
              nativeBuildInputs = [ pkgs.pkg-config ];
            };
          };
      }
    );
}