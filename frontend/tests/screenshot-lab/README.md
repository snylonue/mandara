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

For CPU-source investigation, `ENERGY_VERSIONS=after` limits the comparison to the
current frontend. `ENERGY_VARIANTS` selects comma-separated diagnostic controls:

| Variant | Isolated behavior |
| --- | --- |
| `default` | Unmodified reader |
| `no-progress-animation` | Keep progress updates but disable their CSS transition |
| `hidden-progress` | Preserve track geometry and progress callbacks; omit its painting |
| `hidden-text` | Preserve layout and scrolling but omit chapter-content painting |
| `no-wheel-handler` | EPUB only: omit its native wheel listener before initialization |
| `transparent-text` | Keep paragraph geometry and backgrounds; omit visible glyph color |
| `no-radius` | TXT only: remove the article corner radius |
| `no-page-glow` | Remove only the outer document background image |
| `cached-content` | Hint a separate layer for EPUB body or TXT text, excluding the TXT article background |
| `cached-article` | TXT only: hint a layer including both article text and background |
| `suppress-scroll-observers` | Preserve native scrolling; skip scroll listener registration before application initialization |
| `opaque-scroll-surface` | Give the actual scrolling surface its existing backing color without transforms or layer hints |

Variant order rotates across repetitions. Hidden content and removed wheel listeners
are causal controls, not proposed product behavior. In particular, removing the wheel
listener breaks vertical-publication navigation. Verify the recorded geometry and
`skippedWheelHandlers` count before interpreting a control.

`ENERGY_TRACE=1` records a CDP trace and JavaScript CPU profile for each selected
scenario and builds source maps for attribution. Trace files can be loaded into Chrome
DevTools. Profiler samples are sampled wall time; trace `tdur` values, where present,
are thread CPU time. Nested trace durations must not be blindly summed. These runs
include tracing/profiling overhead and must be kept separate from uninstrumented CPU
comparisons. The regular mode does not start a profiler or tracing session.

For finer paint attribution on Linux:

```sh
ENERGY_VERSIONS=after ENERGY_SCENARIOS=scroll ENERGY_THREAD_CPU=1 ENERGY_WARMUP=1 node energy.mjs
ENERGY_VERSIONS=after ENERGY_SCENARIOS=scroll ENERGY_THREAD_CPU=1 ENERGY_WARMUP=1 ENERGY_PAINT_TRACE=1 ENERGY_PAINT_COMMANDS=1 node energy.mjs
node paint-analysis.mjs /path/to/one/result-directory
npm run test:paint
```

Keep these runs sequential and use distinct artifact directories. The first run has
no trace or sampling profiler. OS snapshots read only this browser's tracked tasks,
at the two endpoints, using the actual `getconf CLK_TCK` frequency. They include
thread-start identity, task churn, read failures, endpoint read duration, and an
independent comparison against CDP process CPU. A task named `chromium` can still
be a renderer main thread; its task id equals its process id.

`ENERGY_PAINT_TRACE=1` records paint-only traces without a JavaScript sampling
profiler. User Timing marks bound the active interval. The analyzer normalizes
synchronous B/E scopes and partitions X scopes on each thread's CPU clock
(`tts`/`tdur`), assigning overlapping/nested CPU only once. It reports missing
CPU clocks, incomplete scopes, excluded boundary scopes, and coverage against the
OS thread budget. Unknown time is never replaced with wall duration. Trace-enabled
CPU is not interchangeable with the clean run, even without a JS profiler.

Layer metadata connects raster task layer ids to DOM owners, dimensions, scrolling
rectangles, compositing reasons, and standalone paint commands. Optional
`ENERGY_PAINT_COMMANDS=1` snapshots and profiles layers **after** the measured
interval. Replay timings are isolated software-bitmap wall time, not live frame CPU;
profiles cannot simply be multiplied by frame count. Large-layer replay is skipped
above a 48 MB bitmap guard because Chromium applies the clip after full allocation.

`ENERGY_SCROLL_PX` changes scroll distance (default 3600); keep speed constant by
adjusting `ENERGY_SCROLL_MS`. `ENERGY_WARMUP=1` scrolls 600 px and restores the
position before measuring. To check work shifted outside a warm scroll window,
`ENERGY_LIFECYCLE=1` records cumulative browser-tree CPU from process startup,
including reaped-child CPU, plus endpoint PSS from `smaps_rollup`. Use a **fresh
runner/browser per format and variant**, disable warm-up, and include a longer
scroll. Endpoint PSS is neither peak memory nor physical GPU allocation. Read errors
remain explicit. `ENERGY_SCREENSHOTS=1` saves a screenshot after the timed scroll;
use separate visual checks rather than including capture work in CPU comparisons.
Layer hints are diagnostic only: they can change containing blocks, text rendering,
and resource retention in arbitrary publications.

For causal scroll-policy checks, `ENERGY_PREFER_COMPOSITING=1` launches Chromium
with `--enable-prefer-compositing-to-lcd-text`. This is a browser-policy
counterfactual, not a product optimization. The exact browser build and requested
flags are recorded. `ENERGY_SCROLL_DIAGNOSTICS=1` adds native CC debug, input,
browser and UI trace categories; its CPU overhead must not be treated as a clean
measurement. Suppressed-observer counts and actual scroll background are recorded.
The opaque-surface probe can override publication backgrounds and must not be
shipped unchanged; publisher background propagation requires separate validation.

Add `blank-idle` to `ENERGY_SCENARIOS` for a fresh empty-context control before each
repetition. `ENERGY_IDLE_SEGMENTS=4 ENERGY_IDLE_MS=5000` separates consecutive idle
windows instead of extrapolating a startup/scroll tail into a steady cost. Segment
identities remain separate in analysis. Segmentation is rejected with tracing,
preventing trace artifacts from being overwritten. Idle trace records also report
active page/frame animations. Browser frame-clock work is not automatically proof
of an application timer, and unmeasured user-action frequencies must not be invented
to convert these budgets into battery-life estimates.

Implementation references: [CDP LayerTree](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/LayerTree.pdl),
[Chromium picture replay](https://raw.githubusercontent.com/chromium/chromium/main/third_party/blink/renderer/platform/graphics/picture_snapshot.cc),
and [display-list raster playback](https://raw.githubusercontent.com/chromium/chromium/main/cc/paint/display_item_list.cc).

Results include every sample, alternating A/B order, fixture geometry, CPU/process metadata,
API request counts, and skipped/failed capabilities. Idle periods do not poll the page or run
an animation-frame sampler. Selection changes use real DOM selections, not fabricated
selection events; scrolling uses Chromium input gestures. Screenshot rasterization,
progress persistence, and physical mobile devices are outside the default workload.
Cold work is included only in the optional cumulative lifecycle check.

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
