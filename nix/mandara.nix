# The server binary plus the frontend bundle, with the bundle wired up as
# the *default* of `MANDARA_FRONTEND_DIR` (an explicitly set variable or
# `--frontend-dir` still wins, e.g. from the NixOS module).
{
  runCommand,
  makeWrapper,
  version,
  meta,
  server,
  frontend,
  # The component set the package was built from, exposed for introspection.
  plugins ? null,
}:

runCommand "mandara-${version}"
  {
    nativeBuildInputs = [ makeWrapper ];
    passthru = {
      inherit server frontend plugins;
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
  ''
