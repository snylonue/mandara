# The backend binary. Plugins are deliberately not an input: the server
# package builds on its own, and a deployment selects the components it
# wants through `services.mandara.plugins`.
{
  rustPlatform,
  version,
  meta,
  src,
}:

rustPlatform.buildRustPackage {
  pname = "mandara-server";
  inherit version src;
  cargoLock.lockFile = src + "/Cargo.lock";
  # Only the server: the other workspace members are wasm plugin guests.
  cargoBuildFlags = [
    "-p"
    "mandara-server"
  ];
  # The workspace suite also runs `mandara-plugin`'s introspection test,
  # which reads built components from `plugins-built/`; restricting the
  # check to this crate keeps the server build independent of the plugins
  # (the VM check loads the packaged components end to end instead).
  cargoTestFlags = [
    "-p"
    "mandara-server"
  ];
  meta = meta // {
    description = "mandara backend: REST API, auth, library, sessions, shares";
    mainProgram = "mandara-server";
  };
}
