import { beforeEach, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useNavigate } from "react-router-dom";
import { api } from "../src/api";
import { SeriesBatchContentDialog } from "../src/components/SeriesBatchContentDialog";
import { SeriesPage } from "../src/pages/SeriesPage";
import type { BookEntry, FileMeta, PluginInstance, SeriesBrief, User } from "../src/types";

vi.mock("../src/api", async (importOriginal) => ({ ...await importOriginal<typeof import("../src/api")>(), api: vi.fn() }));
vi.mock("../src/auth", () => ({ useAuth: () => ({ user: viewer }) }));
let viewer: User | null;
const date = "2026-10-01T00:00:00Z";
const series: SeriesBrief = { id: "saved-series", title: "现有系列", authors: ["原作者"], description: "原简介", cover_url: null,
  ext: {}, volume_count: 3, created_by: "reader", created_at: date };
const source: PluginInstance = { id: "wenku8", name: "文库", wasm_file: "wenku8.wasm", enabled: true,
  capabilities: ["lookup", "content"], source_info: { kind: "manual-id" } };
function file(bookId: string): FileMeta {
  return { id: "file-" + bookId, book_id: bookId, source: "local", external_id: "file-" + bookId,
    format: "txt", label: "已有版本", visibility: "private", owner_id: "reader", chapter_count: 1,
    content_mode: "file", created_at: date };
}
let books: BookEntry[];
let submitted: FormData[];
let handler: (path: string, init?: RequestInit) => Promise<unknown>;
const onClose = vi.fn();
const onChanged = vi.fn(async () => {});
const names = ["第1卷 · 卷一", "第2卷 · 卷二", "第3卷 · 卷三"];
const match = (index: number) => screen.getByLabelText(names[index - 1] + "对应的内容");

beforeEach(() => {
  vi.clearAllMocks(); submitted = []; series.created_by = "reader"; series.ext = {};
  viewer = { id: "reader", username: "读者", role: "user", created_at: date };
  books = ["卷一", "卷二", "卷三"].map((title, index) => ({
    book: { id: "local:" + (index + 1), title, authors: ["原作者"], description: "原简介", cover_url: null,
      ext: { isbn: "9787302257646" }, created_by: "reader", created_at: date, series_id: series.id, volume_no: index + 1 },
    files: index === 0 ? [file("local:1")] : [], series,
  }));
  handler = async (path, init) => {
    if (path === "/plugins") return [source];
    if (path.startsWith("/plugins/wenku8/series?")) return {
      series: { kind: "series", id: "upstream-series", title: "另一版本系列", authors: ["另一作者"], book_id: null },
      books: [1, 2, 3].map((n) => ({ kind: "book", id: "candidate:" + n, title: "正文卷" + n, authors: ["另一作者"], book_id: null })),
    };
    if (path.includes("/content?")) return {
      id: new URLSearchParams(path.split("?")[1]).get("q")!.replace("candidate:", "content:"), mode: "chapters", chapter_count: 10,
    };
    if (path === "/books" && init?.method === "POST") {
      const form = init.body as FormData; submitted.push(form);
      const target = books.find((entry) => entry.book.id === form.get("book_id"))!;
      const edition = file(target.book.id);
      edition.id = "edition-" + submitted.length + "-" + target.book.id; edition.external_id = edition.id; edition.owner_id = viewer?.id ?? null;
      if (form.has("plugin_source")) { edition.source = String(form.get("plugin_source")); edition.external_id = String(form.get("plugin_book_id")); edition.format = "plugin"; edition.content_mode = "chapters"; }
      edition.visibility = form.get("visibility") === "public" ? "public" : "private"; edition.label = String(form.get("label") ?? "");
      target.files = [...target.files, edition];
      return { series, books: [{ ...target, files: [edition] }] };
    }
    if (path === "/series/" + series.id) return { series, books };
    if (path === "/sessions") return [];
    throw new Error("Unexpected API call: " + path);
  };
  vi.mocked(api).mockImplementation((path, init) => handler(path, init) as ReturnType<typeof api>);
});
function dialog() { render(<SeriesBatchContentDialog series={series} books={books} onClose={onClose} onChanged={onChanged} />); }
function page() {
  render(<MemoryRouter initialEntries={["/series/" + series.id]}><Routes><Route path="/series/:id" element={<SeriesPage />} /></Routes></MemoryRouter>);
}
async function chooseContent() {
  const user = userEvent.setup();
  const pane = document.querySelector(".source-search")! as HTMLElement;
  await within(pane).findByRole("option", { name: /文库/ });
  await user.selectOptions(within(pane).getByRole("combobox"), source.id);
  await user.type(within(pane).getByLabelText("输入系列 ID（如网站系列书号）"), "upstream-series");
  await user.click(within(pane).getByRole("button", { name: "查找" }));
  await waitFor(() => expect((screen.getByRole("button", { name: /添加 [1-3] 卷内容/ }) as HTMLButtonElement).disabled).toBe(false));
  return user;
}

it("selects empty volumes by default and attaches confirmed content using local book IDs", async () => {
  dialog();
  expect((screen.getByRole("checkbox", { name: names[0] }) as HTMLInputElement).checked).toBe(false);
  expect((screen.getByRole("checkbox", { name: names[1] }) as HTMLInputElement).checked).toBe(true);
  const before = structuredClone(books.map((entry) => entry.book));
  const user = await chooseContent();
  expect(screen.queryByLabelText("第2卷 · 卷二内容的可见性")).toBeNull();
  expect(screen.getAllByText("插件内容始终公开，所有登录用户均可阅读。")).toHaveLength(2);
  await user.type(screen.getByLabelText(names[1] + "内容的备注"), " 精校版 ");
  expect(submitted).toHaveLength(0);
  await user.click(screen.getByRole("button", { name: "添加 2 卷内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(submitted.map((form) => [form.get("book_id"), form.get("plugin_source"), form.get("plugin_book_id")])).toEqual([
    ["local:2", "wenku8", "content:2"], ["local:3", "wenku8", "content:3"],
  ]);
  expect(submitted[0].get("visibility")).toBe("public"); expect(submitted[0].get("label")).toBe("精校版");
  expect(submitted[1].get("visibility")).toBe("public");
  for (const form of submitted) for (const field of ["meta_plugin_source", "meta_plugin_book_id", "metadata_only", "title", "authors", "series_id"]) expect(form.has(field)).toBe(false);
  expect(books.map((entry) => entry.book)).toEqual(before); expect(books[0].files).toHaveLength(1);
  expect(onChanged).toHaveBeenCalledOnce();
});

it("supports adding another edition to an existing volume and swapping assignments", async () => {
  dialog(); const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "全选单卷" }));
  await chooseContent();
  await user.selectOptions(match(2), "wenku8:candidate:1");
  expect((match(1) as HTMLSelectElement).value).toBe("wenku8:candidate:2");
  await user.click(screen.getByRole("button", { name: "添加 3 卷内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(submitted.map((form) => [form.get("book_id"), form.get("plugin_book_id")])).toEqual([
    ["local:1", "content:2"], ["local:2", "content:1"], ["local:3", "content:3"],
  ]);
  expect(books[0].files).toHaveLength(2);
});

it("skips unmatched existing volumes without importing metadata", async () => {
  dialog(); const user = await chooseContent();
  await user.selectOptions(match(3), "");
  expect(within(match(3)).getByRole("option", { name: "跳过此卷（不添加内容）" })).toBeTruthy();
  await user.click(screen.getByRole("button", { name: "添加 1 卷内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce()); expect(submitted).toHaveLength(1);
  expect(books[2].files).toHaveLength(0);
});

it("maps uploaded files to existing members and preserves their titles and order", async () => {
  dialog(); const user = userEvent.setup(); await user.click(screen.getByLabelText("上传文件"));
  const files = [new File(["甲"], "甲.txt"), new File(["乙"], "乙.txt")];
  fireEvent.change(screen.getByLabelText("选择系列正文文件"), { target: { files } });
  await user.selectOptions(match(2), "file:0"); await user.selectOptions(match(3), "file:1");
  await user.click(screen.getByRole("button", { name: "添加 2 卷内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(submitted.map((form) => [form.get("book_id"), (form.get("file") as File).name])).toEqual([["local:2", "甲.txt"], ["local:3", "乙.txt"]]);
  expect(submitted.every((form) => !form.has("plugin_source"))).toBe(true);
  expect(books.map((entry) => entry.book.title)).toEqual(["卷一", "卷二", "卷三"]);
});

it("preserves successful attachments and retries only failed books", async () => {
  let fail = true; const original = handler;
  handler = (path, init) => {
    if (path === "/books" && (init!.body as FormData).get("book_id") === "local:3" && fail) { fail = false; return Promise.reject(new Error("第三卷暂时失败")); }
    return original(path, init);
  };
  dialog(); const user = await chooseContent();
  await user.click(screen.getByRole("button", { name: "添加 2 卷内容" }));
  await screen.findByText("第三卷暂时失败");
  expect(onClose).not.toHaveBeenCalled(); expect(onChanged).not.toHaveBeenCalled();
  expect((match(2) as HTMLSelectElement).closest("fieldset")!.disabled).toBe(true);
  await user.click(screen.getByRole("button", { name: "重试失败内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(submitted.map((form) => form.get("book_id"))).toEqual(["local:2", "local:3"]);
  expect(books[1].files).toHaveLength(1); expect(onChanged).toHaveBeenCalledOnce();
});

it("refreshes the page when closing a partially completed batch", async () => {
  const original = handler;
  handler = (path, init) => path === "/books" && (init!.body as FormData).get("book_id") === "local:3" ? Promise.reject(new Error("暂时失败")) : original(path, init);
  dialog(); const user = await chooseContent();
  await user.click(screen.getByRole("button", { name: "添加 2 卷内容" }));
  await screen.findByText("暂时失败");
  await user.click(screen.getByText("关闭", { selector: "button" }));
  expect(onChanged).toHaveBeenCalledOnce(); expect(onClose).toHaveBeenCalledOnce(); expect(books[1].files).toHaveLength(1);
});

it("prevents closing during submission and avoids implicit resubmission", async () => {
  let resolve!: (value: unknown) => void; const original = handler;
  handler = (path, init) => path === "/books" ? new Promise((done) => { resolve = done; }) : original(path, init);
  render(<><button>外部控件</button><SeriesBatchContentDialog series={series} books={books} onClose={onClose} onChanged={onChanged} /></>);
  const user = userEvent.setup(); await user.click(screen.getByRole("checkbox", { name: names[2] }));
  await chooseContent(); await user.click(screen.getByRole("button", { name: "添加 1 卷内容" }));
  await waitFor(() => expect(resolve).toBeTypeOf("function"));
  await user.click(screen.getByLabelText("关闭"));
  await user.tab(); expect(document.activeElement).toBe(screen.getByLabelText("关闭"));
  await user.keyboard("{Escape}");
  expect(onClose).not.toHaveBeenCalled();
  await act(async () => { resolve({ series, books: [{ ...books[1], files: [file("local:2")] }] }); });
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(vi.mocked(api).mock.calls.filter(([path, init]) => path === "/books" && init?.method === "POST")).toHaveLength(1);
});

it("keeps unreadable foreign volumes disabled without shifting later matches", async () => {
  books[1].book.created_by = "another-reader";
  dialog();
  expect((screen.getByRole("checkbox", { name: names[1] }) as HTMLInputElement).disabled).toBe(true);
  const user = await chooseContent();
  await user.click(screen.getByRole("button", { name: "添加 1 卷内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(submitted[0].get("book_id")).toBe("local:3"); expect(submitted[0].get("plugin_book_id")).toBe("content:3");
});

it("allows an admin to attach content to another creator's books", async () => {
  viewer!.role = "admin"; books.forEach((entry) => { entry.book.created_by = "another-reader"; });
  dialog(); const user = await chooseContent();
  await user.click(screen.getByRole("button", { name: "添加 2 卷内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce()); expect(submitted).toHaveLength(2);
});

it("does nothing when no content is matched or the draft is closed before submission", async () => {
  dialog(); const user = userEvent.setup();
  expect((screen.getByRole("button", { name: "添加 0 卷内容" }) as HTMLButtonElement).disabled).toBe(true);
  await chooseContent(); await user.click(screen.getByText("关闭", { selector: "button" }));
  expect(submitted).toHaveLength(0); expect(onChanged).not.toHaveBeenCalled();
});

it("treats an acquisition response without the requested file as a failure", async () => {
  const original = handler;
  handler = (path, init) => path === "/books" ? Promise.resolve({ series, books: [] }) : original(path, init);
  dialog(); const user = await chooseContent();
  await user.click(screen.getByRole("button", { name: "添加 2 卷内容" }));
  await screen.findByRole("alert"); expect(onClose).not.toHaveBeenCalled(); expect(onChanged).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "重试失败内容" })).toBeTruthy();
});

it("renders publication status from series extended metadata", async () => {
  series.ext = { status: "ongoing" };
  page();
  expect((await screen.findByText("连载中")).classList.contains("ext-status-ongoing")).toBe(true);
});

it("opens the batch dialog from the series page and reloads its visible files after success", async () => {
  page(); const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "批量添加内容" }));
  await screen.findByRole("dialog", { name: "批量添加内容" }); await chooseContent();
  await user.click(screen.getByRole("button", { name: "添加 2 卷内容" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "批量添加内容" })).toBeNull());
  await waitFor(() => expect(vi.mocked(api).mock.calls.filter(([path]) => path === "/series/" + series.id)).toHaveLength(2));
  expect(books.every((entry) => entry.files.length === 1)).toBe(true);
});

it("does not expose batch attachment to a read-only visitor", async () => {
  viewer!.id = "read-only"; books.forEach((entry) => { entry.files = []; });
  page(); await screen.findByRole("heading", { name: "现有系列", level: 1 });
  expect(screen.queryByRole("button", { name: "批量添加内容" })).toBeNull();
  expect(vi.mocked(api).mock.calls.some(([path]) => path === "/plugins")).toBe(false);
});

it("allows a reader to add an edition to a visible book without editing its metadata", async () => {
  viewer!.id = "another-reader"; books[0].files[0].visibility = "public";
  const before = structuredClone(books[0].book);
  page(); const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "批量添加内容" }));
  expect((screen.getByRole("checkbox", { name: names[1] }) as HTMLInputElement).disabled).toBe(true);
  await user.click(screen.getByRole("checkbox", { name: names[0] })); await chooseContent();
  await user.click(screen.getByRole("button", { name: "添加 1 卷内容" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "批量添加内容" })).toBeNull());
  expect(submitted).toHaveLength(1); expect(submitted[0].get("book_id")).toBe("local:1");
  expect(books[0].book).toEqual(before); expect(books[0].files[1].owner_id).toBe(viewer.id);
});

it("allows a volume owner to batch attach without requiring ownership of the series", async () => {
  series.created_by = "someone-else";
  page(); const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "批量添加内容" }));
  await screen.findByRole("dialog", { name: "批量添加内容" });
  expect(screen.queryByRole("button", { name: "编辑系列" })).toBeNull();
});

it("starts a fresh draft after successful attachment and page refresh", async () => {
  page(); const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "批量添加内容" }));
  await chooseContent(); await user.click(screen.getByRole("button", { name: "添加 2 卷内容" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "批量添加内容" })).toBeNull());
  await waitFor(() => expect(vi.mocked(api).mock.calls.filter(([path]) => path === "/series/" + series.id)).toHaveLength(2));
  await user.click(screen.getByRole("button", { name: "批量添加内容" }));
  expect(screen.getAllByRole("checkbox").every((checkbox) => !(checkbox as HTMLInputElement).checked)).toBe(true);
  expect((screen.getByRole("button", { name: "添加 0 卷内容" }) as HTMLButtonElement).disabled).toBe(true);
  expect(submitted).toHaveLength(2);
});

it("waits for refreshed file visibility before allowing another batch draft", async () => {
  let reads = 0; let resolve!: (value: unknown) => void; const original = handler;
  handler = (path, init) => path === "/series/" + series.id && ++reads === 2 ? new Promise((done) => { resolve = done; }) : original(path, init);
  page(); const user = userEvent.setup(); await user.click(await screen.findByRole("button", { name: "批量添加内容" }));
  await chooseContent(); await user.click(screen.getByRole("button", { name: "添加 2 卷内容" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "批量添加内容" })).toBeNull());
  expect((screen.getByRole("button", { name: "批量添加内容" }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => { resolve({ series, books }); });
  expect((screen.getByRole("button", { name: "批量添加内容" }) as HTMLButtonElement).disabled).toBe(false);
});

it("does not refresh or close a new series draft when an earlier batch finishes after navigation", async () => {
  const nextSeries = { ...series, id: "next-series", title: "另一个系列", volume_count: 1 };
  const nextBook = { ...books[1], book: { ...books[1].book, id: "next:1", series_id: nextSeries.id, title: "新卷", volume_no: 1 }, series: nextSeries };
  let resolve!: (value: unknown) => void; const original = handler;
  handler = (path, init) => path === "/books" ? new Promise((done) => { resolve = done; })
    : path === "/series/next-series" ? Promise.resolve({ series: nextSeries, books: [nextBook] }) : original(path, init);
  let navigate!: ReturnType<typeof useNavigate>;
  function Navigation() { navigate = useNavigate(); return null; }
  render(<MemoryRouter initialEntries={["/series/" + series.id]}><Navigation /><Routes><Route path="/series/:id" element={<SeriesPage />} /></Routes></MemoryRouter>);
  const user = userEvent.setup(); await user.click(await screen.findByRole("button", { name: "批量添加内容" }));
  await user.click(screen.getByRole("checkbox", { name: names[2] })); await chooseContent();
  await user.click(screen.getByRole("button", { name: "添加 1 卷内容" }));
  await waitFor(() => expect(resolve).toBeTypeOf("function"));
  await act(async () => { navigate("/series/next-series"); });
  await screen.findByRole("heading", { name: "另一个系列", level: 1 });
  await user.click(screen.getByRole("button", { name: "批量添加内容" }));
  await act(async () => { resolve({ series, books: [{ ...books[1], files: [file("local:2")] }] }); });
  expect(screen.getByRole("dialog", { name: "批量添加内容" })).toBeTruthy();
  expect(screen.getByLabelText("第1卷 · 新卷对应的内容")).toBeTruthy();
  expect(vi.mocked(api).mock.calls.filter(([path]) => path === "/series/" + series.id)).toHaveLength(1);
});

it("requires saved volume order before opening the matching draft", async () => {
  page(); const user = userEvent.setup();
  await screen.findByRole("button", { name: "批量添加内容" });
  await user.click(screen.getAllByTitle("下移")[0]);
  expect((screen.getByRole("button", { name: "批量添加内容" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByTitle("请先保存或取消卷顺序调整，再批量添加内容")).toBeTruthy();
  await user.click(screen.getByRole("button", { name: "取消", exact: true }));
  expect((screen.getByRole("button", { name: "批量添加内容" }) as HTMLButtonElement).disabled).toBe(false);
});
