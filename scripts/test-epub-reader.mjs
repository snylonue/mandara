// Offline browser regression: intercept every request, never mutate a live DB.
// npm install --prefix target/epub-audit/browser playwright
// PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs CHROMIUM_BIN=... node scripts/test-epub-reader.mjs [exported-book.json ...]
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? "playwright");
const origin = "http://127.0.0.1:8112";
const documents = [
  '<!doctype html><html data-epub-layout="reflowable" lang="zh-CN"><head><style>body{color:rgb(12,34,56)}.nav{display:none}p{text-indent:2em;margin-bottom:2em}.spacer{height:2000px}</style></head><body><a href="#target">脚注</a><p>正文<ruby>字<rt>zì</rt></ruby></p><table><tr><td>表格</td></tr></table><script>window.executed=true;parent.executed=true</script><img src="https://tracker.invalid/track"><div class="spacer"></div><p id="target">目标</p></body></html>',
  '<!doctype html><html data-epub-layout="reflowable"><head><style>body{writing-mode:vertical-rl;height:400px;width:4000px}</style></head><body><p>竖排正文</p><svg viewBox="0 0 10 10"><rect width="10" height="10" fill="red"/></svg></body></html>',
  '<!doctype html><html data-epub-layout="pre-paginated"><head><meta name="viewport" content="width=1000,height=1500"></head><body><svg viewBox="0 0 1000 1500" width="1000" height="1500"><rect width="1000" height="1500" fill="red"/></svg></body></html>',
];
let book = { title: "测试书", chapters: documents.map((content, idx) => ({ idx, title: "章节" + idx, content })), toc: [{ title: "小节", idx: 0, frag: "target", children: [] }] };
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_BIN, headless: true, args: ["--no-sandbox"] });
try {
  const page = await browser.newPage({ viewport: { width: 1100, height: 850 } });
  const unexpected = [];
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.route("**/*", async route => {
    const url = new URL(route.request().url());
    const json = data => route.fulfill({ json: data });
    if (url.origin !== origin) { unexpected.push(url.href); return route.abort(); }
    if (url.pathname === "/api/health") return json({ status: "ok" });
    if (url.pathname === "/api/auth/me") return json({ id: "audit", username: "audit", role: "user" });
    if (url.pathname === "/api/files/fixture/sessions") return json({ sessions: [] });
    if (url.pathname === "/api/files/fixture") return json({ book: { id: "test", title: book.title }, file: { id: "fixture", format: "epub", label: "", chapter_count: book.chapters.length }, chapters: book.chapters.map(({ idx, title, linear }) => ({ idx, title, linear })), toc: book.toc });
    const chapter = url.pathname.match(/^\/api\/files\/fixture\/chapters\/(\d+)$/);
    if (chapter) return json(book.chapters[Number(chapter[1])]);
    if (url.pathname.startsWith("/assets/")) return route.fulfill({ path: path.resolve("frontend/dist", url.pathname.slice(1)) });
    if (url.pathname.startsWith("/read/")) return route.fulfill({ path: path.resolve("frontend/dist/index.html"), contentType: "text/html" });
    return route.fulfill({ status: 404, body: "not in offline fixture" });
  });
  async function open(idx = 0) {
    await page.goto(origin + "/read/fixture?chapter=" + idx);
    const element = page.locator("iframe.epub-document");
    await element.waitFor();
    const frame = await (await element.elementHandle()).contentFrame();
    await frame.waitForFunction(() => !!document.body && document.readyState === "complete");
    return frame;
  }
  let frame = await open();
  assert.equal(await frame.locator("p").first().evaluate(el => getComputedStyle(el).textIndent), "34px");
  assert.equal(await frame.locator("body").evaluate(el => getComputedStyle(el).color), "rgb(12, 34, 56)");
  assert.equal(await page.evaluate(() => window.executed ?? false), false);
  assert.equal(await frame.evaluate(() => window.executed ?? false), false);
  assert.ok(await page.locator("header.nav").isVisible());
  assert.equal(await frame.locator("ruby").count(), 1);
  assert.equal(await frame.locator("table").count(), 1);
  await frame.locator("a").click();
  await frame.waitForFunction(() => document.scrollingElement.scrollTop > 0);
  assert.ok(await frame.evaluate(() => document.getElementById("target").getBoundingClientRect().top < innerHeight));
  frame = await open(1);
  assert.equal(await frame.locator("body").evaluate(el => getComputedStyle(el).writingMode), "vertical-rl");
  await frame.evaluate(() => { document.scrollingElement.scrollLeft = -1000; });
  await page.waitForFunction(() => document.querySelector(".reader-topbar-chapter").textContent.includes("%"));
  assert.ok(await frame.evaluate(() => Math.abs(document.scrollingElement.scrollLeft) > 0));
  frame = await open(2);
  await frame.waitForFunction(() => document.documentElement.style.transform.startsWith("scale("));
  assert.ok(await frame.evaluate(() => document.documentElement.getBoundingClientRect().width <= innerWidth + 1));
  await page.setViewportSize({ width: 600, height: 850 });
  await frame.waitForFunction(() => document.documentElement.getBoundingClientRect().width <= innerWidth + 1);
  console.log("PASS: CSS isolation, CSP, semantics, fragment navigation, vertical scrolling, fixed viewport scaling");
  book = {
    title: "内部链接测试",
    chapters: [
      { idx: 0, title: "第一章", linear: true, content: '<!doctype html><html><body><a href="epub:chapter/2#note%20one">注释</a></body></html>' },
      { idx: 1, title: "第二章", linear: true, content: '<!doctype html><html><body>第二章</body></html>' },
      { idx: 2, title: "注释", linear: false, content: '<!doctype html><html><body><div style="height:2000px"></div><p id="note one">注释正文<a href="epub:chapter/0">返回</a></p></body></html>' },
    ], toc: [{ title: "注释目录", idx: 2, frag: "note one", children: [] }],
  };
  frame = await open();
  await frame.locator("a").click();
  await page.locator('iframe[title="注释"]').waitFor();
  frame = await (await page.locator("iframe.epub-document").elementHandle()).contentFrame();
  await frame.waitForFunction(() => document.getElementById("note one")?.getBoundingClientRect().top < innerHeight);
  await frame.getByText("返回", { exact: true }).click();
  await page.locator('iframe[title="第一章"]').waitFor();
  await page.locator(".reader-nav-btn.next").click();
  await page.locator('iframe[title="第二章"]').waitFor();
  assert.ok(await page.locator(".reader-nav-btn.next").isDisabled());
  console.log("PASS: cross-document footnote/backlink, decoded fragments and linear-only next order");
  for (const file of process.argv.slice(2)) {
    book = JSON.parse(await fs.readFile(file, "utf8"));
    let occurrences = 0;
    for (const chapter of book.chapters) {
      frame = await open(chapter.idx);
      assert.ok(await frame.locator("body").count());
      await frame.waitForFunction(() => Array.from(document.images).filter(i => i.getAttribute("src")).every(i => i.complete));
      const images = await frame.locator("img").evaluateAll(images => images.map(img => ({ loaded: img.complete && img.naturalWidth > 0, src: img.getAttribute("src") })));
      assert.ok(images.filter(i => i.src).every(i => i.loaded), "broken images: " + file + " chapter=" + chapter.idx);
      occurrences += images.filter(i => i.src).length;
      if (chapter.content.includes("data-mandara-compat=")) {
        assert.equal(await frame.locator("html").evaluate(el => getComputedStyle(el).writingMode), "vertical-rl");
      }
    }
    console.log("PASS: corpus", path.basename(file), "all-chapters=" + book.chapters.length, "image-occurrences=" + occurrences);
  }
  assert.deepEqual(unexpected, []);
  assert.deepEqual(errors, []);
} finally { await browser.close(); }
