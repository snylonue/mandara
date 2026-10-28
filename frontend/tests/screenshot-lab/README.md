# Screenshot fidelity lab

This is an isolated diagnostic suite, not a replacement screenshot implementation.
Comparator-only dependencies belong to this directory and do not enter the production
frontend bundle. SnapDOM is also a production dependency; html-to-image is now only
an isolated comparator.

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
- Compare the production passage renderer and final PNG against the selected native
  pixel region, adding only the PNG's documented 16 CSS px outer padding. Attribution
  is disabled for this body comparison. Partial ranges use native pixels masked by
  the original range rectangles, without creating a second text layout.
- Test the strongest candidates after selection extraction as a separate stage:
  an accurate renderer cannot recover sibling context or counter state already
  removed by the selection snapshot.

## Coverage

Nineteen synthetic publications cover mixed-language text, variable font weights,
italic synthesis, ruby, bidi, emoji, vertical writing, inherited variables,
pseudo-elements, counters, grid/flex/table layout, shadows/transforms/clipping,
media queries and container units, raster/background/SVG images, EPUB semantic
attributes, partial selections, long content, TXT, delayed images, scrollable
children, dark backgrounds, and a genuinely missing image (HTTP 404).

Profiles are 772x700 and 390x740 CSS px at DPR 2. The narrower viewport is **not**
physical mobile-device testing. Browser startup failures are recorded separately
from library failures. Linux WebKit is not a substitute for Safari on an iPhone.

Ten additional real-browser boundary checks exercise empty/oversized selections,
width/height guards, cancellation cleanup, missing images/fonts, and the canvas
pixel budget with a painted marker at the very end of the document. A small
selection from a chapter over 25,000 CSS px high also verifies that only the
selected region is subject to the output-size guard, while sibling styles survive.

The suite does not yet establish behavior for every CSS feature, platform font,
real EPUB, font/image race, browser zoom level, or physical device. Timings are
observations, not controlled performance benchmarks. No real book content or
production user data is required.

## Production capture contract

- Snapshot the complete inert document, CSSOM rules, stylesheet state, original
  viewport, publication boundary, scroll positions, and range endpoint paths.
  Removing unselected siblings before layout changes selectors and counters.
- Restore that context in an isolated frame, embed and load the required fonts,
  then let SnapDOM freeze computed presentation. Do not replay publication CSS
  against its synthetic pseudo-elements. Rewrite SVG image references only after
  attribute selectors have been evaluated. Preflight CSS, HTML, and SVG image
  resources with the frame's origin policy and an abort signal, including missing
  backgrounds that SnapDOM otherwise drops silently.
- Preserve the complete publication paint tree instead of maintaining a list of
  CSS features whose ancestors must be retained. Keep the TXT application shell
  outside this boundary and composite its uniform background without importing
  its viewport clipping into the excerpt.
- Crop in page coordinates and mask partial ranges after layout. Original text
  shaping remains intact; unselected pixels are not included in the PNG.
- Preserve measured zero-width scrollbar gutters in the SVG image environment.
  Retain CSS dimensions separately from rounded, budget-limited device pixels.
- Treat renderer fallback/resource warnings as failures rather than silently
  incomplete exports. The advisory reconcile-risk warning is not a resource
  fallback and is not, by itself, an export failure.

The lab records pixel mismatches; a successful diagnostic command does **not**
mean every capture is exact. Inspect summary.json and per-case diffs. In particular,
SnapDOM 3.4's generated-content/first-letter combination and small long-text/TXT
raster differences remain explicit fidelity limitations, not normalized baselines.
No universal browser/CSS fidelity guarantee is implied.

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

### Reader CPU / energy diagnostics

Run the production A/B workload from this directory after installing the lab dependencies:

```sh
NIXPKGS=github:NixOS/nixpkgs/a32edd7654519351e48e80372a928df336394670
nix run "$NIXPKGS#chromium" -- --version
export CHROMIUM_EXECUTABLE="$(nix eval --raw "$NIXPKGS#chromium.outPath")/bin/chromium"
node energy.mjs
```

The runner builds the baseline and current frontend using the same installed dependencies.
A Vite loader supplies historical source without checking out or overwriting working files.
It serves synthetic chapter/API fixtures from private loopback ports, runs sequentially in
Chromium, and closes its servers and browser afterward. No backend or user data is used.

The default baseline is commit `1cd039ee`, before the reader hot-path changes. Override
`ENERGY_BASE` when comparing another revision. `ENERGY_OUTPUT` controls the artifact directory;
`ENERGY_REPETITIONS`, `ENERGY_IDLE_MS`, `ENERGY_SCROLL_MS`,
`ENERGY_SELECTION_STEPS`, and `ENERGY_SETTINGS_STEPS` control workload sizes.
`ENERGY_FORMATS` selects `epub,txt`; `ENERGY_SCENARIOS` selects comma-separated
`idle,scroll,selection,settings,background-idle` phases.
For native background-tab checks:

```sh
ENERGY_NATIVE_FOCUS=1 ENERGY_FORMATS=epub ENERGY_SCENARIOS=background-idle node energy.mjs
```

This launches a fresh browser profile and connects with `noDefaults: true`, so
Playwright does not force pages to stay focused/visible.
The native browser may require broader runtime access for shared memory and its
diagnostic directory; the runner never changes those directory permissions.

Results include every sample, alternating A/B order, fixture geometry, CPU/process metadata,
API request counts, and skipped/failed capabilities. Idle periods do not poll the page or run
an animation-frame sampler. Selection changes use real DOM selections, not fabricated
selection events; scrolling uses Chromium input gestures. Screenshot rasterization,
progress persistence, cold loading, and physical mobile devices are outside this workload.

Duration metrics use CDP `threadTicks` (thread running time); process totals use
`SystemInfo.getProcessInfo` CPU seconds across the browser's processes. The latter has
coarser resolution and is invalid for a sample with process churn. Heap deltas without
forced GC are not retained-memory or peak-memory measurements. Background results are
reported only if the page genuinely becomes hidden. The runner disables Playwright's
automatic focus emulation and activates another real tab; visibility is never spoofed.

These are CPU-work measurements, **not energy measurements** when the RAPL counter is
unreadable. The diagnostic records that permission failure rather than changing host
permissions. If the counter is readable, it additionally records wrap-aware package
energy deltas and average power. Those cover the entire CPU package, including unrelated
processes, and must not be described as per-page or whole-machine energy. A software-rendered headless browser cannot establish physical GPU/display
power, battery life, or real-screen frame pacing. Do not convert CPU percentage reductions
into energy savings. Keep other profiling and build/test jobs stopped during the run.


To validate only the production pipeline and its boundaries:

```sh
LAB_BROWSERS=chromium LAB_RENDERERS=current-passage,current-png \
LAB_BOUNDARIES=1 LAB_OUTPUT=results/production npm test
```

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
