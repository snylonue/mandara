import { afterEach, beforeEach, expect, it } from "vitest";
import { readFileSync } from "node:fs";

const css = ["components.css", "pages/detail.css", "pages/misc.css"]
  .map((name) => readFileSync(`src/styles/${name}`, "utf8")).join("\n");
let style: HTMLStyleElement;
let root: HTMLDivElement;

beforeEach(() => {
  style = document.createElement("style");
  style.textContent = css;
  document.head.append(style);
  root = document.createElement("div");
  root.innerHTML = `<div class="detail-info">
    <div><a class="tag lg" href="/series/test"><svg width="13"></svg><span>系列《这是一个很长的系列名称》 第四卷</span></a></div>
    <h1>这是一个很长的书名</h1>
    <table class="ext-table"><tbody><tr><th>译者</th><td><span class="tag">很长的译者名称</span></td></tr></tbody></table>
  </div><span class="tag outside">epub</span>`;
  document.body.append(root);
});

afterEach(() => { root.remove(); style.remove(); });

it("allows long detail tags to wrap within their container", () => {
  for (const tag of root.querySelectorAll(".detail-info .tag")) {
    const computed = getComputedStyle(tag);
    expect(computed.whiteSpace).toBe("normal");
    expect(computed.maxWidth).toBe("100%");
    expect(computed.minWidth).toBe("0px");
  }
  expect(getComputedStyle(root.querySelector(".tag > span")!).minWidth).toBe("0px");
  expect(getComputedStyle(root.querySelector("svg")!).flexShrink).toBe("0");
});

it("permits emergency breaks in detail text without changing other tags", () => {
  const info = getComputedStyle(root.querySelector(".detail-info")!);
  expect(info.overflowWrap).toBe("anywhere");
  expect(info.minWidth).toBe("0px");
  expect(getComputedStyle(root.querySelector(".outside")!).whiteSpace).toBe("nowrap");
});
