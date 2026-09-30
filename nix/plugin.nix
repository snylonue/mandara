# One wasm plugin component. The guest crates are plain core wasm modules;
# `wasm-tools component new` lifts them into components (the WIT world
# imports no wasi, so no adapter is involved). Output:
# $out/lib/mandara/plugins/<name>.wasm.
{
  lib,
  buildRustPackage,
  wasmTools,
  version,
  meta,
  src,
  # Crate name, e.g. `hello-plugin`.
  crate,
}:

let
  name = lib.removeSuffix "-plugin" crate;
  # cargo names the artifact after the lib target, with `-` -> `_`.
  artifact = lib.replaceStrings [ "-" ] [ "_" ] crate;
in
buildRustPackage {
  pname = "mandara-plugin-${name}";
  inherit version src;
  cargoLock.lockFile = src + "/Cargo.lock";
  nativeBuildInputs = [ wasmTools ];
  doCheck = false;

  # buildRustPackage's hooks would force the host target; drive cargo
  # directly for the wasm target (deps are vendored by the setup hook).
  buildPhase = ''
    runHook preBuild
    cargo build --offline --release --target wasm32-unknown-unknown -p ${crate}
    runHook postBuild
  '';

  postBuild = ''
    mkdir -p components
    wasm-tools component new \
      "target/wasm32-unknown-unknown/release/${artifact}.wasm" \
      -o "components/${name}.wasm"
    wasm-tools validate --features component-model "components/${name}.wasm"
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p $out/lib/mandara/plugins
    cp components/${name}.wasm $out/lib/mandara/plugins/
    runHook postInstall
  '';

  meta = meta // {
    description = "mandara wasm plugin component: ${name} (book/metadata source)";
  };
}
