import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ReaderScreenshotDialog, type ReaderExcerpt } from "../src/components/ReaderScreenshotDialog";
import { renderReaderScreenshot } from "../src/readerScreenshot";

vi.mock("../src/readerScreenshot", () => ({ renderReaderScreenshot: vi.fn() }));
const renderer = vi.mocked(renderReaderScreenshot);
const blob = new Blob(["png"], { type: "image/png" });
const excerpt: ReaderExcerpt = {
  text: "只截取这句话。", title: "测试书名", chapter: "第二章", chapterIdx: 1,
  style: { background: "#fff", foreground: "#111", muted: "#666", accent: "#369", fontFamily: "serif" },
};
const write = vi.fn();
const createObjectURL = vi.fn();
const revokeObjectURL = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  renderer.mockReset().mockResolvedValue(blob);
  createObjectURL.mockReturnValue("blob:preview");
  Object.defineProperty(URL, "createObjectURL", { configurable: true, value: createObjectURL });
  Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: revokeObjectURL });
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { write } });
  vi.stubGlobal("isSecureContext", true);
  vi.stubGlobal("ClipboardItem", class { constructor(public data: Record<string, Blob>) {} });
  write.mockResolvedValue(undefined);
});
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it("renders only the excerpt and independently omits optional metadata", async () => {
  render(<ReaderScreenshotDialog excerpt={excerpt} onClose={vi.fn()} />);
  await screen.findByAltText("所选文字截图预览");
  expect(renderer.mock.lastCall?.[0]).toEqual({ text: excerpt.text, title: excerpt.title, chapter: excerpt.chapter, style: excerpt.style, link: undefined });
  fireEvent.click(screen.getByLabelText("书名"));
  fireEvent.click(screen.getByLabelText("章节"));
  await waitFor(() => expect(renderer.mock.lastCall?.[0]).toMatchObject({ title: undefined, chapter: undefined }));
  expect((screen.getByLabelText("分享链接") as HTMLInputElement).disabled).toBe(true);
});

it("requires explicit confirmation to create a share and reuses its link", async () => {
  const create = vi.fn().mockResolvedValue("https://example.com/share/token/read?file_id=version");
  render(<ReaderScreenshotDialog excerpt={excerpt} createShareLink={create} onClose={vi.fn()} />);
  await screen.findByAltText("所选文字截图预览");
  expect(create).not.toHaveBeenCalled();
  fireEvent.click(screen.getByLabelText("分享链接"));
  expect(create).not.toHaveBeenCalled();
  expect((screen.getByRole("button", { name: "保存 PNG" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(/任何持链接者/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "确认并生成分享链接" }));
  await waitFor(() => expect(renderer.mock.lastCall?.[0].link).toBe("https://example.com/share/token/read?file_id=version&chapter=1"));
  fireEvent.click(screen.getByLabelText("分享链接"));
  fireEvent.click(screen.getByLabelText("分享链接"));
  await screen.findByAltText("所选文字截图预览");
  expect(create).toHaveBeenCalledTimes(1);
});

it("reuses a public share without a write request and preserves the selected version", async () => {
  render(<ReaderScreenshotDialog excerpt={excerpt} shareLink="/share/public/read?file_id=two&chapter=0" onClose={vi.fn()} />);
  fireEvent.click(screen.getByLabelText("分享链接"));
  await screen.findByAltText("所选文字截图预览");
  expect(renderer.mock.lastCall?.[0].link).toBe(location.origin + "/share/public/read?file_id=two&chapter=1");
  expect(screen.queryByRole("button", { name: "确认并生成分享链接" })).toBeNull();
});

it("keeps saving without a link available after share permission errors", async () => {
  const create = vi.fn().mockRejectedValue(new Error("无权分享"));
  render(<ReaderScreenshotDialog excerpt={excerpt} createShareLink={create} onClose={vi.fn()} />);
  fireEvent.click(screen.getByLabelText("分享链接"));
  fireEvent.click(screen.getByRole("button", { name: "确认并生成分享链接" }));
  expect((await screen.findByRole("alert")).textContent).toBe("无权分享");
  fireEvent.click(screen.getByLabelText("分享链接"));
  await screen.findByAltText("所选文字截图预览");
  expect((screen.getByRole("button", { name: "保存 PNG" }) as HTMLButtonElement).disabled).toBe(false);
});

it("copies the generated PNG, handles rejection, and offers a PNG download", async () => {
  const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
  render(<ReaderScreenshotDialog excerpt={excerpt} onClose={vi.fn()} />);
  await screen.findByAltText("所选文字截图预览");
  fireEvent.click(screen.getByRole("button", { name: "复制图片" }));
  await screen.findByText("图片已复制");
  expect(write.mock.calls[0][0][0].data["image/png"]).toBe(blob);
  write.mockRejectedValueOnce(new Error("permission denied"));
  fireEvent.click(screen.getByRole("button", { name: "复制图片" }));
  await screen.findByRole("alert");
  fireEvent.click(screen.getByRole("button", { name: "保存 PNG" }));
  const anchor = click.mock.instances[0] as HTMLAnchorElement;
  expect(anchor.href).toBe("blob:preview");
  expect(anchor.download).toBe("阅读摘录.png");
});

it("supports saving on insecure HTTP without attempting clipboard access", async () => {
  vi.stubGlobal("isSecureContext", false);
  render(<ReaderScreenshotDialog excerpt={excerpt} onClose={vi.fn()} />);
  await screen.findByAltText("所选文字截图预览");
  expect((screen.getByRole("button", { name: "复制图片" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "保存 PNG" }) as HTMLButtonElement).disabled).toBe(false);
  expect(screen.getByText(/当前 HTTP 连接/)).toBeTruthy();
  expect(write).not.toHaveBeenCalled();
});

it("ignores stale renders and revokes object URLs on changes and unmount", async () => {
  let finish!: (blob: Blob) => void;
  renderer.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const { unmount } = render(<ReaderScreenshotDialog excerpt={excerpt} onClose={vi.fn()} />);
  fireEvent.click(screen.getByLabelText("书名"));
  await screen.findByAltText("所选文字截图预览");
  await act(async () => { finish(blob); });
  expect(createObjectURL).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByLabelText("章节"));
  await waitFor(() => expect(createObjectURL).toHaveBeenCalledTimes(2));
  expect(revokeObjectURL).toHaveBeenCalledTimes(1);
  unmount();
  expect(revokeObjectURL).toHaveBeenCalledTimes(2);
});

it("shows translated size and rendering errors without exporting stale images", async () => {
  renderer.mockRejectedValueOnce(new Error("selection-size"));
  render(<ReaderScreenshotDialog excerpt={excerpt} onClose={vi.fn()} />);
  expect((await screen.findByRole("alert")).textContent).toContain("6000");
  expect((screen.getByRole("button", { name: "保存 PNG" }) as HTMLButtonElement).disabled).toBe(true);
  renderer.mockRejectedValueOnce(new Error("canvas-unavailable"));
  fireEvent.click(screen.getByLabelText("书名"));
  await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("截图生成失败"));
});
