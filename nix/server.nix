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
  cargoBuildFlags = [
    "-p"
    "mandara-server"
  ];
  # restricting the check to this crate keeps the server build independent of the plugins
  cargoTestFlags = [
    "-p"
    "mandara-server"
  ];
  meta = meta // {
    description = "mandara backend: REST API, auth, library, sessions, shares";
    mainProgram = "mandara-server";
  };
}
