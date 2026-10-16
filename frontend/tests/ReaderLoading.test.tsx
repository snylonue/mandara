import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Reader } from "../src/components/Reader";
import type { Chapter } from "../src/types";
import { ApiError } from "../src/api";

const chapters = [
  { idx: 0, title: "第一章", linear: true },
  { idx: 1, title: "第二章", linear: true },
];
const first = { idx: 0, title: "第一章", content: "<p>缓存正文</p>" };
const second = { idx: 1, title: "第二章", content: "<p>下一章正文</p>" };

beforeEach(() => {
  localStorage.clear();
  HTMLElement.prototype.scrollTo = vi.fn();
});
afterEach(() => {
  delete (HTMLElement.prototype as Partial<HTMLElement>).scrollTo;
  localStorage.clear();
});

it("stops loading on failure and retries the same chapter", async () => {
  const load = vi.fn<(idx: number) => Promise<Chapter>>()
    .mockRejectedValueOnce(new Error("书源暂时不可用"))
    .mockResolvedValueOnce(first);
  render(<Reader title="测试书籍" chapters={chapters} loadChapter={load} />);
  expect((await screen.findByRole("alert")).textContent).toBe("书源暂时不可用");
  expect(screen.queryByText("加载中…")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "重新加载章节" }));
  await screen.findByText("缓存正文");
  expect(load.mock.calls).toEqual([[0], [0]]);
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.queryByRole("button", { name: "重新加载章节" })).toBeNull();
});

it("repairs a missing body only on explicit retry and reads the next chapter normally", async () => {
  const load = vi.fn<(idx: number) => Promise<Chapter>>()
    .mockRejectedValueOnce(new ApiError(404, "stored chapter"))
    .mockResolvedValueOnce(second);
  const retry = vi.fn<(idx: number) => Promise<Chapter>>().mockResolvedValue(first);
  render(<Reader title="测试书籍" chapters={chapters} loadChapter={load} retryChapter={retry} />);
  expect((await screen.findByRole("alert")).textContent).toBe("章节正文尚未缓存，点击“重新加载章节”可补下载。");
  expect(retry).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "重新加载章节" }));
  await screen.findByText("缓存正文");
  expect(retry.mock.calls).toEqual([[0]]);
  fireEvent.click(screen.getByRole("button", { name: /下一章/ }));
  await screen.findByText("下一章正文");
  fireEvent.click(screen.getByRole("button", { name: /上一章/ }));
  await screen.findByText("缓存正文");
  expect(load.mock.calls).toEqual([[0], [1]]);
  expect(retry.mock.calls).toEqual([[0]]);
});

it("keeps public retries storage-only and explains a missing body", async () => {
  const load = vi.fn<(idx: number) => Promise<Chapter>>()
    .mockRejectedValueOnce(new ApiError(404, "stored chapter"))
    .mockResolvedValueOnce(first);
  render(<Reader title="测试书籍" chapters={chapters} loadChapter={load} readOnly />);
  expect((await screen.findByRole("alert")).textContent).toBe("章节正文尚未缓存，请稍后重试或联系书籍管理者。");
  fireEvent.click(screen.getByRole("button", { name: "重新加载章节" }));
  await screen.findByText("缓存正文");
  expect(load.mock.calls).toEqual([[0], [0]]);
});

it("ignores a repaired chapter until it is selected again", async () => {
  let complete!: (chapter: Chapter) => void;
  const pending = new Promise<Chapter>(resolve => { complete = resolve; });
  const load = vi.fn<(idx: number) => Promise<Chapter>>()
    .mockRejectedValueOnce(new ApiError(404, "stored chapter"))
    .mockResolvedValueOnce(second);
  const retry = vi.fn().mockReturnValue(pending);
  render(<Reader title="测试书籍" chapters={chapters} loadChapter={load} retryChapter={retry} />);
  await screen.findByRole("alert");
  fireEvent.click(screen.getByRole("button", { name: "重新加载章节" }));
  await waitFor(() => expect(retry).toHaveBeenCalledWith(0));
  fireEvent.click(screen.getByRole("button", { name: /下一章/ }));
  await screen.findByText("下一章正文");
  await act(async () => complete(first));
  expect(screen.queryByText("缓存正文")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /上一章/ }));
  await screen.findByText("缓存正文");
  expect(load.mock.calls).toEqual([[0], [1]]);
});

it("uses a translated fallback for non-Error failures", async () => {
  render(<Reader title="测试书籍" chapters={chapters} loadChapter={async () => { throw null; }} />);
  expect((await screen.findByRole("alert")).textContent).toBe("加载失败");
  expect(screen.queryByText("加载中…")).toBeNull();
});

it("ignores a failure from a chapter that is no longer selected", async () => {
  let reject!: (error: Error) => void;
  const pending = new Promise<Chapter>((_, fail) => { reject = fail; });
  const load = vi.fn<(idx: number) => Promise<Chapter>>()
    .mockReturnValueOnce(pending)
    .mockResolvedValueOnce(second);
  render(<Reader title="测试书籍" chapters={chapters} loadChapter={load} />);
  await waitFor(() => expect(load).toHaveBeenCalledWith(0));
  fireEvent.click(screen.getByRole("button", { name: /下一章/ }));
  await screen.findByText("下一章正文");
  await act(async () => { reject(new Error("过期请求失败")); });
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.queryByText("加载中…")).toBeNull();
});

it("allows read-only chapter navigation without saving progress", async () => {
  const load = vi.fn(async (idx: number) => ({
    idx, title: chapters[idx].title, content: idx === 0 ? first.content : second.content,
  }));
  const onProgress = vi.fn();
  const { unmount } = render(<Reader title="测试书籍" chapters={chapters} loadChapter={load} onProgress={onProgress} readOnly />);
  await screen.findByText("缓存正文");
  expect((screen.getByRole("button", { name: /上一章/ }) as HTMLButtonElement).disabled).toBe(true);
  const next = screen.getByRole("button", { name: /下一章/ }) as HTMLButtonElement;
  expect(next.disabled).toBe(false);
  fireEvent.click(next);
  await screen.findByText("下一章正文");
  expect((screen.getByRole("button", { name: /下一章/ }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: /上一章/ }));
  await screen.findByText("缓存正文");
  expect(load.mock.calls).toEqual([[0], [1]]);
  unmount();
  expect(onProgress).not.toHaveBeenCalled();
});

it("keeps successful chapter content cached across navigation", async () => {
  const load = vi.fn<(idx: number) => Promise<Chapter>>()
    .mockResolvedValueOnce(first)
    .mockResolvedValueOnce(second);
  render(<Reader title="测试书籍" chapters={chapters} loadChapter={load} />);
  await screen.findByText("缓存正文");
  fireEvent.click(screen.getByRole("button", { name: /下一章/ }));
  await screen.findByText("下一章正文");
  fireEvent.click(screen.getByRole("button", { name: /上一章/ }));
  await screen.findByText("缓存正文");
  expect(load.mock.calls).toEqual([[0], [1]]);
});
