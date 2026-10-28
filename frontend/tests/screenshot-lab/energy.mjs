import { chromium } from "playwright";
import { build } from "../../node_modules/vite/dist/node/index.js";
import { createServer } from "node:http";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { execFileSync, spawn } from "node:child_process";
import { dirname, resolve, join, extname, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { cpus, release, totalmem, loadavg } from "node:os";
import { setTimeout as delay } from "node:timers/promises";

const dir = dirname(fileURLToPath(import.meta.url));
const root = resolve(dir, "../../..");
const frontend = resolve(dir, "../..");
const baseline = process.env.ENERGY_BASE || "1cd039ee4b616ad54a1e1d328abc17ceaa1aaa6f";
const current = execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim();
if (execFileSync("git", ["status", "--porcelain", "--", "frontend/src"], { cwd: root, encoding: "utf8" }).trim()) {
  throw new Error("Commit frontend source changes before measuring revision-to-revision results");
}
execFileSync("git", ["diff", "--quiet", baseline, current, "--", "frontend/package.json", "frontend/package-lock.json", "frontend/vite.config.ts"], { cwd: root });
const output = resolve(process.env.ENERGY_OUTPUT || join(dir, "results-energy", new Date().toISOString().replaceAll(":", "-")));
const binary = process.env.CHROMIUM_EXECUTABLE;
if (!binary) throw new Error("Set CHROMIUM_EXECUTABLE to a Nix-provided Chromium binary");
const nativeFocus = process.env.ENERGY_NATIVE_FOCUS === "1";
const variants = (process.env.ENERGY_VARIANTS || "default").split(",");
const versions = (process.env.ENERGY_VERSIONS || "before,after").split(",");
const tracing = process.env.ENERGY_TRACE === "1";
if (variants.some(v => !["default", "no-progress-animation", "hidden-progress", "hidden-text", "no-wheel-handler"].includes(v)) || versions.some(v => !["before", "after"].includes(v))) throw new Error("Unknown diagnostic variant or version");
const repetitions = Number(process.env.ENERGY_REPETITIONS || 3);
const idleMs = Number(process.env.ENERGY_IDLE_MS || 15000);
const scrollMs = Number(process.env.ENERGY_SCROLL_MS || 6000);
const selectionSteps = Number(process.env.ENERGY_SELECTION_STEPS || 40);
const settingsSteps = Number(process.env.ENERGY_SETTINGS_STEPS || 12);
const formats = (process.env.ENERGY_FORMATS || "epub,txt").split(",");
const scenarios = new Set((process.env.ENERGY_SCENARIOS || "idle,scroll,selection,settings,background-idle").split(","));
for (const value of [repetitions, idleMs, scrollMs, selectionSteps, settingsSteps]) {
  if (!Number.isFinite(value) || value <= 0) throw new Error("Workload sizes must be positive finite numbers");
}
if (formats.some(format => !["epub", "txt"].includes(format)) || [...scenarios].some(scenario => !["idle", "scroll", "selection", "settings", "background-idle"].includes(scenario))) throw new Error("Unknown format or scenario");
for (const value of [repetitions, selectionSteps, settingsSteps]) {
  if (!Number.isInteger(value)) throw new Error("Repetition and operation counts must be integers");
}
const paragraphs = 1000;
const chapterCount = 512;
const records = [];
const skipped = [];
const errors = [];
const servers = [];
await mkdir(output, { recursive: true });
const original = new Map();
const changed = execFileSync("git", ["diff", "--name-only", baseline, current, "--", "frontend/src"], { cwd: root, encoding: "utf8" }).trim().split(String.fromCharCode(10)).filter(Boolean);
for (const path of changed) {
  try { original.set(resolve(root, path), execFileSync("git", ["show", baseline + ":" + path], { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] })); }
  catch { /* New modules are not reachable from the baseline entry. */ }
}
const body = Array.from({ length: paragraphs }, (_, i) => '<p id="p' + i + '">第' + i + '段。窗外的雨渐渐停了，少年合上书页，望向远处的群山。这是一段用于阅读性能比较的合成文字，不包含真实用户数据。</p>').join("");
const chapters = Array.from({ length: chapterCount }, (_, idx) => ({ idx, title: "第" + idx + "章", linear: true }));
const book = { id: "lab", title: "阅读性能合成样本", authors: ["诊断样本"], description: "", cover_url: null, ext: {} };
const file = { id: "lab-file", format: "epub", label: "测试样本", chapter_count: chapterCount, source: "local" };
const toc = chapters.map(chapter => ({ title: chapter.title, idx: chapter.idx, frag: null, children: [] }));
const html = '<!doctype html><html lang="zh-CN"><head><style>body{margin:0}p{margin:0 0 1em}</style></head><body>' + body + '</body></html>';

async function provision(version) {
  const outDir = join(output, version);
  await build({ root: frontend, configFile: join(frontend, "vite.config.ts"), logLevel: "silent", plugins: version === "before" ? [{ name: "baseline-source", enforce: "pre", load(id) { return original.get(id.split("?")[0]); } }] : [], build: { outDir, emptyOutDir: false, sourcemap: tracing } });
  let requests = 0;
  const server = createServer(async (req, res) => {
    try {
      const url = new URL(req.url, "http://localhost");
      const path = url.pathname;
      if (path.startsWith("/api/")) {
        requests++;
        let data;
        if (path === "/api/health") data = { status: "ok", auth_enabled: false };
        else if (path === "/api/auth/me") { res.statusCode = 401; data = { error: "unauthorized" }; }
        else if (/^[/]api[/]shares[/](epub|txt)$/.test(path)) data = { kind: "book", book, file, files: [file], session: null, created_at: "2026-01-01T00:00:00Z", expires_at: null };
        else if (/^[/]api[/]shares[/](epub|txt)[/]book$/.test(path)) data = { book, file, chapters, toc };
        else if (/^[/]api[/]shares[/](epub|txt)[/]chapters[/][0-9]+$/.test(path)) { const parts = path.split("/"); const idx = Number(parts.at(-1)); data = { idx, title: chapters[idx].title, content: parts[3] === "epub" ? html : body }; }
        else { res.statusCode = 404; data = { error: "unexpected fixture endpoint" }; errors.push({ version, path }); }
        res.setHeader("Content-Type", "application/json"); res.end(JSON.stringify(data)); return;
      }
      const target = path.startsWith("/assets/") ? resolve(outDir, "." + path) : join(outDir, "index.html");
      if (!target.startsWith(outDir + sep)) { res.statusCode = 403; res.end(); return; }
      const content = await readFile(target);
      const mime = { ".html": "text/html", ".js": "application/javascript", ".css": "text/css", ".woff2": "font/woff2" }[extname(target)] || "application/octet-stream";
      res.setHeader("Content-Type", mime); res.end(content);
    } catch (error) { res.statusCode = 500; res.end(String(error)); }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  servers.push(server);
  return { version, url: "http://127.0.0.1:" + server.address().port, requests: () => requests };
}

async function capabilities() {
  const rapl = "/sys/devices/virtual/powercap/intel-rapl/intel-rapl:0/energy_uj";
  let energy;
  try { energy = { path: rapl, readable: true, sampleMicrojoules: Number(await readFile(rapl, "utf8")), maxEnergyRangeMicrojoules: Number(await readFile(rapl.replace("energy_uj", "max_energy_range_uj"), "utf8")) }; }
  catch (error) { energy = { path: rapl, readable: false, error: error.code }; }
  return { os: release(), cpu: cpus()[0].model, logicalCpus: cpus().length, memoryBytes: totalmem(), loadAverage: loadavg(), perfEventParanoid: (await readFile("/proc/sys/kernel/perf_event_paranoid", "utf8")).trim(), energy };
}
const report = { status: "running", timestamp: new Date().toISOString(), baseline, current, environment: await capabilities(), protocol: { production: true, headless: true, nativeFocus, hardwareGpu: false, viewport: { width: 1000, height: 800, deviceScaleFactor: 1 }, variants, versions, tracing, repetitions, idleMs, scrollMs, selectionSteps, settingsSteps, formats, scenarios: [...scenarios], paragraphs, chapterCount, measurement: "CDP Performance threadTicks; total browser-process cpuTime; no sampling trace during measurements", powerMeasurement: "Unavailable when RAPL is unreadable; CPU milliseconds are not joules", background: "Only report background measurements when document.visibilityState actually becomes hidden", scope: "Synthetic public Reader with fixture API, no real backend, user data, persistence, or screenshot rasterization" }, records, skipped, errors };

async function snapshot(rootCdp, pageCdp) {
  const metrics = Object.fromEntries((await pageCdp.send("Performance.getMetrics")).metrics.map(m => [m.name, m.value]));
  const processes = (await rootCdp.send("SystemInfo.getProcessInfo")).processInfo;
  const energyMicrojoules = report.environment.energy.readable
    ? Number(await readFile(report.environment.energy.path, "utf8")) : null;
  return { metrics, processes, energyMicrojoules, clock: performance.now() };
}
function difference(before, after) {
  const result = { wallMs: after.clock - before.clock, mainTaskCpuMs: 1000 * (after.metrics.TaskDuration - before.metrics.TaskDuration), scriptCpuMs: 1000 * (after.metrics.ScriptDuration - before.metrics.ScriptDuration), layoutCpuMs: 1000 * (after.metrics.LayoutDuration - before.metrics.LayoutDuration), styleCpuMs: 1000 * (after.metrics.RecalcStyleDuration - before.metrics.RecalcStyleDuration), layouts: after.metrics.LayoutCount - before.metrics.LayoutCount, styleRecalculations: after.metrics.RecalcStyleCount - before.metrics.RecalcStyleCount, heapEndBytes: after.metrics.JSHeapUsedSize, heapDeltaBytes: after.metrics.JSHeapUsedSize - before.metrics.JSHeapUsedSize, browserCpuMs: 0, cpuByProcessTypeMs: {}, processChurn: [] };
  const old = new Map(before.processes.map(p => [p.id, p]));
  for (const p of after.processes) {
    const previous = old.get(p.id);
    const delta = 1000 * (p.cpuTime - (previous?.cpuTime || 0));
    result.browserCpuMs += delta;
    result.cpuByProcessTypeMs[p.type] = (result.cpuByProcessTypeMs[p.type] || 0) + delta;
    if (!previous) result.processChurn.push(p.id);
    old.delete(p.id);
  }
  for (const p of old.values()) result.processChurn.push(p.id);
  result.browserCpuValid = result.processChurn.length === 0;
  if (before.energyMicrojoules !== null && after.energyMicrojoules !== null) {
    const range = report.environment.energy.maxEnergyRangeMicrojoules;
    result.packageEnergyJ = ((after.energyMicrojoules - before.energyMicrojoules + range) % range) / 1e6;
    result.packageAverageW = result.packageEnergyJ * 1000 / result.wallMs;
  }
  return result;
}
async function measure(label, fixture, rootCdp, cdp, scenario, action) {
  if (!scenarios.has(scenario)) return;
  const apiBefore = fixture.requests();
  if (tracing) {
    await cdp.send("Profiler.enable");
    await cdp.send("Profiler.start");
    await rootCdp.send("Tracing.start", { transferMode: "ReturnAsStream", traceConfig: { recordMode: "recordAsMuchAsPossible", includedCategories: ["toplevel", "devtools.timeline", "cc", "gpu", "viz", "blink", "disabled-by-default-devtools.timeline", "disabled-by-default-devtools.timeline.frame"] } });
  }
  const before = await snapshot(rootCdp, cdp);
  const detail = await action();
  const after = await snapshot(rootCdp, cdp);
  if (tracing) {
    const prefix = [label.repetition, label.version, label.format, label.variant, scenario].join("-");
    const profile = await cdp.send("Profiler.stop");
    await writeFile(join(output, prefix + ".cpuprofile"), JSON.stringify(profile.profile));
    const completed = new Promise(resolve => rootCdp.once("Tracing.tracingComplete", resolve));
    await rootCdp.send("Tracing.end");
    const { stream, dataLossOccurred } = await completed;
    if (dataLossOccurred) throw new Error("Trace data was lost");
    let content = "";
    for (;;) { const chunk = await rootCdp.send("IO.read", { handle: stream }); content += chunk.base64Encoded ? Buffer.from(chunk.data, "base64").toString("utf8") : chunk.data; if (chunk.eof) break; }
    await rootCdp.send("IO.close", { handle: stream });
    await writeFile(join(output, prefix + ".trace.json"), content);
  }
  const record = { ...label, scenario, ...difference(before, after), apiRequests: fixture.requests() - apiBefore, detail };
  records.push(record);
  await writeFile(join(output, "results.json"), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(record));
}

let browser;
let nativeProcess;
try {
  const fixtures = { before: await provision("before"), after: await provision("after") };
  if (nativeFocus) {
    nativeProcess = spawn(binary, ["--headless=new", "--no-sandbox", "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "--remote-debugging-port=0", "--user-data-dir=" + join(output, "browser-profile"), "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
    const endpoint = await new Promise((resolve, reject) => {
      let text = "";
      const timer = setTimeout(() => reject(new Error("Chromium debugging endpoint timed out")), 15000);
      nativeProcess.stderr.on("data", data => {
        text += data;
        const match = text.match(/DevTools listening on (ws:[^\s]+)/);
        if (match) { clearTimeout(timer); resolve(match[1]); }
      });
      nativeProcess.once("error", error => { clearTimeout(timer); reject(error); });
      nativeProcess.once("exit", (code, signal) => { clearTimeout(timer); reject(new Error("Chromium exited " + code + " signal=" + signal + ": " + text.slice(-2000))); });
    });
    browser = await chromium.connectOverCDP(endpoint, { noDefaults: true });
  } else {
    browser = await chromium.launch({ executablePath: binary, headless: true, ignoreDefaultArgs: ["--disable-background-timer-throttling", "--disable-backgrounding-occluded-windows", "--disable-renderer-backgrounding"] });
  }
  const rootCdp = await browser.newBrowserCDPSession();
  const gpu = (await rootCdp.send("SystemInfo.getInfo")).gpu;
  report.environment.browser = browser.version();
  report.environment.gpu = { devices: gpu.devices, renderer: gpu.auxAttributes.glRenderer, displayType: gpu.auxAttributes.displayType };
  for (let repetition = 0; repetition < repetitions; repetition++) {
    for (const version of (repetition % 2 ? [...versions].reverse() : versions)) {
      for (const format of formats) for (const variant of [...variants.slice(repetition % variants.length), ...variants.slice(0, repetition % variants.length)]) {
        if (format === "txt" && variant === "no-wheel-handler") continue;
        const fixture = fixtures[version];
        const label = { repetition: repetition + 1, version, format, variant };
        const context = nativeFocus ? browser.contexts()[0] : await browser.newContext({ viewport: { width: 1000, height: 800 }, deviceScaleFactor: 1, locale: "zh-CN" });
        const page = await context.newPage();
        if (nativeFocus) { await page.setViewportSize({ width: 1000, height: 800 }); await page.bringToFront(); }
        if (variant === "no-wheel-handler") await page.addInitScript(() => {
          const add = EventTarget.prototype.addEventListener;
          EventTarget.prototype.addEventListener = function(type, ...args) {
            if (type === "wheel" && window.frameElement?.classList.contains("epub-document")) { window.__skippedWheelHandlers = (window.__skippedWheelHandlers || 0) + 1; return; }
            return Reflect.apply(add, this, [type, ...args]);
          };
        });
        page.on("pageerror", error => errors.push({ ...label, message: String(error) }));
        await page.goto(fixture.url + "/share/" + format + "/read");
        await page.locator(format === "epub" ? "iframe.epub-document" : ".epub-content p").first().waitFor();
        const target = format === "epub" ? await page.locator("iframe.epub-document").elementHandle().then(el => el.contentFrame()) : page;
        await target.waitForFunction(() => document.querySelectorAll("p").length >= 1000);
        await page.evaluate(() => document.fonts.ready);
        await target.evaluate(() => document.fonts.ready);
        await page.waitForLoadState("networkidle");
        await delay(1500);
        if (variant === "no-progress-animation") await page.addStyleTag({ content: ".progress-fill { transition: none !important; }" });
        if (variant === "hidden-progress") await page.addStyleTag({ content: ".progress-bar { visibility: hidden !important; }" });
        if (variant === "hidden-text") await target.evaluate(epub => { const style = document.createElement("style"); style.textContent = (epub ? "body" : ".epub-content") + " { visibility: hidden !important; }"; document.head.append(style); }, format === "epub");
        await delay(300);
        report.fixtureGeometry ??= [];
        report.fixtureGeometry.push({ ...label, geometry: await target.evaluate(epub => { const p = (document.querySelector(".epub-content") || document.body).querySelector("p"); const root = epub ? document.scrollingElement : document.querySelector(".reader-viewport"); const css = getComputedStyle(p); return { skippedWheelHandlers: window.__skippedWheelHandlers || 0, paragraphs: document.querySelectorAll("p").length, scrollHeight: root.scrollHeight, clientHeight: root.clientHeight, fontSize: css.fontSize, lineHeight: css.lineHeight, fontFamily: css.fontFamily }; }, format === "epub") });
        const cdp = await context.newCDPSession(page);
        await cdp.send("Performance.enable", { timeDomain: "threadTicks" });
        await measure(label, fixture, rootCdp, cdp, "idle", async () => { await delay(idleMs); return { visibility: await page.evaluate(() => document.visibilityState) }; });
        const box = await page.locator(format === "epub" ? "iframe.epub-document" : ".reader-viewport").boundingBox();
        await measure(label, fixture, rootCdp, cdp, "scroll", async () => {
          const x = Math.round(box.x + box.width / 2), y = Math.round(box.y + box.height / 2);
          await cdp.send("Input.synthesizeScrollGesture", { x, y, yDistance: -3600, speed: 3600000 / scrollMs, gestureSourceType: "mouse", preventFling: true });
          await delay(400);
          return target.evaluate(epub => ({ scrollTop: epub ? document.scrollingElement.scrollTop : document.querySelector(".reader-viewport").scrollTop }), format === "epub");
        });
        await target.evaluate(epub => { (epub ? document.scrollingElement : document.querySelector(".reader-viewport")).scrollTop = 0; }, format === "epub");
        await delay(500);
        await measure(label, fixture, rootCdp, cdp, "selection", () => target.evaluate(async steps => {
          const root = document.querySelector(".epub-content") || document.body;
          const node = root.querySelector("p").firstChild;
          for (let i = 0; i < steps; i++) {
            const range = document.createRange(); range.setStart(node, 3); range.setEnd(node, 20 + (i % 20));
            const selection = document.getSelection(); selection.removeAllRanges(); selection.addRange(range);
            await new Promise(resolve => setTimeout(resolve, 30));
          }
          return { selections: steps, selectedTextLength: document.getSelection().toString().length };
        }, selectionSteps));
        await target.evaluate(() => document.getSelection()?.removeAllRanges());
        await delay(400);
        await page.getByRole("button", { name: "阅读设置", exact: true }).click();
        const buttons = page.locator(".reader-settings");
        await measure(label, fixture, rootCdp, cdp, "settings", async () => {
          for (let i = 0; i < settingsSteps; i++) {
            await buttons.getByRole("button", { name: i % 2 ? "A−" : "A+", exact: true }).click();
            await target.evaluate(() => document.fonts.ready);
          }
          return { changes: settingsSteps };
        });
        await page.getByRole("button", { name: "关闭", exact: true }).click();
        if (scenarios.has("background-idle")) {
          // Native mode skips Playwright's original focus override. A second CDP
          // client cannot reliably undo that original override in regular mode.
          await cdp.send("Emulation.setFocusEmulationEnabled", { enabled: false });
          const { targetInfo } = await cdp.send("Target.getTargetInfo");
          const other = await rootCdp.send("Target.createTarget", { url: "about:blank", newWindow: false, browserContextId: targetInfo.browserContextId });
          await rootCdp.send("Target.activateTarget", other);
          await delay(500);
          const visibility = await page.evaluate(() => document.visibilityState);
          if (visibility === "hidden") await measure(label, fixture, rootCdp, cdp, "background-idle", async () => { await delay(idleMs); return { visibility, frameVisibility: await target.evaluate(() => document.visibilityState) }; });
          else skipped.push({ ...label, scenario: "background-idle", reason: "Real tab backgrounding did not produce hidden visibility in this browser environment", observedVisibility: visibility });
          await rootCdp.send("Target.closeTarget", other);
        }
        if (nativeFocus) await page.close();
        else await context.close();
      }
    }
  }
  report.status = "completed";
} catch (error) {
  report.status = "failed";
  errors.push({ phase: "fatal", message: String(error) });
  throw error;
} finally {
  if (browser) await browser.close();
  if (nativeProcess) {
    if (nativeProcess.exitCode === null && nativeProcess.signalCode === null) {
      nativeProcess.kill("SIGTERM");
      await new Promise(resolve => nativeProcess.once("exit", resolve));
    }
    report.environment.nativeBrowserExit = { exitCode: nativeProcess.exitCode, signal: nativeProcess.signalCode };
  }
  for (const server of servers) await new Promise(resolve => server.close(resolve));
  await writeFile(join(output, "results.json"), JSON.stringify(report, null, 2));
}
console.log("Results: " + join(output, "results.json"));
if (errors.length) process.exitCode = 1;
