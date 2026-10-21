import { beforeEach, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { api, ApiError } from "../src/api";
import { ContentFilePicker } from "../src/components/ContentFilePicker";
import { SourceSearchPane } from "../src/components/SourceSearch";
import { SeriesContentMatch } from "../src/components/SeriesContentMatch";
import type { PluginContentOption, PluginInstance } from "../src/types";

vi.mock("../src/api", async (original) => ({ ...await original<typeof import("../src/api")>(), api: vi.fn() }));
const source: PluginInstance = { id: "novelia", wasm_file: "novelia.wasm", name: "Novelia", enabled: true,
  capabilities: ["search", "lookup", "content", "book-file", "content-list"] };
const options: PluginContentOption[] = [
  { id: "wenku-volume:series:encoded-file", title: "第一卷 100%.epub", available: true },
  { id: "wenku-volume:series:other-file", title: "第二卷.txt", available: true },
  { id: "wenku-volume:series:untranslated", title: "第三卷.epub", available: false },
];
beforeEach(() => { vi.clearAllMocks(); vi.mocked(api).mockResolvedValue(options); });

it("lists filenames from only a book reference and returns the exact selected identity", async () => {
  const onPick = vi.fn();
  const user = userEvent.setup();
  render(<ContentFilePicker instance={source} onPick={onPick} />);
  await user.type(screen.getByLabelText("书籍 ID 或链接"), "https://n.novelia.cc/wenku/series");
  await user.click(screen.getByRole("button", { name: "列出文件" }));
  await screen.findByText(options[0].title);
  expect(screen.queryByText(options[0].id)).toBeNull();
  const rows = screen.getAllByRole("listitem");
  expect(rows).toHaveLength(3);
  expect((within(rows[2]).getByRole("button") as HTMLButtonElement).disabled).toBe(true);
  await user.click(within(rows[1]).getByRole("button", { name: "选择此文件" }));
  expect(onPick).toHaveBeenCalledWith([options[1]], "https://n.novelia.cc/wenku/series");
  expect(api).toHaveBeenCalledOnce();
  expect(vi.mocked(api).mock.calls[0][0]).toContain("/content-options?q=");
});

it("ignores stale results after editing the book reference", async () => {
  let finish!: (value: PluginContentOption[]) => void;
  vi.mocked(api).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
  render(<ContentFilePicker instance={source} onPick={vi.fn()} />);
  const input = screen.getByLabelText("书籍 ID 或链接");
  fireEvent.change(input, { target: { value: "old" } });
  fireEvent.click(screen.getByRole("button", { name: "列出文件" }));
  fireEvent.change(input, { target: { value: "new" } });
  finish(options);
  await waitFor(() => expect(screen.queryByText(options[0].title)).toBeNull());
  expect((screen.getByRole("button", { name: "列出文件" }) as HTMLButtonElement).disabled).toBe(false);
});

it("distinguishes an empty book from lookup failure and permits retry", async () => {
  vi.mocked(api).mockRejectedValueOnce(new ApiError(404, "missing")).mockResolvedValueOnce([]);
  const user = userEvent.setup();
  render(<ContentFilePicker instance={source} onPick={vi.fn()} />);
  await user.type(screen.getByLabelText("书籍 ID 或链接"), "series");
  await user.click(screen.getByRole("button", { name: "列出文件" }));
  await screen.findByRole("alert");
  await user.click(screen.getByRole("button", { name: "列出文件" }));
  await screen.findByText("这本书下还没有文件。");
  expect(screen.queryByRole("alert")).toBeNull();
});

it("uses content discovery instead of metadata lookup in the content pane", async () => {
  const user = userEvent.setup();
  const onPick = vi.fn();
  render(<SourceSearchPane purpose="content" instances={[source]} filter={() => true} onPick={onPick} />);
  await user.selectOptions(screen.getByRole("combobox"), source.id);
  expect(api).not.toHaveBeenCalled();
  expect(screen.queryByLabelText("输入书目 ID")).toBeNull();
  await user.type(screen.getByLabelText("书籍 ID 或链接"), "series");
  await user.click(screen.getByRole("button", { name: "列出文件" }));
  await screen.findByText(options[0].title);
  await user.click(within(screen.getAllByRole("listitem")[0]).getByRole("button"));
  expect(onPick).toHaveBeenCalledWith(expect.objectContaining({ item: expect.objectContaining({ id: options[0].id, title: options[0].title }) }));
});

it("clears its busy state when a pending source is unmounted", async () => {
  vi.mocked(api).mockReturnValue(new Promise(() => {}));
  const onLoadingChange = vi.fn();
  const view = render(<ContentFilePicker instance={source} initialReference="series" onPick={vi.fn()} onLoadingChange={onLoadingChange} />);
  await waitFor(() => expect(onLoadingChange).toHaveBeenLastCalledWith(true));
  view.unmount();
  expect(onLoadingChange).toHaveBeenLastCalledWith(false);
});

it("matches listed files to local volumes without exposing internal IDs", async () => {
  vi.mocked(api).mockImplementation(async (path) => {
    if (path.includes("/content-options?")) return options;
    if (path.includes("/content?")) return { id: new URLSearchParams(path.split("?")[1]).get("q"), mode: "file", chapter_count: 0 };
    throw new Error("Unexpected API call: " + path);
  });
  const onChange = vi.fn();
  const user = userEvent.setup();
  render(<SeriesContentMatch targets={[{ id: "local-1", title: "本地第一卷" }, { id: "local-2", title: "本地第二卷" }]}
    selectedIds={["local-1", "local-2"]} instances={[source]} disabled={false} completedIds={[]} failures={{}} onChange={onChange} />);
  await user.selectOptions(screen.getAllByRole("combobox")[0], source.id);
  await user.type(screen.getByLabelText("书籍 ID 或链接"), "series");
  await user.click(screen.getByRole("button", { name: "列出文件" }));
  await user.click(await screen.findByRole("button", { name: "使用这些文件并匹配卷册" }));
  await waitFor(() => expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ ready: true, assignments: [
    expect.objectContaining({ targetId: "local-1", content: { kind: "plugin", source: "novelia", id: options[0].id } }),
    expect.objectContaining({ targetId: "local-2", content: { kind: "plugin", source: "novelia", id: options[1].id } }),
  ] })));
  expect(screen.queryByText(options[0].id)).toBeNull();
  expect(screen.queryByText("内容 ID")).toBeNull();
});

it("offers all available files for explicit series matching without metadata import", async () => {
  const user = userEvent.setup();
  const onPick = vi.fn();
  render(<SourceSearchPane purpose="content" target="series" instances={[source]} filter={() => true} onPick={onPick} onClear={vi.fn()} />);
  await user.selectOptions(screen.getByRole("combobox"), source.id);
  await user.type(screen.getByLabelText("书籍 ID 或链接"), "series");
  await user.click(screen.getByRole("button", { name: "列出文件" }));
  await user.click(await screen.findByRole("button", { name: "使用这些文件并匹配卷册" }));
  expect(onPick.mock.calls[0][0].books.map((book: { id: string }) => book.id)).toEqual(options.slice(0, 2).map((option) => option.id));
  expect(api).toHaveBeenCalledOnce();
});
