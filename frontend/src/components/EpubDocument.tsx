import { useEffect, useImperativeHandle, useMemo, useRef, type Ref } from "react";
import type { ReaderSettings } from "./ReaderSettings";

export interface EpubDocumentHandle {
  scrollToFragment: (fragment: string) => void;
  restoreFraction: (fraction: number) => void;
}

/** Never insert publisher CSS into the application DOM. */
export function EpubDocument({ content, title, settings, onFraction, onReady, onNavigate, ref }: {
  content: string;
  title: string;
  settings: ReaderSettings;
  onFraction: (fraction: number) => void;
  onReady: () => void;
  onNavigate: (idx: number, fragment?: string) => void;
  ref?: Ref<EpubDocumentHandle>;
}) {
  const frame = useRef<HTMLIFrameElement>(null);
  const cleanup = useRef<(() => void) | null>(null);
  const defaultsRef = useRef<HTMLStyleElement | null>(null);
  const callbacks = useRef({ onFraction, onReady, onNavigate });
  callbacks.current = { onFraction, onReady, onNavigate };
  const source = useMemo(() => {
    const csp = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data: " + location.origin + "/api/images/; font-src data:; base-uri 'none'; form-action 'none'";
    // CSP precedes all source markup, including when stored HTML is malformed.
    return content.replace(/<!doctype html>/i, '<!doctype html><meta http-equiv="Content-Security-Policy" content="' + csp + '">');
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

  function applySettings() {
    const doc = frame.current?.contentDocument;
    const host = frame.current?.parentElement;
    if (!doc || !host) return;
    const inherited = getComputedStyle(host);
    let defaults = defaultsRef.current;
    if (!defaults || defaults.ownerDocument !== doc) {
      defaults = doc.createElement("style");
      doc.head.prepend(defaults);
      defaultsRef.current = defaults;
    }
    // Low-specificity defaults precede publication rules; no paragraph/div
    // justification or display:block overrides the publisher's own cascade.
    defaults.textContent = ":where(html){font-size:" + settings.fontSize + "px;line-height:" + settings.lineHeight + ";font-family:" + inherited.fontFamily + ";color:" + inherited.color + ";background:transparent} :where(body){margin:0;padding:0.5em} :where(img,svg){max-width:100%;height:auto} :where(table){max-width:100%}";
    if (doc.documentElement.dataset.epubLayout !== "pre-paginated") {
      const imageOnly = doc.images.length === 1 && !doc.body.textContent?.trim() && !doc.querySelector("svg,table,video,audio,canvas");
      // Overlay scrollbars do not reserve a gutter. Keep text clear even when
      // publication CSS removes body padding or sizes it to the viewport.
      if (!imageOnly) {
        const isVertical = vertical();
        doc.documentElement.style.setProperty("box-sizing", "border-box");
        doc.documentElement.style.setProperty("scrollbar-gutter", "stable");
        doc.documentElement.style.setProperty(isVertical ? "padding-bottom" : "padding-right", "1em");
        doc.body.style.setProperty("box-sizing", "border-box");
        doc.body.style.setProperty(isVertical ? "max-height" : "max-width", isVertical ? "calc(100vh - 1em)" : "100%");
      }
      // A vertical block can be wider than the viewport, so 100% alone does
      // not constrain publisher image dimensions. Bound both physical axes.
      defaults.textContent += " :where(img){max-width:calc(100vw - 3em);max-height:calc(100vh - 3em);width:auto;height:auto} :where(html[data-mandara-image-only] body){display:grid;place-items:center;width:100vw;height:100vh;box-sizing:border-box} :where(html[data-mandara-image-only] img){display:block}";
      doc.documentElement.toggleAttribute("data-mandara-image-only", imageOnly);
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
    applySettings();
    scaleFixedLayout();
    const doc = frame.current?.contentDocument;
    const win = frame.current?.contentWindow;
    if (!doc || !win) return;
    function scroll() {
      const root = scroller();
      if (!root) return;
      const max = vertical() ? root.scrollWidth - root.clientWidth : root.scrollHeight - root.clientHeight;
      const offset = vertical() ? Math.abs(root.scrollLeft) : root.scrollTop;
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
    const observer = new ResizeObserver(scaleFixedLayout);
    if (frame.current) observer.observe(frame.current);
    win.addEventListener("scroll", scroll, { passive: true });
    doc.addEventListener("click", click);
    doc.addEventListener("wheel", wheel, { passive: false });
    doc.addEventListener("keydown", keydown);
    cleanup.current = () => {
      observer.disconnect();
      win.removeEventListener("scroll", scroll);
      doc.removeEventListener("click", click);
      doc.removeEventListener("wheel", wheel);
      doc.removeEventListener("keydown", keydown);
    };
    callbacks.current.onReady();
  }
  useEffect(() => { applySettings(); scaleFixedLayout(); }, [settings]);
  useEffect(() => () => cleanup.current?.(), []);

  return <iframe
    ref={frame}
    className="epub-document"
    title={title}
    sandbox="allow-same-origin"
    referrerPolicy="no-referrer"
    srcDoc={source}
    onLoad={ready}
  />;
}
