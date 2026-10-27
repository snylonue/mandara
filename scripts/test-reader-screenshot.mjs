// Offline pixel regression; no application server or database is accessed.
// npm install --prefix target/screenshot-audit/browser playwright
// PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs CHROMIUM_BIN=... node scripts/test-reader-screenshot.mjs
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { build } from "../frontend/node_modules/vite/dist/node/index.js";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? "playwright");
const output = path.resolve("target/screenshot-regression");
await fs.mkdir(output, { recursive: true });
const entry = path.join(output, "entry.ts");
const bundle = await build({
  configFile: false, root: path.resolve("frontend"), publicDir: false, logLevel: "warn",
  plugins: [{
    name: "screenshot-regression-entry",
    resolveId(id) { if (id === entry) return id; },
    load(id) {
      if (id === entry) return 'import * as reader from ' + JSON.stringify(path.resolve("frontend/src/readerScreenshot.ts")) + '; window.reader = reader;';
    },
  }],
  build: { write: false, minify: false, lib: { entry, formats: ["es"], fileName: "regression" }, rolldownOptions: { output: { codeSplitting: false } } },
});
const script = (Array.isArray(bundle) ? bundle : [bundle]).flatMap(result => result.output).find(file => file.type === "chunk").code;
const fontRoot = path.resolve("frontend/node_modules/@fontsource-variable/noto-serif-sc");
const fonts = (await fs.readFile(path.join(fontRoot, "index.css"), "utf8")).replaceAll("./files/", "http://screenshot.test/fonts/");
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_BIN, headless: true, args: ["--no-sandbox"] });
try {
  const page = await browser.newPage({ viewport: { width: 1000, height: 850 }, deviceScaleFactor: 2 });
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.route("**/*", async route => {
    const url = new URL(route.request().url());
    assert.equal(url.origin, "http://screenshot.test");
    if (url.pathname === "/") return route.fulfill({ contentType: "text/html", body: '<!doctype html><body style="margin:0;background:#f4edd9"><script type="module" src="/regression.js"></script>' });
    if (url.pathname === "/regression.js") return route.fulfill({ contentType: "text/javascript", body: script });
    if (url.pathname.startsWith("/fonts/")) return route.fulfill({ path: path.join(fontRoot, "files", path.basename(url.pathname)) });
    throw new Error("Unexpected request: " + url.href);
  });
  await page.goto("http://screenshot.test/");
  await page.waitForFunction(() => !!window.reader);
  for (const mode of ["horizontal-tb", "vertical-rl"]) for (const count of [0, 60, 1000]) {
    const label = mode + "-" + count;
    const bounds = await page.evaluate(async ({ fonts, mode, count }) => {
      const frame = document.createElement("iframe");
      frame.style.cssText = "display:block;border:0;width:748px;height:800px";
      const text = "这是用于验证选段截图的正文，字体和行距应当保持不变。";
      const markup = "<p>" + text + "</p>";
      frame.srcdoc = '<!doctype html><html><head><style>' + fonts
        + 'html{font:17px/1.75 "Noto Serif SC Variable",serif;color:#43382a;writing-mode:' + mode + '}body{margin:0;padding:8.5px}p{margin:17px}'
        + '</style></head><body>' + markup.repeat(count) + '<section><p>' + text.repeat(3) + '</p><p style="opacity:.4">' + text.repeat(3) + '</p></section></body></html>';
      await new Promise(resolve => { frame.onload = resolve; document.body.append(frame); });
      const doc = frame.contentDocument;
      await doc.fonts.ready;
      await doc.fonts.load('17px "Noto Serif SC Variable"', text);
      const selected = doc.querySelector("section");
      // Reach the excerpt in both physical writing axes without changing its layout.
      selected.scrollIntoView({ block: "end" });
      const range = doc.createRange();
      range.setStart(selected.firstElementChild.firstChild, 0);
      range.setEnd(selected.lastElementChild.firstChild, selected.lastElementChild.firstChild.length);
      doc.getSelection().addRange(range);
      window.excerpt = window.reader.selectedEpubContent(doc, doc.getSelection());
      doc.getSelection().removeAllRanges();
      window.sourceFrame = frame;
      const rect = range.getBoundingClientRect(), host = frame.getBoundingClientRect();
      return { x: host.x + Math.floor(rect.left), y: host.y + Math.floor(rect.top), width: Math.ceil(rect.right) - Math.floor(rect.left), height: Math.ceil(rect.bottom) - Math.floor(rect.top) };
    }, { fonts, mode, count });
    const native = await page.screenshot({ clip: bounds, path: path.join(output, label + "-native.png") });
    const result = await page.evaluate(async native => {
      const blob = await window.reader.renderReaderScreenshot({ ...window.excerpt, style: { background: "#f4edd9", foreground: "#43382a", muted: "#8a7a5f", accent: "#8b6a3f", fontFamily: '"Noto Serif SC Variable",serif' } });
      const exported = await createImageBitmap(blob);
      const reference = await createImageBitmap(await (await fetch("data:image/png;base64," + native)).blob());
      const canvas = document.createElement("canvas");
      canvas.width = reference.width; canvas.height = reference.height;
      const ctx = canvas.getContext("2d");
      ctx.drawImage(reference, 0, 0);
      const expected = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(exported, 32, 32, canvas.width, canvas.height, 0, 0, canvas.width, canvas.height);
      const actual = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
      let error = 0, expectedInk = 0, actualInk = 0;
      for (let i = 0; i < expected.length; i += 4) {
        error += Math.abs(expected[i] - actual[i]);
        expectedInk += Math.max(0, 244 - expected[i]);
        actualInk += Math.max(0, 244 - actual[i]);
      }
      reference.close(); exported.close();
      const png = await new Promise(resolve => { const reader = new FileReader(); reader.onload = () => resolve(reader.result); reader.readAsDataURL(blob); });
      window.sourceFrame.remove();
      return { meanError: error / (canvas.width * canvas.height * 255), inkRatio: actualInk / expectedInk, png };
    }, native.toString("base64"));
    await fs.writeFile(path.join(output, label + "-export.png"), Buffer.from(result.png.split(",")[1], "base64"));
    console.log(label, JSON.stringify({ meanError: result.meanError, inkRatio: result.inkRatio }));
    assert.ok(result.meanError < 0.005, label + ": exported text shifted relative to native paint");
    assert.ok(result.inkRatio > 0.94 && result.inkRatio < 1.06, label + ": selected glyphs were clipped or extra text leaked");
  }
  assert.deepEqual(errors, []);
  console.log("PASS: native/export pixels agree across long horizontal and vertical chapters");
} finally {
  await browser.close();
}
