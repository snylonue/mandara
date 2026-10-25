import React from "react";
import { createRoot } from "react-dom/client";
import { flushSync } from "react-dom";
import { EpubDocument } from "../../src/components/EpubDocument";
import {
  selectedEpubContent,
  selectedTextContent,
  renderReaderScreenshot,
} from "../../src/readerScreenshot";
import { renderEpubScreenshot } from "../../src/epubScreenshot";
import { toCanvas } from "html-to-image";
import { snapdom } from "@zumer/snapdom";
import { domToCanvas } from "modern-screenshot";
import domtoimage from "dom-to-image-more";
import html2canvas from "html2canvas";
import "../../src/styles/index.css";
import { fixtures, baseCSS } from "./fixtures.mjs";
import { checkBoundaries } from "./boundaries";

const root = createRoot(document.getElementById("root")!);
const settings = {
  fontSize: 18,
  lineHeight: 1.8,
  theme: "light",
  font: "serif",
  widthEm: 48,
} as const;
let current: any;
let doc: Document;
let target: HTMLElement;
let excerpt: any;
const bitmap = async (blob: Blob) => {
  const url = URL.createObjectURL(blob);
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    const canvas = document.createElement("canvas");
    canvas.width = img.naturalWidth;
    canvas.height = img.naturalHeight;
    canvas.getContext("2d")!.drawImage(img, 0, 0);
    return canvas;
  } finally {
    URL.revokeObjectURL(url);
  }
};
const settle = async (document: Document) => {
  await document.fonts.ready;
  await Promise.all(
    Array.from(document.images, (img) =>
      img.complete
        ? Promise.resolve()
        : new Promise<void>((resolve) => {
            img.onload = () => resolve();
            img.onerror = () => resolve();
          }),
    ),
  );
  await new Promise<void>((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
  );
};
const descriptor = (el: Element) => {
  const s = getComputedStyle(el),
    r = el.getBoundingClientRect();
  return {
    tag: el.tagName,
    id: el.id,
    text: el.textContent?.slice(0, 80),
    width: r.width,
    height: r.height,
    font: s.font,
    fontFamily: s.fontFamily,
    color: s.color,
    background: s.backgroundColor,
    before: getComputedStyle(el, "::before").content,
  };
};

(window as any).lab = {
  boundaries: checkBoundaries,
  fixtures: fixtures.map(({ id, selection, format, broken }: any) => ({
    id,
    selection,
    format,
    broken,
  })),
  async setup(id: string, width: number, height: number) {
    current = fixtures.find((f: any) => f.id === id);
    const content =
      '<!doctype html><html lang="zh-CN" xmlns:epub="http://www.idpf.org/2007/ops"><head><style>' +
      baseCSS +
      (current.css || "") +
      '</style></head><body><div id="capture">' +
      current.body +
      "</div></body></html>";
    const hostStyle = {
      position: "fixed",
      left: 0,
      top: 0,
      width,
      height,
      fontSize: 18,
      lineHeight: 1.8,
      fontFamily: '"Noto Serif SC Variable",serif',
      background: "white",
      color: "#222",
      "--fg": "#222",
      "--bg": "#fff",
    } as React.CSSProperties;
    if (current.format === "txt") {
      flushSync(() =>
        root.render(
          <div className="reader theme-light" style={hostStyle}>
            <article
              className="reader-body font-serif"
              style={{
                margin: 0,
                padding: 0,
                width,
                maxWidth: "none",
                fontSize: 18,
                lineHeight: 1.8,
              }}
            >
              <div
                className="epub-content"
                id="capture"
                style={{
                  background: "white",
                  color: "#222",
                  display: "flow-root",
                }}
                dangerouslySetInnerHTML={{ __html: current.body }}
              />
            </article>
          </div>,
        ),
      );
      doc = document;
    } else {
      await new Promise<void>((resolve) =>
        flushSync(() =>
          root.render(
            <div className="reader theme-light" style={hostStyle}>
              <EpubDocument
                key={id}
                content={content}
                title="测试章节"
                settings={settings}
                onFraction={() => {}}
                onReady={resolve}
                onNavigate={() => {}}
              />
            </div>,
          ),
        ),
      );
      doc = document.querySelector("iframe")!.contentDocument!;
    }
    await settle(doc);
    target = doc.querySelector(current.selection || "#capture")!;
    // Integer fixture bounds avoid native clip rounding being mistaken for layout drift.
    target.style.minHeight =
      Math.ceil(target.getBoundingClientRect().height) + "px";
    // Snapshot before opening a dialog or moving scroll position, as the reader does.
    const range = doc.createRange();
    if (current.selection) range.selectNode(target);
    else range.selectNodeContents(target);
    if (current.format === "txt") excerpt = selectedTextContent(target, range);
    else {
      const selection = doc.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);
      excerpt = selectedEpubContent(doc, selection);
      selection.removeAllRanges();
    }
    return {
      viewport: excerpt.viewport,
      target: descriptor(target),
      elements: [target, ...target.querySelectorAll("*")].map(descriptor),
      textLength: excerpt.text.length,
      fonts: Array.from(doc.fonts)
        .filter((f) => f.status === "loaded")
        .map((f) => ({ family: f.family, weight: f.weight, status: f.status })),
    };
  },
  async capture(name: string, dpr: number) {
    if (name.endsWith("-snapshot")) {
      const sourceTarget = target;
      const frame = document.createElement("iframe");
      frame.setAttribute("sandbox", "allow-same-origin");
      frame.style.cssText =
        "position:fixed;left:-10000px;top:0;border:0;width:" +
        excerpt.viewport.width +
        "px;height:" +
        excerpt.viewport.height +
        "px";
      try {
        await new Promise<void>((resolve) => {
          frame.onload = () => resolve();
          frame.srcdoc = excerpt.document;
          document.body.append(frame);
        });
        await settle(frame.contentDocument!);
        target = frame.contentDocument!.querySelector(
          current.selection || "#capture",
        )!;
        return await (window as any).lab.capture(name.slice(0, -9), dpr);
      } finally {
        target = sourceTarget;
        frame.remove();
      }
    }
    const start = performance.now();
    let canvas: HTMLCanvasElement;
    const rect = target.getBoundingClientRect();
    if (name === "html-to-image")
      canvas = await toCanvas(target, { pixelRatio: dpr });
    else if (name === "snapdom" || name === "snapdom-reconcile")
      canvas = await snapdom.toCanvas(target, {
        embedFonts: true,
        dpr,
        scale: 1,
        reconcile: name === "snapdom-reconcile",
      });
    else if (name === "modern-screenshot")
      canvas = await domToCanvas(target, { scale: dpr, timeout: 10000 });
    else if (name === "dom-to-image-more")
      canvas = await domtoimage.toCanvas(target, { scale: dpr });
    else if (name === "html2canvas")
      canvas = await html2canvas(target, {
        scale: dpr,
        backgroundColor: "#fff",
        logging: false,
        useCORS: true,
      });
    else if (name === "current-passage")
      canvas = await renderEpubScreenshot(
        excerpt.document,
        excerpt.viewport.width,
        12000,
        excerpt.viewport.height,
      );
    else if (name === "current-png")
      canvas = await bitmap(
        await renderReaderScreenshot({
          ...excerpt,
          style: {
            background: "#fff",
            foreground: "#222",
            muted: "#666",
            accent: "#287",
            fontFamily: '"Noto Serif SC Variable",serif',
          },
        }),
      );
    else throw new Error("unknown-renderer");
    // PNGs are composited over the same opaque background, not visually normalized.
    const output = document.createElement("canvas");
    output.width = canvas.width;
    output.height = canvas.height;
    const ctx = output.getContext("2d")!;
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, output.width, output.height);
    ctx.drawImage(canvas, 0, 0);
    return {
      png: output.toDataURL("image/png"),
      width: canvas.width,
      height: canvas.height,
      ms: performance.now() - start,
      cssWidth: rect.width,
      cssHeight: rect.height,
    };
  },
  async snapshot() {
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-same-origin");
    frame.style.cssText =
      "position:fixed;left:0;top:0;border:0;width:" +
      excerpt.viewport.width +
      "px;height:" +
      excerpt.viewport.height +
      "px";
    await new Promise<void>((resolve) => {
      frame.onload = () => resolve();
      frame.srcdoc = excerpt.document;
      document.body.append(frame);
    });
    await settle(frame.contentDocument!);
    const selected = frame.contentDocument!.querySelector<HTMLElement>(
      current.selection || "#capture",
    )!;
    const original = descriptor(target);
    const restored = descriptor(selected);
    return { original, restored };
  },
};
