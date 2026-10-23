import { beforeEach, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { api } from "../src/api";
import { SourceSearchPane } from "../src/components/SourceSearch";
import type { PluginInstance } from "../src/types";

vi.mock("../src/api", async (original) => ({ ...await original<typeof import("../src/api")>(), api: vi.fn() }));
const source: PluginInstance = { id: "bangumi", name: "bangumi", wasm_file: "test.wasm", enabled: true, capabilities: ["search", "lookup"] };
const item = { kind: "book", id: "123", title: "示例书籍", authors: ["作者"], description: "一段很长的简介", book_id: null };
beforeEach(() => { vi.clearAllMocks(); vi.mocked(api).mockResolvedValue({ total: 1, items: [item] }); });
function pane(instances = [source]) {
  const onPick = vi.fn();
  render(<SourceSearchPane instances={instances} filter={() => true} onPick={onPick} />);
  return onPick;
}

it("prioritizes ID lookup without issuing an empty search and supports Enter", async () => {
  pane(); const user = userEvent.setup();
  await user.selectOptions(screen.getByRole("combobox"), source.id);
  expect(api).not.toHaveBeenCalled();
  expect(screen.queryByPlaceholderText("搜索书名 / 作者…")).toBeNull();
  expect(screen.getByRole("option", { name: "bangumi", exact: true })).toBeTruthy();
  expect((screen.getByRole("button", { name: "查找" }) as HTMLButtonElement).disabled).toBe(true);
  await user.type(screen.getByLabelText("输入书籍 ID"), "123{Enter}");
  await screen.findByText(item.title);
  expect(api).toHaveBeenCalledOnce();
  expect(vi.mocked(api).mock.calls[0][0]).toBe("/plugins/bangumi/lookup?q=123");
  expect(screen.getByText(item.description).closest("details")!.open).toBe(false);
});

it("only searches after explicitly choosing search and entering a query", async () => {
  pane(); const user = userEvent.setup();
  await user.selectOptions(screen.getByRole("combobox"), source.id);
  await user.click(screen.getByRole("button", { name: "尝试搜索" }));
  expect(api).not.toHaveBeenCalled();
  await user.type(screen.getByPlaceholderText("搜索书名 / 作者…"), "书名");
  await screen.findByText(item.title);
  expect(api).toHaveBeenCalledOnce();
  expect(vi.mocked(api).mock.calls[0][0]).toContain("/search?");
  await user.click(screen.getByRole("button", { name: "按 ID 获取" }));
  expect(screen.queryByText(item.title)).toBeNull();
  expect(screen.getByLabelText("输入书籍 ID")).toBeTruthy();
});

it("supports search-only sources without offering unsupported ID lookup", async () => {
  pane([{ ...source, capabilities: ["search"] }]); const user = userEvent.setup();
  await user.selectOptions(screen.getByRole("combobox"), source.id);
  expect(screen.queryByLabelText("输入书籍 ID")).toBeNull();
  await user.type(screen.getByPlaceholderText("搜索书名 / 作者…"), "书名");
  await screen.findByText(item.title);
});

it("clears the ID and ignores pending lookup results when switching sources", async () => {
  let resolve!: (value: unknown) => void;
  vi.mocked(api).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
  pane([source, { ...source, id: "other" }]);
  fireEvent.change(screen.getByRole("combobox"), { target: { value: source.id } });
  fireEvent.change(screen.getByLabelText("输入书籍 ID"), { target: { value: "123" } });
  fireEvent.click(screen.getByRole("button", { name: "查找" }));
  await waitFor(() => expect(resolve).toBeTypeOf("function"));
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "other" } });
  await act(async () => { resolve({ total: 1, items: [item] }); });
  expect(screen.queryByText(item.title)).toBeNull();
  expect((screen.getByLabelText("输入书籍 ID") as HTMLInputElement).value).toBe("");
});
