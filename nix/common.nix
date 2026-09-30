# Shared values for the mandara build recipes: the workspace version, the
# cargo workspace source, the clean-source filter and package metadata. Kept
# in one place so `frontend.nix`, `server.nix` and `plugin.nix` can be
# overridden independently.
{
  lib,
  # Repository root.
  repoRoot,
}:

let
  workspace = builtins.fromTOML (builtins.readFile (repoRoot + "/Cargo.toml"));
in
{
  version = workspace.workspace.package.version;

  meta = {
    description = "Self-hosted light-novel reading website (epub/txt, multi-user, wasm plugins)";
    homepage = workspace.workspace.package.repository;
    # The workspace declares `MIT OR Apache-2.0`.
    license = with lib.licenses; [
      mit
      asl20
    ];
    platforms = lib.platforms.unix;
    maintainers = [ ];
  };

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
  src = lib.fileset.toSource {
    root = repoRoot;
    fileset = lib.fileset.unions [
      (repoRoot + "/Cargo.toml")
      (repoRoot + "/Cargo.lock")
      (repoRoot + "/crates")
      (repoRoot + "/plugins")
    ];
  };

  # Every `plugins/*-plugin` crate, so adding a plugin never needs a change
  # here (each is a cdylib built for wasm32-unknown-unknown).
  pluginCrates = lib.mapAttrsToList (name: _: name) (
    lib.filterAttrs (name: type: type == "directory" && lib.hasSuffix "-plugin" name) (
      builtins.readDir (repoRoot + "/plugins")
    )
  );
}
