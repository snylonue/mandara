# Screenshot fidelity lab

This is an isolated diagnostic suite, not a replacement screenshot implementation.
Candidate dependencies belong to this directory and do not enter the production
frontend bundle.

## Method

- Mount the real EpubDocument component with its sandbox, font injection, gutter
  handling, reader settings, and publication CSS. TXT uses the real reader CSS
  and selectedTextContent path.
- Capture the actual displayed pixels with Playwright, twice, and reject unstable
  references. Long iframe content is captured as visible scroll tiles without
  enlarging the original viewport; viewport-dependent styles keep their meaning.
- Run each library in a fresh page. Use the same source fonts and resources;
  enable font embedding where the library requires it. Do not replace publisher
  backgrounds with a capture option. Composite transparent output on white only
  after rendering.
- Keep fixture bounds integer-valued to avoid native clipping-rounding noise.
  Do not resize, align, blur, or otherwise normalize output images.
- Compare exact RGBA pixels and Pixelmatch differences (threshold 0.1, excluding
  antialiasing). Record geometry changes separately, even when changed pixels are
  zero. Matching this finite corpus is not proof of universal fidelity.
- Compare the production passage renderer and final PNG against native pixels,
  retaining the document's right gutter and adding only the PNG's documented
  16 CSS px outer padding. Attribution is disabled for this body comparison.
- Test the strongest candidates after selection extraction as a separate stage:
  an accurate renderer cannot recover sibling context or counter state already
  removed by the selection snapshot.

## Coverage

Eighteen synthetic publications cover mixed-language text, variable font weights,
italic synthesis, ruby, bidi, emoji, vertical writing, inherited variables,
pseudo-elements, counters, grid/flex/table layout, shadows/transforms/clipping,
media queries and container units, raster/background/SVG images, EPUB semantic
attributes, partial selections, long content, TXT, delayed images, scrollable
children, dark backgrounds, and a genuinely missing image (HTTP 404).

Profiles are 772x700 and 390x740 CSS px at DPR 2. The narrower viewport is **not**
physical mobile-device testing. Browser startup failures are recorded separately
from library failures. Linux WebKit is not a substitute for Safari on an iPhone.

Eight additional real-browser boundary checks exercise empty/oversized selections,
width/height guards, cancellation cleanup, missing images/fonts, and the canvas
pixel budget with a painted marker at the very end of the document.

The suite does not yet establish behavior for every CSS feature, platform font,
real EPUB, font/image race, browser zoom level, or physical device. Timings are
observations, not controlled performance benchmarks. No real book content or
production user data is required.

## Run

Install the regular frontend dependencies first, then the isolated lab:

```sh
cd frontend
npm ci
cd tests/screenshot-lab
npm ci
```

Use Playwright 1.63.0-compatible browsers. With the repository's pinned Nixpkgs,
materialize the optional browser tools without changing the development shell:

```sh
NIXPKGS=github:NixOS/nixpkgs/a32edd7654519351e48e80372a928df336394670
nix run "$NIXPKGS#playwright-test" -- --version
export PLAYWRIGHT_BROWSERS_PATH="$(nix eval --raw "$NIXPKGS#playwright-driver.browsers")"
npm test
```

The runner owns a temporary loopback-only Vite server and closes it when finished.
It does not start or replace the application's deployed server.

Optional environment variables:

| Variable            | Purpose                                                  |
| ------------------- | -------------------------------------------------------- |
| LAB_BROWSERS        | Comma-separated engines; default chromium,firefox,webkit |
| LAB_CASES           | Comma-separated fixture IDs; default all                 |
| LAB_PROFILE         | wide or narrow; default both                             |
| LAB_RENDERERS       | Comma-separated adapters; see run.mjs                    |
| LAB_OUTPUT          | Artifact directory; default results                      |
| LAB_TIMEOUT         | Per-capture deadline in milliseconds; default 15000      |
| LAB_BOUNDARIES      | Run boundary checks when set                             |
| LAB_ONLY_BOUNDARIES | Run no pixel-comparison cases when set                   |
| LAB_RESUME          | Reuse completed cases from the same output directory     |
| LAB_REDO_CASES      | Discard specified case IDs when resuming                 |

Resume only with unchanged fixtures, browser builds, and capture settings; redo
any affected cases after changing configuration. Normal runs start a fresh result
set. Old unreferenced artifact files are not used as baselines.

Example selection-stage experiment:

```sh
LAB_BROWSERS=chromium \
LAB_CASES=plain,selection-siblings,selection-counters,responsive,long \
LAB_RENDERERS=snapdom-snapshot,dom-to-image-more-snapshot \
LAB_OUTPUT=results/snapshot npm test

LAB_BROWSERS=chromium LAB_BOUNDARIES=1 LAB_ONLY_BOUNDARIES=1 \
LAB_OUTPUT=results/boundaries npm test
```

Reports, native reference PNGs, output PNGs, diff PNGs, source measurements,
selection-context comparisons, and browser diagnostics are written under the
output directory. Dependencies and generated artifacts are ignored by Git.
The command fails when no browser can produce a capture, or a boundary assertion
fails. Pixel mismatches are diagnostic results, not an assertion that every
candidate library should pass.
