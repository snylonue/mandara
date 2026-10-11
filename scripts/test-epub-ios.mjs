// Build frontend/dist first. Run with PLAYWRIGHT_MODULE and BROWSER_BIN set.
// READER_BROWSER selects chromium (default) or webkit. Every request is mocked.
import assert from "node:assert/strict";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";

const engines = await import(process.env.PLAYWRIGHT_MODULE ?? "playwright");
const engine = process.env.READER_BROWSER ?? "chromium";
const browser = await engines[engine].launch({
  executablePath: process.env.BROWSER_BIN,
  headless: true,
  args: engine === "chromium" ? ["--no-sandbox"] : [],
});
const origin = "http://127.0.0.1:8112";
const paragraphs = '<p>这是用于验证兼容排版能从章节开头读到末尾的正文。<ruby>字<rt>zì</rt></ruby></p>'.repeat(150);
const chapters = [
  { idx: 0, title: "空样式表正文", content: '<!doctype html><html data-epub-layout="reflowable" class="vrtl"><head><title>空样式表正文</title><style data-mandara-compat="empty-css-vrtl">:where(html){writing-mode:vertical-rl;text-orientation:mixed}</style></head><body><a href="#last">跳到末尾</a><p id="first" onclick="parent.handlerRan=true">正文开头</p>' + paragraphs + '<p id="last">正文末尾</p><script>parent.executed=true</script><script src="https://tracker.invalid/script.js"></script><img src="https://tracker.invalid/image.png"></body></html>' },
  { idx: 1, title: "原书样式", content: '<!doctype html><html data-epub-layout="reflowable"><head><title>原书样式</title><style>html{writing-mode:vertical-rl}p{color:rgb(12,34,56)}</style></head><body><p>原书竖排正文</p></body></html>' },
];
const profiles = [
  { name: "iPhone", userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1", platform: "iPhone", touch: 5, mode: "horizontal-tb" },
  { name: "iPad", userAgent: "Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1", platform: "iPad", touch: 5, mode: "horizontal-tb" },
  { name: "iPad desktop mode", userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) AppleWebKit/605.1.15 Version/18.0 Safari/605.1.15", platform: "MacIntel", touch: 5, mode: "horizontal-tb" },
  { name: "macOS", userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) AppleWebKit/605.1.15 Version/18.0 Safari/605.1.15", platform: "MacIntel", touch: 0, mode: "vertical-rl" },
  { name: "Android", userAgent: "Mozilla/5.0 (Linux; Android 15) AppleWebKit/537.36 Chrome/140.0.0.0 Mobile Safari/537.36", platform: "Linux armv8l", touch: 5, mode: "vertical-rl" },
];
async function waitInFrame(frame, predicate, arg) {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    if (await frame.evaluate(predicate, arg)) return;
    await delay(50);
  }
  throw new Error("Sandboxed frame condition timed out");
}
try {
  for (const profile of profiles) {
    const context = await browser.newContext({ viewport: { width: 1194, height: 728 }, userAgent: profile.userAgent, hasTouch: profile.touch > 0 });
    await context.addInitScript(({ platform, touch }) => {
      Object.defineProperty(navigator, "platform", { get: () => platform });
      Object.defineProperty(navigator, "maxTouchPoints", { get: () => touch });
    }, profile);
    const page = await context.newPage();
    const errors = [], unexpected = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.route("**/*", async route => {
      const url = new URL(route.request().url());
      if (url.origin !== origin) { unexpected.push(url.href); return route.abort(); }
      const json = data => route.fulfill({ json: data });
      if (url.pathname === "/api/health") return json({ status: "ok" });
      if (url.pathname === "/api/auth/me") return json({ id: "test", username: "test", role: "user" });
      if (url.pathname === "/api/files/fixture/sessions") return json({ sessions: [] });
      if (url.pathname === "/api/files/fixture") return json({ book: { id: "test", title: "测试书" }, file: { id: "fixture", format: "epub", chapter_count: chapters.length }, chapters: chapters.map(({ idx, title }) => ({ idx, title, linear: true })), toc: [{ title: "正文末尾", idx: 0, frag: "last", children: [] }] });
      const chapter = url.pathname.match(new RegExp("^/api/files/fixture/chapters/([0-9]+)$"));
      if (chapter) return json(chapters[Number(chapter[1])]);
      if (url.pathname.startsWith("/assets/")) return route.fulfill({ path: path.resolve("frontend/dist", url.pathname.slice(1)) });
      if (url.pathname.startsWith("/read/")) return route.fulfill({ path: path.resolve("frontend/dist/index.html"), contentType: "text/html" });
      unexpected.push(url.href);
      return route.fulfill({ status: 404 });
    });
    async function frame() {
      const element = page.locator("iframe.epub-document");
      await element.waitFor();
      const result = await (await element.elementHandle()).contentFrame();
      await waitInFrame(result, title => document.title === title && document.readyState === "complete" && document.head.querySelectorAll("style").length > 0, await element.getAttribute("title"));
      return result;
    }
    await page.goto(origin + "/read/fixture");
    let doc = await frame();
    assert.equal(await doc.locator("body").evaluate(el => getComputedStyle(el).writingMode), profile.mode, profile.name);
    assert.equal(await doc.locator("ruby").count(), 150);
    assert.equal(await page.evaluate(() => window.executed ?? false), false);
    await doc.locator("#first").click();
    assert.equal(await page.evaluate(() => window.handlerRan ?? false), false);
    await doc.getByText("跳到末尾", { exact: true }).click();
    await waitInFrame(doc, () => {
      const r = document.getElementById("last").getBoundingClientRect();
      return r.left >= 0 && r.right <= innerWidth + 1 && r.top >= 0 && r.bottom <= innerHeight + 1;
    });
    assert.equal(await doc.evaluate(() => location.hash), "#last");
    assert.ok(await doc.evaluate(() => Math.abs(document.scrollingElement.scrollLeft) + document.scrollingElement.scrollTop > 0));
    await page.locator(".reader-nav-btn.next").click();
    await page.locator('iframe[title="原书样式"]').waitFor();
    doc = await frame();
    assert.equal(await doc.locator("body").evaluate(el => getComputedStyle(el).writingMode), "vertical-rl");
    assert.equal(await doc.locator("p").evaluate(el => getComputedStyle(el).color), "rgb(12, 34, 56)");
    await page.locator(".reader-nav-btn").first().click();
    await page.locator('iframe[title="空样式表正文"]').waitFor();
    doc = await frame();
    assert.equal(await doc.locator("body").evaluate(el => getComputedStyle(el).writingMode), profile.mode);
    assert.equal(await doc.evaluate(() => document.scrollingElement.scrollTop), 0);
    assert.equal(await doc.evaluate(() => document.scrollingElement.scrollLeft), 0);
    await page.getByRole("button", { name: "目录", exact: true }).click();
    await page.locator(".toc-link").click();
    await waitInFrame(doc, () => {
      const r = document.getElementById("last").getBoundingClientRect();
      return r.left >= 0 && r.right <= innerWidth + 1 && r.top >= 0 && r.bottom <= innerHeight + 1;
    });
    assert.deepEqual(errors, []);
    assert.deepEqual(unexpected, []);
    console.log("PASS", engine, profile.name, "direction, complete text, fragment/TOC, chapter return, publisher CSS and CSP");
    await context.close();
  }
} finally { await browser.close(); }
