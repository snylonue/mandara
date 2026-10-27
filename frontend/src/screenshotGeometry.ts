/** Track clipped clones, including SnapDOM's unmapped offscreen spacers. */
export function screenshotGeometry(signal: AbortSignal) {
  const attribute = "data-reader-geometry-" + Math.random().toString(36).slice(2);
  const sources = new Map<string, Element>();
  let released = false;
  return {
    track(source: Element) {
      signal.throwIfAborted();
      if (released) return;
      const id = String(sources.size);
      sources.set(id, source);
      source.setAttribute(attribute, id);
    },
    restore(root: Element) {
      for (const clone of [root, ...root.querySelectorAll("*")]) {
        const id = clone.getAttribute(attribute);
        if (id === null) continue;
        clone.removeAttribute(attribute);
        const source = sources.get(id);
        if (!source || source.namespaceURI !== "http://www.w3.org/1999/xhtml") continue;
        const spacer = clone as HTMLElement;
        if (spacer.style.visibility !== "hidden" || clone.hasChildNodes() || !source.hasChildNodes()) continue;
        const style = source.ownerDocument.defaultView!.getComputedStyle(source);
        if (style.visibility === "hidden" || style.display === "none") continue;
        // offsetWidth/offsetHeight round each culled paragraph to whole pixels.
        // Retain its untransformed border box so errors cannot accumulate before the selection.
        for (const [dimension, edges] of [
          ["width", ["padding-left", "padding-right", "border-left-width", "border-right-width"]],
          ["height", ["padding-top", "padding-bottom", "border-top-width", "border-bottom-width"]],
        ] as const) {
          const value = style.getPropertyValue(dimension);
          if (!value.endsWith("px")) continue;
          const size = parseFloat(value) + (style.boxSizing === "border-box" ? 0
            : edges.reduce((sum, edge) => sum + (parseFloat(style.getPropertyValue(edge)) || 0), 0));
          if (!Number.isFinite(size) || size < 0) continue;
          for (const prefix of ["", "min-", "max-"]) spacer.style.setProperty(prefix + dimension, size + "px");
        }
      }
    },
    release() {
      released = true;
      for (const source of sources.values()) source.removeAttribute(attribute);
      sources.clear();
    },
  };
}
