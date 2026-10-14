import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Reader } from "../src/components/Reader";
import type { Chapter } from "../src/types";

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
