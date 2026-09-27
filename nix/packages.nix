# Build recipes for mandara, composed as a `lib.makeScope` package set so a
# consumer can override one component at a time:
#
#   pkgs.mandaraPackages.overrideScope (final: prev: {
#     frontend = prev.frontend.override { ... };
#   })
#
# Each component lives in its own file (frontend.nix, server.nix,
# plugin.nix, plugins.nix, mandara.nix). The server and frontend build with
# plain nixpkgs; only the plugins need the rust-overlay toolchain, because
# nixpkgs' rustc ships no `wasm32-unknown-unknown` std.
{
  pkgs,
  lib ? pkgs.lib,
  # Repository root; override to build from another checkout.
  src ? ../.,
}:

let
  common = import ./common.nix { inherit lib src; };

  scope = lib.makeScope pkgs.newScope (
    self:
    let
      inherit (self) callPackage;

      buildPlugin =
        crate:
        callPackage ./plugin.nix {
          inherit crate;
          inherit (common) version meta rustSource;
          buildRustPackage = self.wasmRustPlatform.buildRustPackage;
          wasmTools = pkgs.wasm-tools;
        };

      pluginPackages = lib.listToAttrs (
        map (crate: {
          name = "mandara-plugin-${lib.removeSuffix "-plugin" crate}";
          value = buildPlugin crate;
        }) common.pluginCrates
      );
    in
    {
      # wasm toolchain for the plugin crates. `pkgs.rust-bin` comes from
      # rust-overlay; keeping both as scope attributes makes the whole
      # plugin set rebuildable with a different toolchain.
      rustToolchain = pkgs.rust-bin.stable.latest.default.override {
        targets = [ "wasm32-unknown-unknown" ];
      };

      wasmRustPlatform = pkgs.makeRustPlatform {
        cargo = self.rustToolchain;
        rustc = self.rustToolchain;
      };

      frontend = callPackage ./frontend.nix {
        inherit (common) version meta cleanSource;
        inherit src;
      };

      plugins = callPackage ./plugins.nix {
        inherit pluginPackages;
        inherit (common) version meta;
      };

      server = callPackage ./server.nix {
        inherit (common) version meta rustSource;
        # The host tests need the packaged components; plain nixpkgs (no
        # rust-overlay) has no wasm toolchain, so they are skipped there.
        plugins = if pkgs ? rust-bin then self.plugins else null;
      };

      mandara = callPackage ./mandara.nix {
        inherit (common) version meta;
        inherit (self) server frontend plugins;
      };

      inherit pluginPackages;
    }
    // pluginPackages
  );
in
{
  mandara = scope.mandara;
  mandara-server = scope.server;
  mandara-frontend = scope.frontend;
  mandara-plugins = scope.plugins;
  # The whole scope, for `overrideScope`-style overrides. Not a derivation,
  # so the flake keeps it out of `packages.<system>`.
  mandaraPackages = scope;
}
// scope.pluginPackages
