# The backend binary. `plugins` is only staged for the host test suite,
# which checks that the packaged components introspect against this host;
# pass `plugins = null` to build the server without that dependency.
{
  lib,
  rustPlatform,
  version,
  meta,
  rustSource,
  plugins ? null,
}:

rustPlatform.buildRustPackage {
  pname = "mandara-server";
  inherit version;
  src = rustSource;
  cargoLock.lockFile = rustSource + "/Cargo.lock";
  # Only the server: the other workspace members are wasm plugin guests.
  cargoBuildFlags = [
    "-p"
    "mandara-server"
  ];
  # A component built from an older WIT version otherwise loads fine but
  # silently loses every capability, so the tests embed the packaged
  # components and fail on an export mismatch. Without the wasm toolchain
  # (plain nixpkgs, no rust-overlay) the suite cannot run and is skipped.
  doCheck = plugins != null;
  preCheck = lib.optionalString (plugins != null) ''
    mkdir -p plugins-built
    cp ${plugins}/lib/mandara/plugins/*.wasm plugins-built/
  '';
  meta = meta // {
    description = "mandara backend: REST API, auth, library, sessions, shares";
    mainProgram = "mandara-server";
  };
}
