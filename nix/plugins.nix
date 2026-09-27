# All in-repo wasm components in one directory, for deployments that want
# `[ pkgs.mandara-plugins ]`. It only links the per-plugin derivations, so
# each component is still built on its own.
{
  lib,
  runCommand,
  version,
  meta,
  pluginPackages,
}:

runCommand "mandara-plugins-${version}"
  {
    meta = meta // {
      description = "mandara wasm plugin components (all in-repo plugins)";
    };
  }
  ''
    mkdir -p $out/lib/mandara/plugins
    ${lib.concatMapStrings (package: ''
      ln -s ${package}/lib/mandara/plugins/*.wasm $out/lib/mandara/plugins/
    '') (lib.attrValues pluginPackages)}
  ''
