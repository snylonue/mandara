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
  }

  function scaleFixedLayout() {
    const element = frame.current;
    const doc = element?.contentDocument;
    if (!element || !doc || doc.documentElement.dataset.epubLayout !== "pre-paginated") return;
    const viewport = doc.querySelector('meta[name="viewport"]')?.getAttribute("content") ?? "";
    const width = Number(viewport.match(/(?:^|[,;])\s*width\s*=\s*(\d+(?:\.\d+)?)/i)?.[1]);
    const height = Number(viewport.match(/(?:^|[,;])\s*height\s*=\s*(\d+(?:\.\d+)?)/i)?.[1]);
    if (!(width > 0 && height > 0)) return;
    const scale = Math.min(element.clientWidth / width, window.innerHeight * 0.75 / height);
    const root = doc.documentElement;
    root.style.width = width + "px";
    root.style.height = height + "px";
    root.style.transformOrigin = "top left";
    root.style.transform = "scale(" + scale + ")";
    root.style.overflow = "hidden";
    element.style.height = height * scale + "px";
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
    cleanup.current = () => {
      observer.disconnect();
      win.removeEventListener("scroll", scroll);
      doc.removeEventListener("click", click);
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
