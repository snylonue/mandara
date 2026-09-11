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
            bookshelf
            bookshelf-server
            bookshelf-frontend
            bookshelf-plugins
            ;
        }
      );
    in
    flake-parts.lib.mkFlake { inherit inputs; } {
      # x86_64-darwin is not listed: nixpkgs 26.11 dropped that platform.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      flake.overlays.default = overlay;

      perSystem =
        { system, ... }:
        let
          # pkgs with the rust-overlay overlay applied, so that `rust-bin`
          # toolchains are available.
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          packages = import ./nix/packages.nix { inherit pkgs; };
        in
        {
          packages = packages // {
            default = packages.bookshelf;
          };

          formatter = pkgs.nixfmt-tree;

          devShells.default = pkgs.mkShell {
            packages = [
              pkgs.rust-bin.stable.latest.default.override
              {
                # targets needed to build wasm plugins:
                #   wasm32-unknown-unknown: simple reactor modules (no wasi)
                #   wasm32-wasip1 / wasip2:    preview1/preview2 reactor modules
                extensions = [ "rust-src" ];
                targets = [
                  "wasm32-unknown-unknown"
                  "wasm32-wasip1"
                  "wasm32-wasip2"
                ];
              }
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
              echo "bookshelf dev shell: $(cargo --version) / node $(node --version) / wasm-tools $(wasm-tools --version)"
            '';
          };
        };
    };
}
