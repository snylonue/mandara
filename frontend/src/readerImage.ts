export interface ReaderImage {
  src: string;
  alt: string;
}

/** Resolve original illustration resources without changing publication markup. */
export function readerImageAt(target: Element): ReaderImage | null {
  if (target.closest("a,button,input,textarea,select,[contenteditable]")) return null;
  let image = target.closest("img:not(.gaiji),image");
  if (!image) {
    const svg = target.closest("svg");
    const images = svg?.querySelectorAll("image");
    if (images?.length === 1) image = images[0];
  }
  if (!image) return null;
  const source = image.localName === "img"
    ? (image as HTMLImageElement).currentSrc || image.getAttribute("src")
    : image.getAttribute("href") ?? image.getAttributeNS("http://www.w3.org/1999/xlink", "href");
  if (!source) return null;
  try {
    const url = new URL(source, location.href);
    // Match the reader frame's resource policy before moving an image outside it.
    if (!(url.origin === location.origin && url.pathname.startsWith("/api/images/"))
      && !/^data:image\/(?:png|jpeg|gif|webp|svg\+xml)[;,]/i.test(source)) return null;
    return { src: url.href, alt: image.getAttribute("alt") || image.closest("svg")?.querySelector("title")?.textContent || "" };
  } catch { return null; }
}
