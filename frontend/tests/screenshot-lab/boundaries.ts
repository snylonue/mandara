import { renderEpubScreenshot } from "../../src/epubScreenshot";
import { renderReaderScreenshot } from "../../src/readerScreenshot";

/** Exercise real browser failures and allocation guards without renderer mocks. */
export async function checkBoundaries() {
  const results: { name: string; passed: boolean; detail?: string }[] = [];
  const style = {
    background: "#fff",
    foreground: "#222",
    muted: "#666",
    accent: "#287",
    fontFamily: "serif",
  };
  const source = (body: string, css = "") =>
    "<!doctype html><html><head><style>html,body{margin:0;padding:0}" +
    css +
    "</style></head><body>" +
    body +
    "</body></html>";
  async function test(name: string, work: () => Promise<string | void>) {
    try {
      const detail = await work();
      results.push({ name, passed: true, detail: detail || undefined });
    } catch (error) {
      results.push({ name, passed: false, detail: String(error) });
    }
  }
  async function rejects(work: () => Promise<unknown>, expected: string) {
    try {
      await work();
    } catch (error) {
      if (String(error).includes(expected)) return String(error);
      throw error;
    }
    throw new Error("Expected rejection: " + expected);
  }
  await test("reject-empty-selection", () =>
    rejects(
      () => renderReaderScreenshot({ text: "", style }),
      "selection-size",
    ));
  await test("reject-over-6000-characters", () =>
    rejects(
      () => renderReaderScreenshot({ text: "字".repeat(6001), style }),
      "selection-size",
    ));
  await test("reject-over-height-limit", () =>
    rejects(
      () =>
        renderEpubScreenshot(
          source('<div style="height:12001px">正文</div>'),
          772,
          12000,
        ),
      "selection-size",
    ));
  await test("reject-over-width-limit", () =>
    rejects(
      () => renderEpubScreenshot(source("<p>正文</p>"), 12001, 12000),
      "selection-size",
    ));
  await test("abort-cleans-export-frame", async () => {
    const count = document.querySelectorAll("iframe").length;
    const controller = new AbortController();
    const pending = renderEpubScreenshot(
      source("<p>正文</p>"),
      772,
      12000,
      700,
      controller.signal,
    );
    controller.abort(new Error("lab-cancelled"));
    await rejects(() => pending, "lab-cancelled");
    if (document.querySelectorAll("iframe").length !== count)
      throw new Error("Export frame leaked");
  });
  await test("missing-image-is-explicit-error", () =>
    rejects(
      () =>
        renderEpubScreenshot(
          source('<img src="/api/images/missing.png">'),
          772,
          12000,
        ),
      "image-load",
    ));
  await test("missing-font-is-explicit-error", () =>
    rejects(
      () =>
        renderEpubScreenshot(
          source(
            "<p>正文</p>",
            "@font-face{font-family:labMissing;src:url(/api/images/missing.png)}p{font-family:labMissing}",
          ),
          772,
          12000,
        ),
      "font-load",
    ));
  await test("canvas-budget-keeps-final-content", async () => {
    const canvas = await renderEpubScreenshot(
      source(
        '<div style="height:9500px;position:relative"><div style="position:absolute;bottom:0;width:100%;height:20px;background:rgb(255,0,0)">末尾</div></div>',
      ),
      772,
      12000,
    );
    if (
      canvas.width * canvas.height > 16000000 ||
      Math.max(canvas.width, canvas.height) > 16384
    )
      throw new Error("Canvas budget exceeded");
    const pixel = canvas
      .getContext("2d")!
      .getImageData(Math.floor(canvas.width / 2), canvas.height - 2, 1, 1).data;
    if (pixel[0] !== 255 || pixel[1] !== 0 || pixel[2] !== 0)
      throw new Error("Final painted marker was truncated");
    return (
      canvas.width + "x" + canvas.height + " pixels; final marker preserved"
    );
  });
  return results;
}
