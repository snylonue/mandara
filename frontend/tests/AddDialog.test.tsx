import { beforeEach, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { api, ApiError } from "../src/api";
import { AddDialog } from "../src/components/AddDialog";
import { PluginContentChoice } from "../src/components/PluginContentChoice";
import type { PluginInstance, PluginSearchItem } from "../src/types";

vi.mock("../src/api", async (importOriginal) => ({
  ...await importOriginal<typeof import("../src/api")>(), api: vi.fn(),
}));

const source: PluginInstance = {
  id: "wenku8", wasm_file: "wenku8.wasm", name: "轻小说文库", enabled: true,
  capabilities: ["lookup", "content"], source_info: { kind: "manual-id", id_kind: "opaque" },
};
const metadataOnly: PluginInstance = { ...source, id: "bangumi", name: "Bangumi", capabilities: ["lookup"] };
const item: PluginSearchItem = { kind: "book", id: "3617:v1", title: "示例小说 第一卷", authors: ["作者"], book_id: null };
let submitted: FormData[];
const onAdded = vi.fn();

beforeEach(() => {
  submitted = [];
  vi.clearAllMocks();
  vi.mocked(api).mockImplementation(async (path, init) => {
    if (path.includes("/lookup?")) return { total: 1, items: [item] };
    if (path.includes("/content?")) return { id: "3617:content1", mode: "chapters", chapter_count: 12 };
    if (path === "/books") {
      submitted.push(init!.body as FormData);
      return { series: null, books: [{ book: { id: "book-1", title: item.title }, files: [] }] };
    }
    throw new Error("Unexpected API call: " + path);
  });
});

function dialog() {
  return render(<AddDialog open onClose={vi.fn()} instances={[source, metadataOnly]} onAdded={onAdded} />);
}

async function pickMetadata(instance = source) {
  const user = userEvent.setup();
  await user.click(screen.getByLabelText("从插件源选择"));
  const pane = document.querySelector(".source-search")! as HTMLElement;
  await user.selectOptions(within(pane).getByRole("combobox"), instance.id);
  await user.type(within(pane).getByLabelText("输入书目 ID"), "3617");
  await user.click(within(pane).getByRole("button", { name: "查找" }));
  await user.click(await within(pane).findByRole("button", { name: "作为元数据来源" }));
  return user;
}

it("adds confirmed source content without searching a second time and keeps per-version settings", async () => {
  dialog();
  const user = await pickMetadata();
  const add = await screen.findByRole("button", { name: "添加此来源的内容" });
  await waitFor(() => expect((add as HTMLButtonElement).disabled).toBe(false));
  await user.click(add);
  const note = screen.getByPlaceholderText("备注（如：epub 精校版）");
  await user.type(note, "精校版");
  expect(document.activeElement).toBe(note);
  expect(screen.queryByLabelText("可见性")).toBeNull();
  expect(screen.getByText("插件内容始终公开，所有登录用户均可阅读。")).toBeTruthy();
  expect((screen.getByRole("button", { name: "已选择此内容" }) as HTMLButtonElement).disabled).toBe(true);
  await user.click(screen.getByRole("button", { name: "添加书籍" }));
  await waitFor(() => expect(onAdded).toHaveBeenCalledOnce());
  expect(submitted).toHaveLength(1);
  expect(submitted[0].get("meta_plugin_book_id")).toBe(item.id);
  expect(submitted[0].get("plugin_book_id")).toBe("3617:content1");
  expect(submitted[0].get("meta_plugin_source")).toBe(source.id);
  expect(submitted[0].get("plugin_source")).toBe(source.id);
  expect(submitted[0].get("visibility")).toBe("public");
  expect(submitted[0].get("label")).toBe("精校版");
  expect(vi.mocked(api).mock.calls.filter(([path]) => path.includes("/lookup?"))).toHaveLength(1);
});

it("keeps source content optional and allows removing and reselecting it", async () => {
  dialog();
  const user = await pickMetadata();
  const add = screen.getByRole("button", { name: "添加此来源的内容" });
  await waitFor(() => expect((add as HTMLButtonElement).disabled).toBe(false));
  await user.click(add);
  await user.click(screen.getByRole("button", { name: "移除" }));
  expect((screen.getByRole("button", { name: "添加此来源的内容" }) as HTMLButtonElement).disabled).toBe(false);
  await user.click(screen.getByRole("button", { name: "仅添加书目" }));
  await waitFor(() => expect(submitted).toHaveLength(1));
  expect(submitted[0].get("metadata_only")).toBe("true");
  expect(submitted[0].has("plugin_source")).toBe(false);
});

it("does not offer content from metadata-only sources", async () => {
  dialog();
  const user = await pickMetadata(metadataOnly);
  expect(screen.queryByRole("button", { name: "添加此来源的内容" })).toBeNull();
  expect(vi.mocked(api).mock.calls.some(([path]) => path.includes("/content?"))).toBe(false);
  await user.click(screen.getByRole("button", { name: "仅添加书目" }));
  await waitFor(() => expect(submitted).toHaveLength(1));
  expect(submitted[0].get("meta_plugin_source")).toBe(metadataOnly.id);
});

it("uses a local file as both metadata and content without manual entry", async () => {
  dialog();
  const file = new File(["第一章\n正文"], "示例小说.txt", { type: "text/plain" });
  fireEvent.change(screen.getByLabelText("请选择用于读取书目信息的本地文件"), { target: { files: [file] } });
  await userEvent.click(screen.getByRole("button", { name: "添加书籍" }));
  await waitFor(() => expect(submitted).toHaveLength(1));
  expect(submitted[0].get("file")).toBe(file);
  expect(submitted[0].has("title")).toBe(false);
  expect(submitted[0].has("meta_plugin_source")).toBe(false);
  expect(submitted[0].get("visibility")).toBe("private");
});

it("replaces automatically selected local content when choosing a different local source", async () => {
  dialog();
  const original = new File(["原正文"], "原书.txt");
  const replacement = new File(["新正文"], "新书.txt");
  const picker = screen.getByLabelText("请选择用于读取书目信息的本地文件");
  fireEvent.change(picker, { target: { files: [original] } });
  fireEvent.change(picker, { target: { files: [replacement] } });
  expect(screen.queryByText("原书.txt")).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "添加书籍" }));
  await waitFor(() => expect(submitted).toHaveLength(1));
  expect(submitted[0].get("file")).toBe(replacement);
});

it("creates from the selected metadata file even when another version was picked earlier", async () => {
  dialog();
  const user = await pickMetadata();
  const add = screen.getByRole("button", { name: "添加此来源的内容" });
  await waitFor(() => expect((add as HTMLButtonElement).disabled).toBe(false));
  await user.click(add);
  await user.click(screen.getByLabelText("本地文件"));
  const file = new File(["正文"], "本地版.txt");
  fireEvent.change(screen.getByLabelText("请选择用于读取书目信息的本地文件"), { target: { files: [file] } });
  await user.click(screen.getByRole("button", { name: "添加 2 项内容" }));
  await waitFor(() => expect(submitted).toHaveLength(2));
  expect(submitted[0].get("file")).toBe(file);
  expect(submitted[0].has("meta_plugin_source")).toBe(false);
  expect(submitted[1].get("book_id")).toBe("book-1");
  expect(submitted[1].get("plugin_book_id")).toBe("3617:content1");
});

it("invalidates the selected metadata when switching plugin sources", async () => {
  dialog();
  const user = await pickMetadata();
  const pane = document.querySelector(".source-search")! as HTMLElement;
  await user.selectOptions(within(pane).getByRole("combobox"), metadataOnly.id);
  expect(screen.queryByText("已选书目：《示例小说 第一卷》")).toBeNull();
  await user.click(screen.getByRole("button", { name: "仅添加书目" }));
  expect(await screen.findByText("请先从插件源选择一本书作为元数据来源")).toBeTruthy();
  expect(submitted).toHaveLength(0);
});

it("allows metadata-only submission when a content candidate is unavailable", async () => {
  const implementation = vi.mocked(api).getMockImplementation()!;
  vi.mocked(api).mockImplementation((path, init) => path.includes("/content?")
    ? Promise.reject(new ApiError(404, "not found")) : implementation(path, init));
  dialog();
  const user = await pickMetadata();
  await screen.findByText(/此 ID 暂无可用内容/);
  expect((screen.getByRole("button", { name: "添加此来源的内容" }) as HTMLButtonElement).disabled).toBe(true);
  await user.click(screen.getByRole("button", { name: "仅添加书目" }));
  await waitFor(() => expect(submitted).toHaveLength(1));
  expect(submitted[0].get("metadata_only")).toBe("true");
});

it("can select a different content ID and retry after a transient error", async () => {
  const implementation = vi.mocked(api).getMockImplementation()!;
  let fail = true;
  vi.mocked(api).mockImplementation((path, init) => {
    if (path.includes("/content?") && fail) { fail = false; return Promise.reject(new ApiError(502, "来源暂时不可用")); }
    return implementation(path, init);
  });
  dialog();
  const user = await pickMetadata();
  await screen.findByText("来源暂时不可用");
  await user.click(screen.getByRole("button", { name: "重新确认" }));
  await screen.findByText("来源提供 12 章正文，添加后在后台获取。");
  await user.click(screen.getByText("正文使用不同 ID？"));
  const input = screen.getByLabelText("内容 ID");
  await user.clear(input);
  await user.type(input, "independent-id");
  await waitFor(() => expect(vi.mocked(api).mock.calls.some(([path]) => path.includes("q=independent-id"))).toBe(true));
  const button = screen.getByRole("button", { name: "添加此来源的内容" });
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  await user.click(button);
  await user.click(screen.getByRole("button", { name: "添加书籍" }));
  await waitFor(() => expect(submitted).toHaveLength(1));
  expect(submitted[0].get("meta_plugin_book_id")).toBe(item.id);
  expect(submitted[0].get("plugin_book_id")).toBe("3617:content1");
});

it("attaches local and source content to the same selected plugin metadata", async () => {
  dialog();
  const user = await pickMetadata();
  const add = screen.getByRole("button", { name: "添加此来源的内容" });
  await waitFor(() => expect((add as HTMLButtonElement).disabled).toBe(false));
  await user.click(add);
  await user.click(screen.getByText("添加其他来源或版本"));
  const file = new File(["正文"], "另一版本.txt");
  fireEvent.change(screen.getByLabelText("上传文件"), { target: { files: [file] } });
  await user.click(screen.getByRole("button", { name: "添加 2 项内容" }));
  await waitFor(() => expect(submitted).toHaveLength(2));
  expect(submitted[0].get("meta_plugin_book_id")).toBe(item.id);
  expect(submitted[0].get("plugin_book_id")).toBe("3617:content1");
  expect(submitted[1].get("book_id")).toBe("book-1");
  expect(submitted[1].get("file")).toBe(file);
  expect(submitted[1].has("meta_plugin_book_id")).toBe(false);
});

it("allows manual metadata-only creation and rejects removal of the local metadata source", async () => {
  dialog();
  const user = userEvent.setup();
  const file = new File(["正文"], "本地.txt");
  fireEvent.change(screen.getByLabelText("请选择用于读取书目信息的本地文件"), { target: { files: [file] } });
  await user.click(screen.getByRole("button", { name: "移除" }));
  await user.click(screen.getByRole("button", { name: "添加书籍" }));
  expect(screen.getByText("请选择用于读取书目信息的本地文件")).toBeTruthy();
  expect(submitted).toHaveLength(0);
  await user.click(screen.getByLabelText("手动填写"));
  await user.type(screen.getByPlaceholderText("书名"), "手填书目");
  await user.click(screen.getByRole("button", { name: "仅添加书目" }));
  await waitFor(() => expect(submitted).toHaveLength(1));
  expect(submitted[0].get("title")).toBe("手填书目");
  expect(submitted[0].get("metadata_only")).toBe("true");
});

it("discards an in-flight preview after the metadata selection changes", async () => {
  let resolveOld!: (value: unknown) => void;
  vi.mocked(api).mockImplementationOnce(() => new Promise((resolve) => { resolveOld = resolve; }));
  const picked = vi.fn();
  const view = render(<PluginContentChoice key="old" pick={{ instance: source, item }} disabled={false} selectedIds={[]} onPick={picked} />);
  await waitFor(() => expect(resolveOld).toBeTypeOf("function"));
  view.rerender(<PluginContentChoice key="new" pick={{ instance: source, item: { ...item, id: "other" } }} disabled={false} selectedIds={[]} onPick={picked} />);
  resolveOld({ id: "obsolete", mode: "chapters", chapter_count: 99 });
  await screen.findByText("来源提供 12 章正文，添加后在后台获取。");
  await userEvent.click(screen.getByRole("button", { name: "添加此来源的内容" }));
  expect(picked.mock.calls[0][0].item.id).toBe("3617:content1");
});
