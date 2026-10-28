import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { createMemoryRouter, RouterProvider } from "react-router-dom";
import { api, ApiError } from "../src/api";
import App from "../src/App";
import type { Chapter, ShareBookResponse, ShareView } from "../src/types";

vi.mock("../src/api", async (importOriginal) => ({
  ...await importOriginal<typeof import("../src/api")>(), api: vi.fn(),
}));
const book: ShareBookResponse = {
  book: { id: "book", title: "测试书籍", authors: ["测试作者"], description: "书籍简介", cover_url: "https://example.test/cover.png", ext: {} },
  file: { id: "file", format: "epub", label: "精校版", chapter_count: 2, source: "local" },
  chapters: [{ idx: 0, title: "序章", linear: true }, { idx: 1, title: "第一章", linear: true }],
  toc: [],
};
const view: ShareView = {
  kind: "book", book: book.book, file: book.file, files: [book.file], session: null,
  created_at: "2026-01-01T00:00:00Z", expires_at: null,
};
let sharedBook: ShareBookResponse;
let sharedView: ShareView;
// Keep content assertions independent of cold Vite transforms; AppRoutes covers lazy loading.
beforeAll(async () => { await import("../src/pages/SharePage"); });
beforeEach(() => {
  vi.resetAllMocks();
  sharedBook = book;
  sharedView = view;
  localStorage.clear();
  HTMLElement.prototype.scrollTo = vi.fn();
  vi.mocked(api).mockImplementation(async (path) => {
    if (path === "/shares/test") return sharedView;
    if (path === "/shares/test/book") return sharedBook;
    if (path === "/shares/test/chapters/0") return { idx: 0, title: "序章", content: "<p>序章正文</p>" };
    if (path === "/shares/test/chapters/1") return { idx: 1, title: "第一章", content: "<p>第一章正文</p>" };
    throw new Error("Unexpected API call: " + path);
  });
});
afterEach(() => {
  delete (HTMLElement.prototype as Partial<HTMLElement>).scrollTo;
  delete document.documentElement.dataset.theme;
  localStorage.clear();
});
function sharePage(path = "/share/test") {
  const router = createMemoryRouter([{ path: "*", element: <App /> }], { initialEntries: [path] });
  return { ...render(<RouterProvider router={router} />), router };
}
function expectNoFooter() {
  expect(screen.queryByText(/分享自 Mandara/)).toBeNull();
  expect(screen.queryByRole("link", { name: "自己部署一个 Mandara" })).toBeNull();
}

it("does not show the promotional footer on the overview or in the reader", async () => {
  sharePage();
  fireEvent.click(await screen.findByRole("link", { name: "开始阅读" }));
  await screen.findByText("序章正文");
  expectNoFooter();
  fireEvent.click(screen.getByRole("link", { name: "返回书籍详情" }));
  await screen.findByRole("heading", { name: "测试书籍" });
  expectNoFooter();
});

it("opens an anonymous book overview without fetching chapter content", async () => {
  const { container } = sharePage();
  await screen.findByRole("heading", { name: "测试书籍" });
  expect(screen.getByText("测试作者")).toBeTruthy();
  expect(screen.getByText("书籍简介")).toBeTruthy();
  expect(screen.getByText("精校版")).toBeTruthy();
  expect(screen.getByText("共 2 章")).toBeTruthy();
  expect(screen.getByRole("navigation", { name: "目录" })).toBeTruthy();
  expect(screen.getByRole("link", { name: "第一章" }).getAttribute("href")).toBe("/share/test/read?chapter=1");
  expect(container.querySelector(".app-reader")).toBeNull();
  expect(container.querySelector(".reader")).toBeNull();
  expect(vi.mocked(api).mock.calls.map(([path]) => path)).toEqual(["/shares/test", "/shares/test/book"]);
});

it("reads the whole shared book and returns to its overview without authentication or progress writes", async () => {
  const { container } = sharePage();
  fireEvent.click(await screen.findByRole("link", { name: "开始阅读" }));
  await screen.findByText("序章正文");
  expect(container.querySelector(".app-reader .reader")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /下一章/ }));
  await screen.findByText("第一章正文");
  fireEvent.click(screen.getByRole("button", { name: /上一章/ }));
  await screen.findByText("序章正文");
  fireEvent.click(screen.getByRole("link", { name: "返回书籍详情" }));
  await screen.findByRole("heading", { name: "测试书籍" });
  expect(container.querySelector(".app-reader")).toBeNull();
  expect(vi.mocked(api).mock.calls.every(([path, init]) => path.startsWith("/shares/test") && !init?.method)).toBe(true);
});

it("opens a selected chapter from the overview and supports browser back", async () => {
  const { router } = sharePage();
  fireEvent.click(await screen.findByRole("link", { name: "第一章" }));
  await screen.findByText("第一章正文");
  expect(api).not.toHaveBeenCalledWith("/shares/test/chapters/0");
  await act(() => router.navigate(-1));
  await screen.findByRole("heading", { name: "测试书籍" });
  expect(screen.queryByText("第一章正文")).toBeNull();
});

it("supports refreshing a shared reading URL", async () => {
  sharePage("/share/test/read?chapter=1");
  await screen.findByText("第一章正文");
  expect(screen.getByRole("link", { name: "返回书籍详情" }).getAttribute("href")).toBe("/share/test");
});

it.each(["-1", "999", "NaN", "0.5"])("ignores an invalid chapter query %s", async (chapter) => {
  sharePage(`/share/test/read?chapter=${chapter}`);
  await screen.findByText("序章正文");
  expect(api).toHaveBeenCalledWith("/shares/test/chapters/0");
});

it("keeps session shares on the overview and lets visitors choose the shared progress or the beginning", async () => {
  sharedView = { ...view, kind: "session", session: {
    id: "session", label: "手机", owner_username: "读者",
    position: { chapter_idx: 1, in_chapter: { fraction: 0.5 } },
    percent: 50, updated_at: "2026-01-01T00:00:00Z",
  } };
  sharePage();
  await screen.findByText("读者 的进度 · 阅读档案「手机」");
  expect(screen.getByText("50%")).toBeTruthy();
  expect(api).not.toHaveBeenCalledWith("/shares/test/chapters/1");
  fireEvent.click(screen.getByRole("link", { name: "从分享进度阅读" }));
  await screen.findByText("第一章正文");
  fireEvent.click(screen.getByRole("link", { name: "返回书籍详情" }));
  fireEvent.click(await screen.findByRole("link", { name: "开始阅读" }));
  await screen.findByText("序章正文");
});

it("handles books without chapters without requesting nonexistent content", async () => {
  sharedBook = { ...book, chapters: [] };
  sharePage();
  await screen.findByText("暂无可阅读章节");
  expect(screen.queryByRole("link", { name: "开始阅读" })).toBeNull();
  expect(vi.mocked(api).mock.calls).toHaveLength(2);
});

it("reports invalid or expired links instead of entering the reader", async () => {
  vi.mocked(api).mockRejectedValue(new Error("链接已过期"));
  sharePage();
  expect((await screen.findByRole("alert")).textContent).toBe("链接已过期");
  expect(screen.queryByRole("link", { name: "开始阅读" })).toBeNull();
});

it("discards requests from an old token when another share is opened", async () => {
  let resolve!: (value: ShareBookResponse) => void;
  const pending = new Promise<ShareBookResponse>((done) => { resolve = done; });
  vi.mocked(api).mockImplementation((path) => {
    if (path === "/shares/old/book") return pending as ReturnType<typeof api>;
    return Promise.resolve(path.endsWith("/book") ? book : view) as ReturnType<typeof api>;
  });
  const { router } = sharePage("/share/old");
  await act(() => router.navigate("/share/test"));
  await screen.findByRole("heading", { name: "测试书籍" });
  await act(async () => { resolve({ ...book, book: { ...book.book, title: "过期书籍" } }); });
  expect(screen.queryByText("过期书籍")).toBeNull();
  expect(screen.getByRole("link", { name: "开始阅读" }).getAttribute("href")).toBe("/share/test/read?chapter=0");
});

it("uses the normal detail hero with stored cover and remote fallback", async () => {
  const { container } = sharePage();
  await screen.findByRole("heading", { name: "测试书籍" });
  const cover = container.querySelector(".detail-hero .detail-cover img")!;
  expect(cover.getAttribute("src")).toBe("/api/books/book/cover");
  fireEvent.error(cover);
  expect(cover.getAttribute("src")).toBe("https://example.test/cover.png");
  expect(container.querySelector(".book-detail .detail-info")).toBeTruthy();
});

const alternateBook: ShareBookResponse = {
  ...book, file: { ...book.file, id: "source-file", label: "网络版", source: "书源乙" },
  chapters: [{ idx: 0, title: "网络序章", linear: true }, { idx: 1, title: "网络第一章", linear: true }],
};
function multipleSources() {
  sharedView = { ...view, files: [book.file, alternateBook.file] };
  const fallback = vi.mocked(api).getMockImplementation()!;
  vi.mocked(api).mockImplementation(async (path, init) => {
    if (path === "/shares/test/book?file_id=source-file") return alternateBook;
    if (path === "/shares/test/chapters/0?file_id=source-file") return { idx: 0, title: "网络序章", content: '<p>网络版序章正文</p><img src="/api/images/illustration" alt="插图" />' };
    if (path === "/shares/test/chapters/1?file_id=source-file") return { idx: 1, title: "网络第一章", content: "<p>网络版第一章正文</p>" };
    return fallback(path, init);
  });
}

it("shows all shared sources and isolates chapters and illustrations when switching versions", async () => {
  multipleSources();
  const { router } = sharePage();
  fireEvent.click(await screen.findByRole("link", { name: "查看网络版目录" }));
  await screen.findByRole("link", { name: "网络第一章" });
  expect(router.state.location.search).toBe("?file_id=source-file");
  fireEvent.click(screen.getByRole("link", { name: "开始阅读" }));
  await screen.findByText("网络版序章正文");
  expect(screen.getByRole("img", { name: "插图" }).getAttribute("src")).toBe("/api/images/illustration");
  fireEvent.click(screen.getByRole("button", { name: /下一章/ }));
  await screen.findByText("网络版第一章正文");
  expect(screen.queryByText("第一章正文")).toBeNull();
  fireEvent.click(screen.getByRole("link", { name: "返回书籍详情" }));
  fireEvent.click(await screen.findByRole("link", { name: "查看精校版目录" }));
  fireEvent.click(await screen.findByRole("link", { name: "开始阅读" }));
  await screen.findByText("序章正文");
  expect(screen.queryByText("网络版序章正文")).toBeNull();
  expect(vi.mocked(api).mock.calls.every(([, init]) => !init?.method)).toBe(true);
});

it("supports reloading a deep reading URL for a non-default source", async () => {
  multipleSources();
  sharePage("/share/test/read?file_id=source-file&chapter=1");
  await screen.findByText("网络版第一章正文");
  expect(api).toHaveBeenCalledWith("/shares/test/chapters/1?file_id=source-file");
  expect(screen.getByRole("link", { name: "返回书籍详情" }).getAttribute("href")).toBe("/share/test?file_id=source-file");
});

it("does not request an unselected source from a manipulated URL", async () => {
  sharePage("/share/test/read?file_id=private-file");
  await screen.findByRole("alert");
  expect(screen.getByText("此书籍内容未包含在分享中")).toBeTruthy();
  expect(vi.mocked(api).mock.calls.map(([path]) => path)).toEqual(["/shares/test"]);
});

it("discards a pending directory after changing content sources", async () => {
  multipleSources();
  const fallback = vi.mocked(api).getMockImplementation()!;
  let finish!: (result: ShareBookResponse) => void;
  vi.mocked(api).mockImplementation((path, init) => path === "/shares/test/book?file_id=source-file"
    ? new Promise((resolve) => { finish = resolve; }) : fallback(path, init));
  const { router } = sharePage();
  fireEvent.click(await screen.findByRole("link", { name: "查看网络版目录" }));
  await act(() => router.navigate("/share/test"));
  await screen.findByRole("link", { name: "第一章" });
  await act(async () => finish(alternateBook));
  expect(screen.queryByRole("link", { name: "网络第一章" })).toBeNull();
  expect(screen.getByRole("link", { name: "第一章" })).toBeTruthy();
});

it.each(["", "?file_id=source-file"])("downloads a missing shared chapter only on explicit retry for source %s", async (query) => {
  multipleSources();
  const fallback = vi.mocked(api).getMockImplementation()!;
  const read = "/shares/test/chapters/0" + query;
  const retry = "/shares/test/chapters/0/retry" + query;
  let finish!: (chapter: Chapter) => void;
  vi.mocked(api).mockImplementation((path, init) => {
    if (path === read) return Promise.reject(new ApiError(404, "stored chapter"));
    if (path === retry && init?.method === "POST") return new Promise(resolve => { finish = resolve; });
    return fallback(path, init);
  });
  sharePage("/share/test/read" + query);
  expect((await screen.findByRole("alert")).textContent).toBe("章节正文尚未缓存，点击“重新加载章节”可补下载。");
  const chapterCalls = () => vi.mocked(api).mock.calls.filter(([path]) => path.includes("/chapters/"));
  expect(chapterCalls()).toEqual([[read]]);
  fireEvent.click(screen.getByRole("button", { name: "重新加载章节" }));
  await waitFor(() => expect(api).toHaveBeenCalledWith(retry, { method: "POST" }));
  expect(screen.queryByRole("button", { name: "重新加载章节" })).toBeNull();
  expect(screen.getByText("加载中…")).toBeTruthy();
  await act(async () => finish({ idx: 0, title: "序章", content: "<p>分享补下载正文</p>" }));
  await screen.findByText("分享补下载正文");
  expect(screen.queryByRole("alert")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /下一章/ }));
  await screen.findByText(query ? "网络版第一章正文" : "第一章正文");
  expect(chapterCalls()).toEqual([[read], [retry, { method: "POST" }], ["/shares/test/chapters/1" + query]]);
  expect(vi.mocked(api).mock.calls.every(([path]) => path.startsWith("/shares/test"))).toBe(true);
});

it("keeps a failed share download retryable without retrying automatically", async () => {
  const fallback = vi.mocked(api).getMockImplementation()!;
  let attempts = 0;
  vi.mocked(api).mockImplementation(async (path, init) => {
    if (path === "/shares/test/chapters/0") throw new ApiError(404, "stored chapter");
    if (path === "/shares/test/chapters/0/retry" && init?.method === "POST") {
      if (++attempts === 1) throw new ApiError(500, "书源暂时不可用");
      return { idx: 0, title: "序章", content: "<p>重试恢复的正文</p>" };
    }
    return fallback(path, init);
  });
  sharePage("/share/test/read");
  await screen.findByRole("alert");
  expect(attempts).toBe(0);
  fireEvent.click(screen.getByRole("button", { name: "重新加载章节" }));
  await screen.findByText("书源暂时不可用");
  expect(attempts).toBe(1);
  expect(screen.queryByText("加载中…")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "重新加载章节" }));
  await screen.findByText("重试恢复的正文");
  expect(attempts).toBe(2);
  expect(screen.queryByRole("alert")).toBeNull();
});

it("discards a pending share repair after switching content sources", async () => {
  multipleSources();
  const fallback = vi.mocked(api).getMockImplementation()!;
  let finish!: (chapter: Chapter) => void;
  vi.mocked(api).mockImplementation((path, init) => {
    if (path === "/shares/test/chapters/0") return Promise.reject(new ApiError(404, "stored chapter"));
    if (path === "/shares/test/chapters/0/retry" && init?.method === "POST") return new Promise(resolve => { finish = resolve; });
    return fallback(path, init);
  });
  const { router } = sharePage("/share/test/read");
  await screen.findByRole("alert");
  fireEvent.click(screen.getByRole("button", { name: "重新加载章节" }));
  await waitFor(() => expect(api).toHaveBeenCalledWith("/shares/test/chapters/0/retry", { method: "POST" }));
  await act(() => router.navigate("/share/test/read?file_id=source-file"));
  await screen.findByText("网络版序章正文");
  await act(async () => finish({ idx: 0, title: "旧章节", content: "<p>旧来源补下载正文</p>" }));
  expect(screen.queryByText("旧来源补下载正文")).toBeNull();
  expect(screen.getByText("网络版序章正文")).toBeTruthy();
  expect(vi.mocked(api).mock.calls.filter(([, init]) => init?.method)).toEqual([
    ["/shares/test/chapters/0/retry", { method: "POST" }],
  ]);
});
