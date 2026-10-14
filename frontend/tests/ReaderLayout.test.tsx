import { afterEach, beforeEach, expect, it } from "vitest";
import { readFileSync } from "node:fs";

const layout = readFileSync("src/styles/layout.css", "utf8");
const readerStyles = readFileSync("src/styles/pages/reader.css", "utf8");
let style: HTMLStyleElement;
let root: HTMLDivElement;
beforeEach(() => {
  style = document.createElement("style");
  style.textContent = layout + readerStyles;
  document.head.append(style);
  root = document.createElement("div");
  root.id = "root";
  root.innerHTML = `<div class="app app-reader"><header class="nav"></header><main class="content">
    <div class="reader-page"><div class="reader"><header class="reader-topbar"></header>
    <div class="reader-viewport reader-viewport-document"><article class="reader-body reader-body-document">
    <iframe class="epub-document"></iframe></article></div><footer class="reader-bottom"></footer>
    </div></div></main></div>`;
  document.body.append(root);
});
afterEach(() => { root.remove(); style.remove(); });

it("bounds the whole reading shell instead of inheriting the shelf 100vh minimum", () => {
  for (const element of [document.documentElement, document.body, root]) {
    const css = getComputedStyle(element);
    expect(css.height).toBe("100dvh");
    expect(css.overflow).toBe("hidden");
    expect(css.overscrollBehavior).toBe("none");
  }
  for (const selector of [".app-reader", ".content", ".reader-page", ".reader", ".reader-viewport"]) {
    expect(parseFloat(getComputedStyle(root.querySelector(selector)!).minHeight)).toBe(0);
  }
  expect(getComputedStyle(root.querySelector(".content")!).display).toBe("flex");
  expect(getComputedStyle(root.querySelector(".reader-viewport")!).overflow).toBe("hidden");
});

it("leaves one text scroll container and restores normal page scrolling outside the reader", () => {
  const viewport = root.querySelector(".reader-viewport")!;
  viewport.classList.remove("reader-viewport-document");
  expect(getComputedStyle(viewport).overflow).toBe("auto");
  root.querySelector(".app")!.classList.remove("app-reader");
  for (const element of [document.documentElement, document.body, root]) {
    expect(getComputedStyle(element).overflow).not.toBe("hidden");
    expect(getComputedStyle(element).height).not.toBe("100dvh");
  }
  expect(getComputedStyle(root.querySelector(".app")!).minHeight).toBe(`${window.innerHeight}px`);
});
