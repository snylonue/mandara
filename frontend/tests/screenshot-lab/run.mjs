import { mkdir, writeFile, readFile } from "node:fs/promises";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, firefox, webkit } from "playwright";
import { createServer } from "../../node_modules/vite/dist/node/index.js";
import { PNG } from "pngjs";
import pixelmatch from "pixelmatch";
import { fixtures } from "./fixtures.mjs";

const dir = dirname(fileURLToPath(import.meta.url));
const output = resolve(process.env.LAB_OUTPUT || join(dir, "results"));
const only = new Set((process.env.LAB_CASES || "").split(",").filter(Boolean));
const renderers = (
  process.env.LAB_RENDERERS ||
  "html-to-image,snapdom,snapdom-reconcile,modern-screenshot,dom-to-image-more,html2canvas,current-passage,current-png"
).split(",");
const engines = (process.env.LAB_BROWSERS || "chromium,firefox,webkit").split(
  ",",
);
const profiles = [
  { name: "wide", width: 772, height: 700, dpr: 2 },
  { name: "narrow", width: 390, height: 740, dpr: 2 },
].filter(
  (p) =>
    !process.env.LAB_ONLY_BOUNDARIES &&
    (!process.env.LAB_PROFILE || p.name === process.env.LAB_PROFILE),
);
const timeout = Number(process.env.LAB_TIMEOUT || 15000);
const records = [],
  browsers = [],
  boundaries = [];
await mkdir(output, { recursive: true });
if (process.env.LAB_RESUME) {
  try {
    const previous = JSON.parse(
      await readFile(join(output, "results.json"), "utf8"),
    );
    const redo = new Set((process.env.LAB_REDO_CASES || "").split(","));
    records.push(...previous.records.filter((r) => !redo.has(r.case)));
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
}
const illustration = new PNG({ width: 96, height: 64 });
for (let y = 0; y < 64; y++)
  for (let x = 0; x < 96; x++) {
    const i = (y * 96 + x) * 4;
    illustration.data[i] = x < 48 ? 220 : 40;
    illustration.data[i + 1] = y < 32 ? 120 : 200;
    illustration.data[i + 2] = 170;
    illustration.data[i + 3] = 255;
  }
const image = PNG.sync.write(illustration);
const server = await createServer({
  root: resolve(dir, "../.."),
  configFile: resolve(dir, "../../vite.config.ts"),
  server: { host: "127.0.0.1", port: 0, hmr: false, proxy: { "/api": null } },
  logLevel: "error",
  plugins: [
    {
      name: "lab-images",
      configureServer(s) {
        s.middlewares.use("/api/images/", (req, res) => {
          if (req.url.includes("missing.png")) {
            res.statusCode = 404;
            res.end();
            return;
          }
          const send = () => {
            res.setHeader("Content-Type", "image/png");
            res.end(image);
          };
          const delay = Number(
            new URL(req.url, "http://localhost").searchParams.get("delay") || 0,
          );
          if (delay) setTimeout(send, delay);
          else send();
        });
      },
    },
  ],
});
await server.listen();
const base =
  "http://127.0.0.1:" +
  server.httpServer.address().port +
  "/tests/screenshot-lab/index.html";
const white = (w, h) => {
  const p = new PNG({ width: w, height: h });
  p.data.fill(255);
  return p;
};
function pad(p, amount) {
  const q = white(p.width + amount * 2, p.height + amount * 2);
  PNG.bitblt(p, q, 0, 0, p.width, p.height, amount, amount);
  return q;
}
function compare(reference, actual) {
  const width = Math.max(reference.width, actual.width),
    height = Math.max(reference.height, actual.height);
  const a = white(width, height),
    b = white(width, height),
    diff = white(width, height);
  PNG.bitblt(reference, a, 0, 0, reference.width, reference.height, 0, 0);
  PNG.bitblt(actual, b, 0, 0, actual.width, actual.height, 0, 0);
  const changed = pixelmatch(a.data, b.data, diff.data, width, height, {
    threshold: 0.1,
    includeAA: false,
  });
  let exact = 0;
  for (let i = 0; i < a.data.length; i += 4)
    if (
      a.data[i] !== b.data[i] ||
      a.data[i + 1] !== b.data[i + 1] ||
      a.data[i + 2] !== b.data[i + 2] ||
      a.data[i + 3] !== b.data[i + 3]
    )
      exact++;
  return {
    diff,
    changed,
    exact,
    percent: (changed / (width * height)) * 100,
    geometryMatch:
      reference.width === actual.width && reference.height === actual.height,
    expected: [reference.width, reference.height],
    actual: [actual.width, actual.height],
  };
}
// Capture long iframe content as visible native tiles. Enlarging the frame would
// change media queries and vh units; locator.screenshot alone can clip its content.
async function nativeScreenshot(page, fixture, profile) {
  if (fixture.range) {
    const { clip, mask } = await page.evaluate(() => window.lab.nativeRange());
    const viewport = fixture.format === "txt" ? { x: 0, y: 0 } : await page.locator("iframe").boundingBox();
    const png = PNG.sync.read(await page.screenshot({ clip: { ...clip, x: clip.x + viewport.x, y: clip.y + viewport.y }, animations: "disabled", caret: "hide" }));
    // Native browser pixels are the oracle; the range mask defines only what may be disclosed.
    for (let y = 0; y < png.height; y++) for (let x = 0; x < png.width; x++) {
      const cssX = (x + 0.5) / profile.dpr, cssY = (y + 0.5) / profile.dpr;
      if (!mask.some(rect => cssX >= rect.x && cssX < rect.x + rect.width && cssY >= rect.y && cssY < rect.y + rect.height)) png.data.fill(255, (y * png.width + x) * 4, (y * png.width + x + 1) * 4);
    }
    return PNG.sync.write(png);
  }
  if (fixture.format === "txt")
    return page
      .locator(fixture.selection || "#capture")
      .screenshot({ animations: "disabled", caret: "hide" });
  const frame = page.frameLocator("iframe");
  const element = frame.locator(fixture.selection || "#capture");
  const handle = await page.locator("iframe").elementHandle();
  const viewport = await handle.boundingBox();
  const inner = await handle.contentFrame();
  const geometry = await element.evaluate((el) => {
    const r = el.getBoundingClientRect(),
      w = el.ownerDocument.defaultView;
    return {
      x: r.left + w.scrollX,
      y: r.top + w.scrollY,
      width: Math.ceil(r.width),
      height: Math.ceil(r.height),
      scrollX: w.scrollX,
      scrollY: w.scrollY,
    };
  });
  const result = white(
    geometry.width * profile.dpr,
    geometry.height * profile.dpr,
  );
  try {
    for (let y = 0; y < geometry.height;) {
      const tile = await inner.evaluate(
        async ({ g, y }) => {
          window.scrollTo(g.x, g.y + y);
          await new Promise((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(resolve)),
          );
          const x = g.x - window.scrollX,
            top = g.y + y - window.scrollY;
          return {
            x,
            y: top,
            width: Math.min(g.width, window.innerWidth - x),
            height: Math.min(g.height - y, window.innerHeight - top),
          };
        },
        { g: geometry, y },
      );
      if (tile.height <= 0 || tile.width < geometry.width)
        throw new Error("Native tile lies outside the reading viewport");
      const data = await page.screenshot({
        clip: {
          x: viewport.x + tile.x,
          y: viewport.y + tile.y,
          width: geometry.width,
          height: tile.height,
        },
        animations: "disabled",
        caret: "hide",
      });
      const png = PNG.sync.read(data);
      const copyHeight = Math.min(png.height, result.height - y * profile.dpr);
      PNG.bitblt(
        png,
        result,
        0,
        0,
        Math.min(png.width, result.width),
        copyHeight,
        0,
        y * profile.dpr,
      );
      y += tile.height;
    }
  } finally {
    await inner.evaluate(
      (g) => window.scrollTo(g.scrollX, g.scrollY),
      geometry,
    );
  }
  return PNG.sync.write(result);
}
async function save() {
  await writeFile(
    join(output, "results.json"),
    JSON.stringify(
      {
        generatedAt: new Date().toISOString(),
        environment: {
          node: process.version,
          platform: process.platform,
          browsersPath: process.env.PLAYWRIGHT_BROWSERS_PATH,
          fontconfigFile: process.env.FONTCONFIG_FILE,
          timeout,
          pixelmatch: { threshold: 0.1, includeAA: false },
        },
        packages: {
          ...JSON.parse(await readFile(join(dir, "package.json"), "utf8"))
            .dependencies,
          "html-to-image": JSON.parse(
            await readFile(
              join(dir, "node_modules/html-to-image/package.json"),
              "utf8",
            ),
          ).version,
        },
        browsers,
        records,
        boundaries,
      },
      null,
      2,
    ),
  );
  const summary = renderers.map((renderer) => {
    const rows = records.filter((r) => r.renderer === renderer);
    const ok = rows.filter((r) => r.status === "ok");
    return {
      renderer,
      total: rows.length,
      errors: rows.length - ok.length,
      exact: ok.filter((r) => r.exact === 0 && r.geometryMatch).length,
      zeroDiff: ok.filter((r) => r.changed === 0 && r.geometryMatch).length,
      geometryMismatch: ok.filter((r) => !r.geometryMatch).length,
      medianPercent: ok.length
        ? ok.map((r) => r.percent).sort((a, b) => a - b)[
            Math.floor(ok.length / 2)
          ]
        : null,
    };
  });
  await writeFile(
    join(output, "summary.json"),
    JSON.stringify(summary, null, 2),
  );
  let report =
    "# Native-browser screenshot comparison\n\nGenerated: " +
    new Date().toISOString() +
    "\n\nThe reference is the actual EpubDocument component (or the TXT reader styles), captured by Playwright, not another DOM screenshot library. Each reference is captured twice to check stability. No image resizing, alignment search, or font replacement is applied.\n\nPixelmatch uses threshold 0.1 and excludes antialiasing; exact pixel changes are also recorded. Geometry mismatches always remain failures. These synthetic cases are a diagnostic suite, not proof of universal fidelity or a substitute for real-device testing.\n\n## Summary\n\n| Renderer | Cases | Errors | Exact matches | Zero non-AA diff | Geometry mismatches | Median changed pixels |\n|---|---:|---:|---:|---:|---:|---:|\n";
  for (const s of summary)
    report +=
      "| " +
      s.renderer +
      " | " +
      s.total +
      " | " +
      s.errors +
      " | " +
      s.exact +
      " | " +
      s.zeroDiff +
      " | " +
      s.geometryMismatch +
      " | " +
      (s.medianPercent?.toFixed(3) ?? "-") +
      "% |\n";
  report +=
    "\n## Browser availability\n\n" +
    browsers
      .map(
        (b) => "- " + b.name + ": " + (b.error || b.runtimeError || b.version),
      )
      .join("\n") +
    "\n\n## Per-case results\n\n| Browser/profile/case | Renderer | Result | Changed pixels | Dimensions | Artifacts |\n|---|---|---|---:|---|---|\n";
  for (const r of records) {
    const folder = r.browser + "/" + r.profile + "/" + r.case;
    report +=
      "| " +
      folder +
      " | " +
      r.renderer +
      " | " +
      r.status +
      " | " +
      (r.percent?.toFixed(3) ?? "-") +
      "% | " +
      (r.expected?.join("×") || "") +
      " → " +
      (r.actual?.join("×") || "") +
      " | " +
      (r.status === "ok"
        ? "[reference](<" +
          folder +
          "/" +
          (r.renderer === "current-png"
            ? "reference-padded"
            : r.renderer === "current-passage"
              ? "reference-passage"
              : "reference") +
          ".png>) · [output](<" +
          folder +
          "/" +
          r.renderer +
          ".png>) · [diff](<" +
          folder +
          "/" +
          r.renderer +
          "-diff.png>)"
        : String(r.error)
            .replaceAll("|", "/")
            .replaceAll("\n", " ")
            .slice(0, 250)) +
      " |\n";
  }
  if (boundaries.length) {
    report += "\n## Boundary checks\n\n";
    for (const group of boundaries)
      for (const result of group.results)
        report +=
          "- " +
          group.browser +
          "/" +
          result.name +
          ": " +
          (result.passed ? "PASS" : "FAIL") +
          (result.detail ? " — " + result.detail : "") +
          "\n";
  }
  await writeFile(join(output, "report.md"), report);
  return summary;
}
try {
  for (const name of engines) {
    const type = { chromium, firefox, webkit }[name];
    let browser;
    try {
      browser = await type.launch({ headless: true, timeout: 30000 });
      browsers.push({ name, version: browser.version() });
    } catch (error) {
      browsers.push({ name, error: String(error) });
      console.log(name + ": unavailable: " + String(error).split("\n")[0]);
      await save();
      continue;
    }
    try {
      if (process.env.LAB_BOUNDARIES) {
        const page = await browser.newPage();
        try {
          await page.goto(base);
          await page.waitForFunction(() => window.lab);
          const results = await page.evaluate(() => window.lab.boundaries());
          boundaries.push({ browser: name, results });
          console.log(name + " boundary checks: " + JSON.stringify(results));
          if (results.some((r) => !r.passed)) process.exitCode = 1;
        } finally {
          await page.close();
        }
      }
      for (const profile of profiles) {
        const context = await browser.newContext({
          viewport: { width: profile.width, height: profile.height },
          deviceScaleFactor: profile.dpr,
          colorScheme: "light",
          locale: "zh-CN",
        });
        context.setDefaultTimeout(20000);
        try {
          for (const fixture of fixtures.filter(
            (f) => !only.size || only.has(f.id),
          )) {
            if (
              renderers.every((renderer) =>
                records.some(
                  (r) =>
                    r.browser === name &&
                    r.profile === profile.name &&
                    r.case === fixture.id &&
                    r.renderer === renderer,
                ),
              )
            )
              continue;
            const folder = join(output, name, profile.name, fixture.id);
            await mkdir(folder, { recursive: true });
            let reference, metadata, baselineError;
            const baseline = await context.newPage();
            const locator = (p) =>
              fixture.format === "txt"
                ? p.locator(fixture.selection || "#capture")
                : p
                    .frameLocator("iframe")
                    .locator(fixture.selection || "#capture");
            try {
              await baseline.goto(base);
              await baseline.waitForFunction(() => window.lab);
              metadata = await baseline.evaluate(
                ({ id, p }) => window.lab.setup(id, p.width, p.height),
                { id: fixture.id, p: profile },
              );
              const first = await nativeScreenshot(baseline, fixture, profile);
              const second = await nativeScreenshot(baseline, fixture, profile);
              reference = PNG.sync.read(first);
              const stability = compare(reference, PNG.sync.read(second));
              if (stability.exact)
                throw new Error(
                  "Native reference is unstable: " +
                    stability.exact +
                    " changed pixels",
                );
              await writeFile(join(folder, "reference.png"), first);
              await writeFile(
                join(folder, "reference-padded.png"),
                PNG.sync.write(pad(reference, 32)),
              );
              if (fixture.selection) {
                const snapshot = await baseline.evaluate(() =>
                  window.lab.snapshot(),
                );
                await writeFile(
                  join(folder, "selection-context.json"),
                  JSON.stringify(snapshot, null, 2),
                );
              }
              await writeFile(
                join(folder, "source.json"),
                JSON.stringify(metadata, null, 2),
              );
            } catch (error) {
              baselineError = String(error);
              console.log(
                "BASELINE " +
                  name +
                  "/" +
                  profile.name +
                  "/" +
                  fixture.id +
                  ": " +
                  baselineError,
              );
            } finally {
              await baseline.close();
            }
            for (const renderer of renderers) {
              const row = {
                browser: name,
                profile: profile.name,
                case: fixture.id,
                renderer,
              };
              if (baselineError) {
                records.push({
                  ...row,
                  status: "baseline-error",
                  error: baselineError,
                });
                continue;
              }
              const page = await context.newPage();
              const logs = [];
              page.on("console", (msg) => {
                if (msg.type() === "error" || msg.type() === "warning")
                  logs.push(msg.text().slice(0, 600));
              });
              page.on("pageerror", (error) => logs.push(String(error)));
              page.on("requestfailed", (req) =>
                logs.push(
                  "requestfailed " + req.url() + " " + req.failure()?.errorText,
                ),
              );
              try {
                await page.goto(base);
                await page.waitForFunction(() => window.lab);
                await page.evaluate(
                  ({ id, p }) => window.lab.setup(id, p.width, p.height),
                  { id: fixture.id, p: profile },
                );
                // Match the scroll state introduced by the native element screenshot.
                await locator(page).scrollIntoViewIfNeeded();
                let timer;
                const capture = page.evaluate(
                  ({ renderer, dpr }) => window.lab.capture(renderer, dpr),
                  { renderer, dpr: profile.dpr },
                );
                const result = await Promise.race([
                  capture,
                  new Promise((_, reject) => {
                    timer = setTimeout(
                      () =>
                        reject(new Error("capture-timeout " + timeout + "ms")),
                      timeout,
                    );
                  }),
                ]).finally(() => clearTimeout(timer));
                const png = Buffer.from(result.png.split(",")[1], "base64"),
                  actual = PNG.sync.read(png);
                const passageReference = reference;
                const expected =
                  renderer === "current-png"
                    ? pad(passageReference, 32)
                    : renderer === "current-passage"
                      ? passageReference
                      : reference;
                if (renderer === "current-passage")
                  await writeFile(
                    join(folder, "reference-passage.png"),
                    PNG.sync.write(passageReference),
                  );
                if (renderer === "current-png")
                  await writeFile(
                    join(folder, "reference-padded.png"),
                    PNG.sync.write(expected),
                  );
                const comparison = compare(expected, actual);
                const { diff, ...metrics } = comparison;
                await writeFile(join(folder, renderer + ".png"), png);
                await writeFile(
                  join(folder, renderer + "-diff.png"),
                  PNG.sync.write(diff),
                );
                records.push({
                  ...row,
                  status: "ok",
                  ...metrics,
                  ms: result.ms,
                  logs,
                });
                console.log(
                  name +
                    "/" +
                    profile.name +
                    "/" +
                    fixture.id +
                    " " +
                    renderer +
                    ": " +
                    metrics.percent.toFixed(3) +
                    "%" +
                    (metrics.geometryMatch
                      ? ""
                      : " SIZE " + metrics.expected + " -> " + metrics.actual),
                );
              } catch (error) {
                records.push({
                  ...row,
                  status: "error",
                  error: String(error),
                  logs,
                });
                console.log(
                  name +
                    "/" +
                    profile.name +
                    "/" +
                    fixture.id +
                    " " +
                    renderer +
                    ": ERROR " +
                    String(error).split("\n")[0],
                );
              } finally {
                await page.close();
              }
            }
            await save();
          }
        } finally {
          await context.close();
        }
      }
    } catch (error) {
      browsers.find((b) => b.name === name).runtimeError = String(error);
      console.log(name + " runtime error: " + String(error).split("\n")[0]);
      await save();
    } finally {
      await browser.close();
    }
  }
  console.log(JSON.stringify(await save(), null, 2));
  if (
    !records.some((r) => r.status === "ok") &&
    !boundaries.some((b) => b.results.some((r) => r.passed))
  )
    process.exitCode = 1;
} finally {
  await server.close();
}
