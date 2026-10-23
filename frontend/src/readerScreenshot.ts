/** Extract only a selection wholly contained in the current chapter. */
export function selectedChapterText(root: Element, selection: Selection | null): string {
  if (!selection || selection.isCollapsed || selection.rangeCount !== 1) return "";
  const range = selection.getRangeAt(0);
  if (!root.contains(range.startContainer) || !root.contains(range.endContainer)) return "";
  return selection.toString().replace(/\r\n?/g, "\n").trim();
}

export const MAX_SCREENSHOT_TEXT = 6000;
const WIDTH = 900;
const PADDING = 64;
const MAX_HEIGHT = 12000;

export interface ScreenshotStyle {
  background: string;
  foreground: string;
  muted: string;
  accent: string;
  fontFamily: string;
}

export interface ScreenshotSelection {
  text: string;
  /** Selected EPUB markup with its isolated publication styles. */
  document?: string;
}

/** Snapshot the selected DOM and its publication context before a dialog takes focus. */
export function selectedEpubContent(doc: Document, selection: Selection | null): ScreenshotSelection {
  const text = selectedChapterText(doc.body, selection);
  if (!text || text.length > MAX_SCREENSHOT_TEXT) return { text };
  const range = selection!.getRangeAt(0);
  let parent = range.commonAncestorContainer;
  if (parent.nodeType !== Node.ELEMENT_NODE) parent = parent.parentNode!;
  let contents: Node = range.cloneContents();
  while (parent && parent !== doc.documentElement) {
    const wrapper = parent.cloneNode(false);
    wrapper.appendChild(contents);
    contents = wrapper;
    parent = parent.parentNode!;
  }
  const snapshot = doc.implementation.createHTMLDocument("");
  const html = doc.documentElement.cloneNode(false) as HTMLElement;
  const head = snapshot.createElement("head");
  // Only passive styles are copied; never transfer source scripts, base URLs or metadata.
  for (const style of doc.head.querySelectorAll("style")) head.append(style.cloneNode(true));
  html.append(head, contents);
  snapshot.replaceChild(html, snapshot.documentElement);
  for (const el of snapshot.querySelectorAll("script,iframe,object,embed,link,meta,base,form")) el.remove();
  for (const el of snapshot.querySelectorAll("*")) {
    for (const attr of Array.from(el.attributes)) {
      if (/^on/i.test(attr.name) || ["srcdoc", "autofocus"].includes(attr.name)) el.removeAttribute(attr.name);
    }
  }
  return { text, document: "<!doctype html>" + html.outerHTML };
}

export interface ScreenshotOptions extends ScreenshotSelection {
  title?: string;
  chapter?: string;
  link?: string;
  style: ScreenshotStyle;
}

/** Wrap by grapheme so CJK, emoji and long URLs cannot overflow the card. */
export function wrapScreenshotText(text: string, width: number, measure: (text: string) => number): string[] {
  const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
  return text.split("\n").flatMap((paragraph) => {
    const lines: string[] = [];
    let line = "";
    for (const { segment } of segmenter.segment(paragraph)) {
      if (line && measure(line + segment) > width) {
        lines.push(line);
        line = "";
      }
      line += segment;
    }
    lines.push(line);
    return lines;
  });
}

/** Render a selected passage and optional metadata to an opaque PNG. */
export async function renderReaderScreenshot(options: ScreenshotOptions): Promise<Blob> {
  if (!options.text.trim() || options.text.length > MAX_SCREENSHOT_TEXT) throw new Error("selection-size");
  await document.fonts?.ready;
  const passage = options.document
    ? await (await import("./epubScreenshot")).renderEpubScreenshot(options.document, WIDTH - PADDING * 2, MAX_HEIGHT)
    : null;
  const passageHeight = passage ? Math.ceil(passage.height * Math.min(1, (WIDTH - PADDING * 2) / passage.width)) : 0;
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("canvas-unavailable");
  const { style } = options;
  const sections = await Promise.all([
    { text: options.title, size: 20, lineHeight: 30, color: style.foreground, weight: 600 },
    { text: options.chapter, size: 18, lineHeight: 28, color: style.muted, weight: 400 },
    { text: passage ? undefined : options.text, image: passage, size: 28, lineHeight: 48, color: style.foreground, weight: 400 },
    { text: options.link, size: 20, lineHeight: 30, color: style.muted, weight: 400 },
  ].filter((section) => section.text || section.image).map(async (section) => {
    const font = section.weight + " " + section.size + "px " + style.fontFamily;
    // Canvas does not wait for unused unicode-range subsets to download.
    if (section.text) await document.fonts?.load(font, section.text);
    ctx.font = font;
    return { ...section, font, lines: section.image ? [] : wrapScreenshotText(section.text!, WIDTH - PADDING * 2, (line) => ctx.measureText(line).width) };
  }));
  const height = PADDING * 2 + 16 + sections.reduce((sum, section) => sum + (section.image ? passageHeight : section.lines.length * section.lineHeight) + 24, 0);
  // Never silently truncate long selections or exceed mobile canvas limits.
  if (height > MAX_HEIGHT) throw new Error("selection-size");
  canvas.width = WIDTH;
  canvas.height = height;
  ctx.fillStyle = style.background;
  ctx.fillRect(0, 0, WIDTH, height);
  ctx.fillStyle = style.accent;
  ctx.fillRect(PADDING, PADDING, 48, 4);
  ctx.textBaseline = "top";
  let y = PADDING + 32;
  for (const section of sections) {
    if (section.image) {
      ctx.drawImage(section.image, PADDING, y, Math.min(section.image.width, WIDTH - PADDING * 2), passageHeight);
      y += passageHeight + 24;
      continue;
    }
    ctx.font = section.font;
    ctx.fillStyle = section.color;
    for (const line of section.lines) {
      ctx.fillText(line, PADDING, y);
      y += section.lineHeight;
    }
    y += 24;
  }
  return new Promise((resolve, reject) => canvas.toBlob((blob) => {
    if (blob) resolve(blob);
    else reject(new Error("png-unavailable"));
  }, "image/png"));
}
