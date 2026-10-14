import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Reader } from "../src/components/Reader";
import { readFileSync } from "node:fs";

// Load real styles rather than Vitest's stubbed CSS modules.
const tokens = readFileSync("src/styles/tokens.css", "utf8");
const readerStyles = readFileSync("src/styles/pages/reader.css", "utf8");

let style: HTMLStyleElement;
beforeEach(() => {
  localStorage.clear();
  HTMLElement.prototype.scrollTo = vi.fn();
  style = document.createElement("style");
  style.textContent = tokens + readerStyles;
  document.head.append(style);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
});
afterEach(() => {
  style.remove();
  delete (HTMLElement.prototype as Partial<HTMLElement>).scrollTo;
  delete document.documentElement.dataset.theme;
  localStorage.clear();
  vi.unstubAllGlobals();
});

it.each(["light", "dark"])("keeps reading themes independent from the %s app theme", async (appTheme) => {
  document.documentElement.dataset.theme = appTheme;
  // The default reading theme is night, even on a light-themed shelf.
  const chapter = { idx: 0, title: "正文", content: "<p>测试正文</p>" };
  const { container } = render(<Reader title="测试书籍" chapters={[{ idx: 0, title: "正文", linear: true }]}
    loadChapter={async () => chapter} />);
  await screen.findByText("测试正文");
  const article = container.querySelector(".reader-body")!;
  expect(container.querySelector(".reader")!.getAttribute("data-theme")).toBe("dark");
  expect(getComputedStyle(article).backgroundColor).toBe("rgb(15, 17, 21)");
  expect(getComputedStyle(article).getPropertyValue("--fg").trim()).toBe("#e6e9ef");
  expect(getComputedStyle(article).colorScheme).toBe("dark");

  fireEvent.click(screen.getByRole("button", { name: "阅读设置" }));
  for (const [name, theme, background, foreground] of [
    ["羊皮纸", "sepia", "rgb(244, 236, 216)", "#43382a"],
    ["白昼", "light", "rgb(251, 251, 253)", "#333942"],
    ["夜间", "dark", "rgb(15, 17, 21)", "#e6e9ef"],
  ]) {
    fireEvent.click(screen.getByRole("button", { name }));
    await waitFor(() => expect(container.querySelector(".reader")!.getAttribute("data-theme")).toBe(theme));
    expect(getComputedStyle(article).backgroundColor).toBe(background);
    expect(getComputedStyle(article).getPropertyValue("--fg").trim()).toBe(foreground);
    expect(getComputedStyle(article).colorScheme).toBe(theme === "dark" ? "dark" : "light");
    const chrome = getComputedStyle(container.querySelector(".reader")!);
    expect(chrome.colorScheme).toBe(theme === "dark" ? "dark" : "light");
    expect(chrome.getPropertyValue("--bg").trim()).toBe(
      theme === "dark" ? "#0f1115" : theme === "sepia" ? "#f4ecd8" : "#efede4");
    expect(chrome.getPropertyValue("--surface").trim()).toBe(
      theme === "dark" ? "#171a21" : theme === "sepia" ? "#f8f1e3" : "#fcfbf6");
    expect(chrome.getPropertyValue("--surface-raised").trim()).toBe(
      theme === "dark" ? "#1b1f28" : theme === "sepia" ? "#fcf6e9" : "#fffefb");
    expect(getComputedStyle(document.documentElement).getPropertyValue("--bg").trim()).toBe(
      appTheme === "dark" ? "#0f1115" : "#efede4");
    expect(document.documentElement.dataset.theme).toBe(appTheme);
    expect(JSON.parse(localStorage.getItem("mandara_reader_settings")!).theme).toBe(theme);
  }
});
