import { beforeEach, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { api, ApiError } from "../src/api";
import { AddDialog } from "../src/components/AddDialog";
import type { PluginInstance, PluginSearchItem } from "../src/types";

vi.mock("../src/api", async (importOriginal) => ({ ...await importOriginal<typeof import("../src/api")>(), api: vi.fn() }));
const metadataSource: PluginInstance = { id: "bangumi", name: "Bangumi", wasm_file: "bangumi.wasm", enabled: true, capabilities: ["lookup"], source_info: { kind: "manual-id" } };
const contentSource: PluginInstance = { ...metadataSource, id: "wenku8", name: "文库", capabilities: ["lookup", "content"] };
const book = (id: string, title: string): PluginSearchItem => ({ kind: "book", id, title, authors: ["作者"], book_id: null });
const metadataSeries = { kind: "series", id: "metadata-series", title: "元数据系列", authors: ["作者"], book_id: null };
const contentSeries = { ...metadataSeries, id: "content-series", title: "正文系列" };
const metadataBooks = [book("metadata:1", "元数据卷一"), book("metadata:2", "元数据卷二")];
let contentBooks: PluginSearchItem[];
let submissions: FormData[];
let imports: { series_id: string; book_ids: string[] }[];
const onAdded = vi.fn();
const onClose = vi.fn();
const onSeriesCreated = vi.fn();
let handler: (path: string, init?: RequestInit) => Promise<unknown>;

beforeEach(() => {
  vi.clearAllMocks();
  submissions = []; imports = [];
  contentBooks = [book("candidate:1", "正文卷甲"), book("candidate:2", "正文卷乙")];
  handler = async (path, init) => {
    if (path.startsWith("/plugins/bangumi/series?")) return { series: metadataSeries, books: metadataBooks };
    if (path.startsWith("/plugins/wenku8/series?")) return { series: contentSeries, books: contentBooks };
    if (path.includes("/content?")) {
      const id = new URLSearchParams(path.split("?")[1]).get("q")!;
      return { id: id.replace("candidate:", "content:"), mode: "chapters", chapter_count: 10 };
    }
    if (path === "/plugins/bangumi/series") {
      imports.push(JSON.parse(init!.body as string));
      return { id: "library-series", title: metadataSeries.title };
    }
    if (path === "/books") {
      submissions.push(init!.body as FormData);
      return { series: null, books: [{ book: { id: "library-book", title: "元数据卷名" }, files: [] }] };
    }
    if (path === "/series") return { id: "manual-series", title: "手动系列" };
    throw new Error("Unexpected API call: " + path);
  };
  vi.mocked(api).mockImplementation((path, init) => handler(path, init) as ReturnType<typeof api>);
});
function dialog() {
  render(<AddDialog open initialMode="series" onClose={onClose} instances={[metadataSource, contentSource]} onAdded={onAdded} onSeriesCreated={onSeriesCreated} />);
}
async function pickSeries(pane: HTMLElement, source: PluginInstance, id = "series") {
  const user = userEvent.setup();
  await user.selectOptions(within(pane).getByRole("combobox"), source.id);
  await user.type(within(pane).getByLabelText("输入系列 ID（如网站系列书号）"), id);
  await user.click(within(pane).getByRole("button", { name: "查找" }));
  return user;
}
async function pickMetadata() {
  const user = userEvent.setup();
  await user.click(screen.getByLabelText("从插件源选择"));
  await pickSeries(document.querySelector(".source-search")! as HTMLElement, metadataSource);
  await screen.findByLabelText("元数据卷一对应的内容");
  return user;
}
function contentPane() { return document.querySelectorAll(".source-search")[1] as HTMLElement; }
async function pickContent() {
  await pickSeries(contentPane(), contentSource);
  await waitFor(() => expect((screen.getByRole("button", { name: "创建系列并添加内容" }) as HTMLButtonElement).disabled).toBe(false));
}

it("matches by returned order and submits independent confirmed content IDs with primary metadata", async () => {
  dialog(); const user = await pickMetadata(); await pickContent();
  expect((screen.getByLabelText("元数据卷一对应的内容") as HTMLSelectElement).value).toBe("wenku8:candidate:1");
  expect(submissions).toHaveLength(0); expect(imports).toHaveLength(0);
  expect(screen.queryByLabelText("元数据卷一内容的可见性")).toBeNull();
  expect(screen.getAllByText("插件源提供的书籍内容始终公开，所有登录用户均可阅读。")).toHaveLength(2);
  await user.type(screen.getByLabelText("元数据卷一内容的备注"), "精校版");
  await user.click(screen.getByRole("button", { name: "创建系列并添加内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(imports).toEqual([{ series_id: "metadata-series", book_ids: ["metadata:1", "metadata:2"] }]);
  expect(submissions.map((form) => [form.get("meta_plugin_source"), form.get("meta_plugin_book_id"), form.get("plugin_source"), form.get("plugin_book_id")])).toEqual([
    ["bangumi", "metadata:1", "wenku8", "content:1"], ["bangumi", "metadata:2", "wenku8", "content:2"],
  ]);
  expect(submissions[0].get("visibility")).toBe("public"); expect(submissions[0].get("label")).toBe("精校版");
  expect(submissions[1].get("visibility")).toBe("public");
  expect(onSeriesCreated).toHaveBeenCalledWith(expect.objectContaining({ title: "元数据系列" }));
});

it("lets users swap matches and keeps only the selected content for each member", async () => {
  dialog(); const user = await pickMetadata(); await pickContent();
  await user.selectOptions(screen.getByLabelText("元数据卷一对应的内容"), "wenku8:candidate:2");
  expect((screen.getByLabelText("元数据卷二对应的内容") as HTMLSelectElement).value).toBe("wenku8:candidate:1");
  await user.click(screen.getByRole("button", { name: "创建系列并添加内容" }));
  await waitFor(() => expect(submissions).toHaveLength(2));
  expect(submissions.map((form) => form.get("plugin_book_id"))).toEqual(["content:2", "content:1"]);
});

it("unmatched selected members remain metadata-only", async () => {
  dialog(); const user = await pickMetadata(); await pickContent();
  await user.selectOptions(screen.getByLabelText("元数据卷二对应的内容"), "");
  await user.click(screen.getByRole("button", { name: "创建系列并添加内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(imports[0].book_ids).toEqual(["metadata:1", "metadata:2"]); expect(submissions).toHaveLength(1);
});

it("does not shift ordinal matching when earlier metadata volumes are unchecked", async () => {
  dialog(); const user = await pickMetadata(); await pickContent();
  await user.click(screen.getByRole("checkbox", { name: "元数据卷一" }));
  await user.click(screen.getByRole("button", { name: "创建系列并添加内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(imports[0].book_ids).toEqual(["metadata:2"]); expect(submissions[0].get("plugin_book_id")).toBe("content:2");
});

it("allows an empty series with no implicit volume or content import", async () => {
  dialog(); const user = await pickMetadata();
  await user.click(screen.getByRole("button", { name: "取消全选" }));
  await user.click(screen.getByRole("button", { name: "新建系列", exact: true }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(imports[0].book_ids).toEqual([]); expect(submissions).toHaveLength(0);
});

it("leaves missing volumes unmatched and ignores extra content", async () => {
  contentBooks = [book("candidate:1", "正文卷甲")];
  dialog(); const user = await pickMetadata(); await pickContent();
  expect((screen.getByLabelText("元数据卷二对应的内容") as HTMLSelectElement).value).toBe("");
  expect(screen.getByText(/内容来源有 1 卷/)).toBeTruthy();
  await user.click(screen.getByRole("button", { name: "创建系列并添加内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce()); expect(submissions).toHaveLength(1);
});

it("does not import surplus provider volumes as metadata", async () => {
  contentBooks.push(book("candidate:3", "番外"));
  dialog(); const user = await pickMetadata(); await pickContent();
  await user.click(screen.getByRole("button", { name: "创建系列并添加内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(imports[0].book_ids).toEqual(["metadata:1", "metadata:2"]);
  expect(submissions.map((form) => form.get("plugin_book_id"))).toEqual(["content:1", "content:2"]);
});

it("allows uploading local volume files and adjusting their order", async () => {
  dialog(); const user = await pickMetadata();
  await user.click(screen.getByLabelText("上传文件"));
  const first = new File(["正文甲"], "甲.epub", { type: "application/epub+zip" });
  const second = new File(["正文乙"], "乙.txt", { type: "text/plain" });
  fireEvent.change(screen.getByLabelText("选择系列书籍文件"), { target: { files: [first, second] } });
  await user.selectOptions(screen.getByLabelText("元数据卷一对应的内容"), "file:1");
  expect(screen.queryByText("插件源提供的书籍内容始终公开，所有登录用户均可阅读。")).toBeNull();
  await user.selectOptions(screen.getByLabelText("元数据卷一内容的可见性"), "public");
  await user.click(screen.getByRole("button", { name: "创建系列并添加内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(submissions.map((form) => (form.get("file") as File).name)).toEqual(["乙.txt", "甲.epub"]);
  expect(submissions.map((form) => form.get("visibility"))).toEqual(["public", "private"]);
  expect(submissions[0].get("meta_plugin_book_id")).toBe("metadata:1");
  expect(submissions[0].has("plugin_source")).toBe(false);
});

it("blocks unconfirmed content but permits skipping an unavailable candidate", async () => {
  const original = handler;
  handler = (path, init) => path.includes("/content?") ? Promise.reject(new ApiError(404, { error: "Unavailable" })) : original(path, init);
  dialog(); const user = await pickMetadata(); await pickSeries(contentPane(), contentSource);
  await screen.findAllByText(/此 ID 暂无可用内容/);
  expect((screen.getByRole("button", { name: "新建系列", exact: true }) as HTMLButtonElement).disabled).toBe(true);
  await user.click(screen.getByRole("button", { name: "不添加内容", exact: true }));
  await user.click(screen.getByRole("button", { name: "新建系列", exact: true }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce()); expect(submissions).toHaveLength(0);
});

it("supports content IDs that differ from the content catalog's metadata IDs", async () => {
  dialog(); const user = await pickMetadata(); await pickContent();
  await user.click(screen.getByText("查看内容卷并修改内容 ID（2 卷）"));
  const input = screen.getByLabelText("正文卷甲的内容 ID");
  await user.clear(input); await user.type(input, "actual-text-id");
  await waitFor(() => expect((screen.getByRole("button", { name: "创建系列并添加内容" }) as HTMLButtonElement).disabled).toBe(false));
  await user.click(screen.getByRole("button", { name: "创建系列并添加内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(submissions[0].get("plugin_book_id")).toBe("actual-text-id");
  expect(submissions[0].get("meta_plugin_book_id")).toBe("metadata:1");
});

it("rejects different candidates that resolve to the same canonical content ID", async () => {
  const original = handler;
  handler = (path, init) => path.includes("/content?") ? Promise.resolve({ id: "same-text", mode: "chapters", chapter_count: 1 }) : original(path, init);
  dialog(); await pickMetadata(); await pickSeries(contentPane(), contentSource);
  await screen.findByText("同一内容 ID 不能重复匹配多卷，请调整对应关系。");
  expect((screen.getByRole("button", { name: "创建系列并添加内容" }) as HTMLButtonElement).disabled).toBe(true);
  expect(imports).toHaveLength(0);
});

it("keeps partial successes and retries failed content without reimporting metadata", async () => {
  const original = handler; let failed = false;
  handler = (path, init) => {
    if (path === "/books" && (init!.body as FormData).get("meta_plugin_book_id") === "metadata:2" && !failed) {
      failed = true; return Promise.reject(new Error("第二卷暂时失败"));
    }
    return original(path, init);
  };
  dialog(); const user = await pickMetadata(); await pickContent();
  await user.click(screen.getByRole("button", { name: "创建系列并添加内容" }));
  await screen.findByText("第二卷暂时失败");
  expect(onClose).not.toHaveBeenCalled(); expect(onSeriesCreated).not.toHaveBeenCalled();
  expect((screen.getByLabelText("元数据卷一对应的内容") as HTMLSelectElement).closest("fieldset")!.disabled).toBe(true);
  await user.click(screen.getByRole("button", { name: "重试失败内容" }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(imports).toHaveLength(1);
  expect(submissions.map((form) => form.get("meta_plugin_book_id"))).toEqual(["metadata:1", "metadata:2"]);
});

it("discards stale declaration previews when switching away from the content provider", async () => {
  let resolve!: (value: unknown) => void;
  const original = handler;
  handler = (path, init) => path.includes("/content?") ? new Promise((done) => { resolve = done; }) : original(path, init);
  dialog(); const user = await pickMetadata(); await pickSeries(contentPane(), contentSource);
  await waitFor(() => expect(resolve).toBeTypeOf("function"));
  await user.click(screen.getByLabelText("上传文件"));
  await act(async () => { resolve({ id: "stale-text", mode: "chapters", chapter_count: 1 }); });
  expect(screen.queryByText("stale-text")).toBeNull();
  await user.click(screen.getByRole("button", { name: "新建系列", exact: true }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce()); expect(submissions).toHaveLength(0);
});

it("blocks creation while the content source's volume lookup is pending", async () => {
  let resolve!: (value: unknown) => void;
  const original = handler;
  handler = (path, init) => path.startsWith("/plugins/wenku8/series?") ? new Promise((done) => { resolve = done; }) : original(path, init);
  dialog(); await pickMetadata(); await pickSeries(contentPane(), contentSource);
  await waitFor(() => expect(resolve).toBeTypeOf("function"));
  expect((screen.getByRole("button", { name: "新建系列", exact: true }) as HTMLButtonElement).disabled).toBe(true);
  expect(imports).toHaveLength(0);
  await act(async () => { resolve({ series: contentSeries, books: [] }); });
  await waitFor(() => expect((screen.getByRole("button", { name: "新建系列", exact: true }) as HTMLButtonElement).disabled).toBe(false));
});

it("bounds preview concurrency and drops queued requests after the source is cleared", async () => {
  contentBooks = Array.from({ length: 10 }, (_, index) => book("candidate:" + index, "正文卷" + index));
  const original = handler;
  const pending: ((value: unknown) => void)[] = [];
  handler = (path, init) => path.includes("/content?") ? new Promise((done) => { pending.push(done); }) : original(path, init);
  dialog(); const user = await pickMetadata(); await pickSeries(contentPane(), contentSource);
  await waitFor(() => expect(pending).toHaveLength(4));
  await user.click(screen.getByRole("button", { name: "不添加内容", exact: true }));
  await act(async () => { for (const resolve of pending) resolve({ id: "ignored", mode: "chapters", chapter_count: 1 }); });
  expect(pending).toHaveLength(4);
  expect(imports).toHaveLength(0);
});

it("does not acquire content if importing the metadata series fails", async () => {
  const original = handler;
  handler = (path, init) => path === "/plugins/bangumi/series" ? Promise.reject(new Error("元数据导入失败")) : original(path, init);
  dialog(); const user = await pickMetadata(); await pickContent();
  await user.click(screen.getByRole("button", { name: "创建系列并添加内容" }));
  await screen.findByText("元数据导入失败");
  expect(submissions).toHaveLength(0); expect(onClose).not.toHaveBeenCalled();
  expect((screen.getByLabelText("元数据卷一对应的内容") as HTMLSelectElement).closest("fieldset")!.disabled).toBe(false);
});

it("reconfirms candidates when the same content series is selected again", async () => {
  dialog(); const user = await pickMetadata(); await pickContent();
  const count = vi.mocked(api).mock.calls.filter(([path]) => path.includes("/content?")).length;
  await user.click(within(contentPane()).getByRole("button", { name: "作为内容来源" }));
  await waitFor(() => expect(vi.mocked(api).mock.calls.filter(([path]) => path.includes("/content?")).length).toBe(count + 2));
  await waitFor(() => expect((screen.getByRole("button", { name: "创建系列并添加内容" }) as HTMLButtonElement).disabled).toBe(false));
});

it("retains manual series creation without requiring a content source", async () => {
  dialog(); const user = userEvent.setup();
  await user.type(screen.getByPlaceholderText("系列名称"), "手动系列");
  await user.click(screen.getByRole("button", { name: "新建系列", exact: true }));
  await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  expect(submissions).toHaveLength(0); expect(onSeriesCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "manual-series" }));
});
