import { useEffect, useImperativeHandle, useMemo, useRef, type Ref } from "react";
import type { ReaderSettings } from "./ReaderSettings";
import { selectedChapterText, selectedTextContent, type ScreenshotSelection } from "../readerScreenshot";
import { readerImageAt, type ReaderImage } from "../readerImage";
import imagePageStyles from "../styles/epub-image-page.css?raw";
import serifFontStyles from "@fontsource-variable/noto-serif-sc/index.css?inline";

export interface EpubDocumentHandle {
  scrollToFragment: (fragment: string) => void;
  restoreFraction: (fraction: number) => void;
  captureSelection: () => ScreenshotSelection;
}

/** Never insert publisher CSS into the application DOM. */
export function EpubDocument({ content, title, settings, onFraction, onReady, onNavigate, onSelectionChange, onImageOpen, ref }: {
  content: string;
  title: string;
  settings: ReaderSettings;
  onFraction: (fraction: number) => void;
  onReady: () => void;
  onNavigate: (idx: number, fragment?: string) => void;
  onSelectionChange?: (selection: ScreenshotSelection) => void;
  onImageOpen?: (image: ReaderImage) => void;
  ref?: Ref<EpubDocumentHandle>;
}) {
  const frame = useRef<HTMLIFrameElement>(null);
  const cleanup = useRef<(() => void) | null>(null);
  const defaultsRef = useRef<HTMLStyleElement | null>(null);
  const selectionRef = useRef<{ doc: Document; range: Range } | null>(null);
  const callbacks = useRef({ onFraction, onReady, onNavigate, onSelectionChange, onImageOpen });
  callbacks.current = { onFraction, onReady, onNavigate, onSelectionChange, onImageOpen };
  const source = useMemo(() => {
    const csp = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data: " + location.origin + "/api/images/; font-src data: " + location.origin + "; base-uri 'none'; form-action 'none'";
    // iOS cannot paint horizontally overflowing vertical iframe documents
    // (https://bugs.webkit.org/show_bug.cgi?id=320457). Skip our optional
    // empty-CSS direction inference there; keep authored publication CSS.
    // iPadOS also identifies as macOS when requesting desktop websites.
    const isIOS = /iPad|iPhone|iPod/.test(navigator.userAgent)
      || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
    const readable = isIOS
      ? content.replace(/<style\b[^>]*\bdata-mandara-compat=["']empty-css-vrtl["'][^>]*>[\s\S]*?<\/style>/gi, "")
      : content;
    // CSP precedes all source markup, including when stored HTML is malformed.
    return readable.replace(/<!doctype html>/i, '<!doctype html><meta http-equiv="Content-Security-Policy" content="' + csp + '">');
  }, [content]);

  function scroller() {
    return frame.current?.contentDocument?.scrollingElement as HTMLElement | null;
  }
  function vertical() {
    const doc = frame.current?.contentDocument;
    return doc ? getComputedStyle(doc.body).writingMode.startsWith("vertical") : false;
  }
  function navigateFragment(fragment: string) {
    const doc = frame.current?.contentDocument;
    if (!doc?.defaultView) return;
    // Native fragment state activates :target and reveals collapsed details;
    // scrolling alone loses publication footnote presentation semantics.
    doc.defaultView.location.hash = "#" + encodeURIComponent(fragment);
    const target = doc.getElementById(fragment);
    for (let parent = target?.parentElement; parent; parent = parent.parentElement) {
      if (parent.tagName === "DETAILS") parent.setAttribute("open", "");
    }
    requestAnimationFrame(() => {
      if (frame.current?.contentDocument !== doc) return;
      target?.scrollIntoView({ block: "start" });
      frame.current?.scrollIntoView({ block: "nearest" });
    });
  }

  useImperativeHandle(ref, () => ({
    captureSelection() {
      const selected = selectionRef.current;
      if (!selected || frame.current?.contentDocument !== selected.doc) return { text: "" };
      return selectedTextContent(selected.doc.documentElement, selected.range);
    },
    scrollToFragment(fragment) {
      navigateFragment(fragment);
    },
    restoreFraction(fraction) {
      const root = scroller();
      if (!root) return;
      if (fraction === 0 && frame.current?.contentWindow) frame.current.contentWindow.location.hash = "";
      if (vertical()) {
        const mode = getComputedStyle(frame.current!.contentDocument!.body).writingMode;
        root.scrollLeft = (mode === "vertical-rl" ? -1 : 1) * (root.scrollWidth - root.clientWidth) * fraction;
      }
      else root.scrollTop = (root.scrollHeight - root.clientHeight) * fraction;
    },
  }));

  function initializeDocument() {
    const doc = frame.current?.contentDocument;
    if (!doc || (defaultsRef.current?.ownerDocument === doc && defaultsRef.current.isConnected)) return;
    const defaults = doc.createElement("style");
    // Low-specificity defaults precede publication rules. Keep this sheet and its
    // font faces intact when only typography or theme settings change.
    let css = ":where(html){} :where(body){margin:0;padding:0.5em} :where(img,svg){max-width:100%;height:auto} :where(table){max-width:100%}" + serifFontStyles;
    if (doc.documentElement.dataset.epubLayout !== "pre-paginated") {
      const illustrations = Array.from(doc.body.querySelectorAll("img:not(.gaiji),svg"))
        .filter(element => !element.parentElement?.closest("svg"));
      let hasText = false;
      if (illustrations.length === 1) {
        // Stop at the first prose node instead of cloning the complete chapter.
        const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT);
        for (let node = walker.nextNode(); node; node = walker.nextNode()) {
          if (node.textContent?.trim() && !node.parentElement?.closest("svg,style,script")) {
            hasText = true;
            break;
          }
        }
      }
      const imageOnly = illustrations.length === 1 && !hasText
        && !doc.body.querySelector("table,video,audio,canvas,math,img.gaiji");
      doc.documentElement.toggleAttribute("data-mandara-image-only", imageOnly);
      doc.querySelectorAll("[data-mandara-image-wrapper]").forEach(element => element.removeAttribute("data-mandara-image-wrapper"));
      doc.querySelectorAll("[data-mandara-image]").forEach(element => element.removeAttribute("data-mandara-image"));
      if (imageOnly) {
        illustrations[0].setAttribute("data-mandara-image", "");
        for (let wrapper = illustrations[0].parentElement; wrapper && wrapper !== doc.body; wrapper = wrapper.parentElement) {
          wrapper.setAttribute("data-mandara-image-wrapper", "");
        }
        css += imagePageStyles;
      }
      // Bound both physical axes, including horizontally overflowing vertical text.
      css += " :where(img){max-width:calc(100vw - 3em);max-height:calc(100vh - 3em);width:auto;height:auto}";
      // Gaiji remain inline glyphs; publisher rules still take precedence.
      css += " :where(img.gaiji){display:inline;width:auto;height:1em;vertical-align:baseline}";
    }
    defaults.textContent = css;
    doc.head.prepend(defaults);
    defaultsRef.current = defaults;
  }

  function applySettings() {
    const doc = frame.current?.contentDocument;
    const host = frame.current?.parentElement;
    const defaults = defaultsRef.current;
    if (!doc || !host || defaults?.ownerDocument !== doc || !defaults.isConnected) return;
    const inherited = getComputedStyle(host);
    // Use the theme token, not an intermediate animated host color.
    const foreground = inherited.getPropertyValue("--fg").trim() || inherited.color;
    const rootRule = defaults.sheet!.cssRules[0] as CSSStyleRule;
    rootRule.style.cssText = "font-size:" + settings.fontSize + "px;line-height:" + settings.lineHeight + ";font-family:" + inherited.fontFamily + ";color:" + foreground + ";color-scheme:" + inherited.colorScheme + ";scrollbar-width:thin;background:transparent";
    if (doc.documentElement.dataset.epubLayout !== "pre-paginated" && !doc.documentElement.hasAttribute("data-mandara-image-only")) {
      // Direction can change through publisher media queries or typography settings.
      const isVertical = vertical();
      doc.documentElement.style.setProperty("box-sizing", "border-box");
      doc.documentElement.style.setProperty("scrollbar-gutter", "stable");
      doc.documentElement.style.setProperty(isVertical ? "padding-bottom" : "padding-right", "1em");
      doc.body.style.setProperty("box-sizing", "border-box");
      doc.body.style.setProperty(isVertical ? "max-height" : "max-width", isVertical ? "calc(100vh - 1em)" : "100%");
    }
  }

  function scaleFixedLayout() {
    const element = frame.current;
    const doc = element?.contentDocument;
    if (!element || !doc || doc.documentElement.dataset.epubLayout !== "pre-paginated") return;
    const viewport = doc.querySelector('meta[name="viewport"]')?.getAttribute("content") ?? "";
    const width = Number(viewport.match(/(?:^|[,;])\s*width\s*=\s*(\d+(?:\.\d+)?)/i)?.[1]);
    const height = Number(viewport.match(/(?:^|[,;])\s*height\s*=\s*(\d+(?:\.\d+)?)/i)?.[1]);
    if (!(width > 0 && height > 0)) return;
    const scale = Math.min(element.clientWidth / width, element.clientHeight / height);
    const root = doc.documentElement;
    root.style.width = width + "px";
    root.style.height = height + "px";
    root.style.transformOrigin = "top left";
    root.style.transform = "scale(" + scale + ")";
    root.style.overflow = "hidden";
  }

  function ready() {
    cleanup.current?.();
    selectionRef.current = null;
    initializeDocument();
    applySettings();
    scaleFixedLayout();
    const doc = frame.current?.contentDocument;
    const win = frame.current?.contentWindow;
    if (!doc || !win) return;
    function scroll() {
      const root = scroller();
      if (!root) return;
      const isVertical = vertical();
      const max = isVertical ? root.scrollWidth - root.clientWidth : root.scrollHeight - root.clientHeight;
      const offset = isVertical ? Math.abs(root.scrollLeft) : root.scrollTop;
      callbacks.current.onFraction(max > 0 ? Math.max(0, Math.min(1, offset / max)) : 0);
    }
    function advance(amount: number) {
      const root = scroller();
      if (!root) return;
      const mode = getComputedStyle(doc!.body).writingMode;
      root.scrollLeft += (mode === "vertical-rl" ? -1 : 1) * amount;
    }
    function wheel(event: WheelEvent) {
      const root = scroller();
      if (!root || !vertical() || root.scrollWidth <= root.clientWidth || event.ctrlKey || event.shiftKey || event.defaultPrevented || Math.abs(event.deltaX) >= Math.abs(event.deltaY)) return;
      event.preventDefault();
      const unit = event.deltaMode === 1 ? parseFloat(getComputedStyle(doc!.documentElement).fontSize) : event.deltaMode === 2 ? root.clientWidth : 1;
      advance(event.deltaY * unit);
    }
    function keydown(event: KeyboardEvent) {
      const root = scroller();
      if (!root || !vertical() || root.scrollWidth <= root.clientWidth || event.ctrlKey || event.altKey || event.metaKey || event.defaultPrevented) return;
      const target = event.target as Element | null;
      if (target?.closest?.("a,button,input,textarea,select,summary,[contenteditable]")) return;
      const page = root.clientWidth * 0.9;
      switch (event.key) {
        case "ArrowDown": advance(parseFloat(getComputedStyle(doc!.documentElement).fontSize) * 3); break;
        case "ArrowUp": advance(-parseFloat(getComputedStyle(doc!.documentElement).fontSize) * 3); break;
        case "PageDown": advance(page); break;
        case "PageUp": advance(-page); break;
        case " ": advance(event.shiftKey ? -page : page); break;
        case "Home": root.scrollLeft = 0; break;
        case "End": advance(root.scrollWidth); break;
        default: return;
      }
      event.preventDefault();
    }
    function click(event: MouseEvent) {
      const target = event.target as Element | null;
      if (!event.defaultPrevented && event.button === 0 && !event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey
        && target && callbacks.current.onImageOpen && !doc!.getSelection()?.toString()) {
        const image = readerImageAt(target);
        if (image) { callbacks.current.onImageOpen(image); return; }
      }
      const anchor = (event.target as Element | null)?.closest("a");
      const href = anchor?.getAttribute("href");
      if (!href) return;
      event.preventDefault();
      const internal = href.match(/^epub:chapter\/(\d+)(?:#(.*))?$/);
      if (internal) {
        try { callbacks.current.onNavigate(Number(internal[1]), internal[2] ? decodeURIComponent(internal[2]) : undefined); } catch { /* Invalid source fragment. */ }
      } else if (href.startsWith("#")) {
        try { navigateFragment(decodeURIComponent(href.slice(1))); } catch { /* Invalid source fragment. */ }
      } else if (/^(https?:|mailto:)/i.test(href)) {
        window.open(href, "_blank", "noopener,noreferrer");
      }
    }
    function selectionChanged() {
      const selection = doc!.getSelection();
      const text = selectedChapterText(doc!.body, selection);
      // Keep dragging cheap; freeze the full publication only when capture starts.
      selectionRef.current = text ? { doc: doc!, range: selection!.getRangeAt(0).cloneRange() } : null;
      callbacks.current.onSelectionChange?.({ text });
    }
    const observer = new ResizeObserver(scaleFixedLayout);
    if (frame.current) observer.observe(frame.current);
    win.addEventListener("scroll", scroll, { passive: true });
    doc.addEventListener("selectionchange", selectionChanged);
    doc.addEventListener("click", click);
    doc.addEventListener("wheel", wheel, { passive: false });
    doc.addEventListener("keydown", keydown);
    let disposed = false;
    cleanup.current = () => {
      disposed = true;
      selectionRef.current = null;
      observer.disconnect();
      win.removeEventListener("scroll", scroll);
      doc.removeEventListener("selectionchange", selectionChanged);
      doc.removeEventListener("click", click);
      doc.removeEventListener("wheel", wheel);
      doc.removeEventListener("keydown", keydown);
    };
    // Restore progress only after webfont metrics have settled.
    const notifyReady = () => {
      if (!disposed && frame.current?.contentDocument === doc) callbacks.current.onReady();
    };
    if (doc.fonts) void doc.fonts.ready.then(notifyReady);
    else notifyReady();
  }
  useEffect(() => { applySettings(); scaleFixedLayout(); }, [settings]);
  useEffect(() => () => cleanup.current?.(), []);

  // WebKit otherwise suppresses even the trusted parent's event listeners.
  // The leading script-src CSP still forbids all publication scripts.
  return <iframe
    ref={frame}
    className="epub-document"
    title={title}
    sandbox="allow-same-origin allow-scripts"
    referrerPolicy="no-referrer"
    srcDoc={source}
    onLoad={ready}
  />;
}
