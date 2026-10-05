# Vite build output, laid out where the server expects it: the server serves
# `<frontend-dir>/dist`.
{
  buildNpmPackage,
  version,
  meta,
  cleanSource,
  # Repository root.
  src,
}:

buildNpmPackage {
  pname = "mandara-frontend";
  inherit version;
  src = cleanSource (src + "/frontend");
  npmDepsHash = "sha256-9x0g1i6+PpE7CJlVuU9aSwQlgZnRwcK1pzk+z2nFfV4=";
  installPhase = ''
    runHook preInstall
    mkdir -p $out/share/mandara/frontend
    cp -r dist $out/share/mandara/frontend/
    runHook postInstall
  '';
  meta = meta // {
    description = "mandara web frontend (Vite build output)";
  };
}
