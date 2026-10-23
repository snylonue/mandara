import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { api } from "../src/api";
import { ShareBookDialog } from "../src/components/ShareBookDialog";
import { BookDetailPage } from "../src/pages/BookDetailPage";
import type { BookEntry, FileMeta, ShareInfo, User } from "../src/types";

vi.mock("../src/api", async (importOriginal) => ({ ...await importOriginal<typeof import("../src/api")>(), api: vi.fn() }));
vi.mock("../src/auth", () => ({ useAuth: () => ({ user: viewer }) }));
let viewer: User;
const date = "2026-01-01T00:00:00Z";
const files: FileMeta[] = [
  { id: "local", book_id: "book", format: "epub", label: "精校版", source: "local", external_id: "local", owner_id: "user", visibility: "private", chapter_count: 2, created_at: date, content_mode: "file" },
  { id: "online", book_id: "book", format: "plugin", label: "网络版", source: "书源乙", external_id: "online", owner_id: null, visibility: "public", chapter_count: 3, created_at: date, content_mode: "chapters" },
];
const info: ShareInfo = { token: "token", url: "/share/token", kind: "book", file_id: "local", session_id: null, expires_at: null, created_at: date };
const foreign: FileMeta = { ...files[0], id: "foreign", owner_id: "other", visibility: "public", label: "他人版本" };
const entry: BookEntry = {
  book: { id: "book", title: "测试书籍", authors: ["测试作者"], description: null, cover_url: null, ext: {}, created_by: "user", created_at: date, series_id: null, volume_no: 0 },
  files: [foreign, ...files], series: null,
};
const onClose = vi.fn();
const onCreated = vi.fn();
beforeEach(() => {
  vi.resetAllMocks();
  viewer = { id: "user", username: "读者", role: "user", created_at: date };
  vi.mocked(api).mockResolvedValue(info);
});
function dialog(initialFileId: string | undefined = "local") {
  return render(<ShareBookDialog files={files} initialFileId={initialFileId} onClose={onClose} onCreated={onCreated} />);
}
const submit = () => screen.getByRole("button", { name: "生成分享链接" });
const check = (label: string) => screen.getByRole("checkbox", { name: new RegExp(label) }) as HTMLInputElement;

it("lets the user choose multiple content versions and sends only checked IDs", async () => {
  dialog();
  expect(check("精校版").checked).toBe(true);
  expect(check("网络版").checked).toBe(false);
  fireEvent.click(check("网络版"));
  expect(api).not.toHaveBeenCalled();
  fireEvent.click(submit());
  await waitFor(() => expect(onCreated).toHaveBeenCalledWith(info));
  expect(api).toHaveBeenCalledWith("/files/local/shares", {
    method: "POST", body: JSON.stringify({ kind: "book", file_ids: ["local", "online"] }),
  });
});

it("disables empty selection and changes the default file when its checkbox is cleared", async () => {
  dialog();
  fireEvent.click(check("精校版"));
  expect((submit() as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(check("网络版"));
  fireEvent.click(submit());
  await waitFor(() => expect(onCreated).toHaveBeenCalled());
  expect(api).toHaveBeenCalledWith("/files/online/shares", { method: "POST", body: JSON.stringify({ kind: "book", file_ids: ["online"] }) });
});

it("does not create a share when the draft is cancelled", () => {
  dialog();
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  expect(onClose).toHaveBeenCalledOnce();
  expect(api).not.toHaveBeenCalled();
});

it("keeps selected content after an error and permits a deliberate retry", async () => {
  vi.mocked(api).mockRejectedValueOnce(new Error("分享失败")).mockResolvedValueOnce(info);
  dialog();
  fireEvent.click(check("网络版"));
  fireEvent.click(submit());
  await screen.findByRole("alert");
  expect(check("网络版").checked).toBe(true);
  fireEvent.click(submit());
  await waitFor(() => expect(onCreated).toHaveBeenCalledWith(info));
  expect(api).toHaveBeenCalledTimes(2);
});

it("prevents duplicate submission and closing while busy, and ignores results after navigation", async () => {
  let finish!: (result: ShareInfo) => void;
  vi.mocked(api).mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const { unmount } = dialog();
  fireEvent.click(submit());
  fireEvent.click(screen.getByRole("button", { name: "正在生成…" }));
  fireEvent.click(screen.getByRole("button", { name: "关闭" }));
  fireEvent.keyDown(window, { key: "Escape" });
  expect(onClose).not.toHaveBeenCalled();
  expect(api).toHaveBeenCalledOnce();
  unmount();
  await act(async () => finish(info));
  expect(onCreated).not.toHaveBeenCalled();
});

function page(detail: BookEntry = entry) {
  vi.mocked(api).mockImplementation(async (path) => {
    if (path === "/books/book") return detail;
    if (path === "/plugins") return [];
    if (path.endsWith("/sessions")) return { sessions: [], last_session_id: null };
    if (path.endsWith("/shares")) return info;
    throw new Error("Unexpected API call: " + path);
  });
  return render(<MemoryRouter initialEntries={["/books/book"]}><Routes><Route path="/books/:id" element={<BookDetailPage />} /></Routes></MemoryRouter>);
}

it("opens the picker from the normal book hero without sharing other users' uploads", async () => {
  const { container } = page();
  await screen.findByRole("heading", { name: "测试书籍" });
  fireEvent.click(within(container.querySelector(".detail-info")! as HTMLElement).getByRole("button", { name: "分享" }));
  const modal = await screen.findByRole("dialog", { name: "选择要分享的内容" });
  expect(within(modal).queryByRole("checkbox", { name: /他人版本/ })).toBeNull();
  expect(check("精校版").checked).toBe(true);
  expect(check("网络版").checked).toBe(false);
  fireEvent.click(check("网络版"));
  fireEvent.click(submit());
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(screen.getByText("/share/token")).toBeTruthy();
  const sent = vi.mocked(api).mock.calls.filter(([, init]) => init?.method === "POST");
  expect(sent).toEqual([["/files/local/shares", { method: "POST", body: JSON.stringify({ kind: "book", file_ids: ["local", "online"] }) }]]);
});

it("starts per-source sharing with that version selected instead of the first file", async () => {
  page();
  const title = await screen.findByText("网络版");
  fireEvent.click(within(title.closest(".file-card")! as HTMLElement).getByRole("button", { name: "分享" }));
  expect(check("网络版").checked).toBe(true);
  expect(check("精校版").checked).toBe(false);
});

it("lets administrators select other users' public content", async () => {
  viewer.role = "admin";
  const { container } = page();
  await screen.findByRole("heading", { name: "测试书籍" });
  fireEvent.click(within(container.querySelector(".detail-info")! as HTMLElement).getByRole("button", { name: "分享" }));
  expect(check("他人版本").checked).toBe(true);
});

it("only offers visibility editing for uploads, including when a plugin provides an EPUB", async () => {
  viewer.role = "admin";
  page({ ...entry, files: [files[0], { ...files[1], format: "epub", content_mode: "file", owner_id: "user" }] });
  const plugin = (await screen.findByText("网络版")).closest(".file-card")! as HTMLElement;
  expect(within(plugin).queryByRole("button", { name: /设为/ })).toBeNull();
  const upload = screen.getByText("精校版").closest(".file-card")! as HTMLElement;
  expect(within(upload).getByRole("button", { name: "设为公开" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "添加内容" }));
  const modal = await screen.findByRole("dialog", { name: "添加内容到本书" });
  expect((within(modal).getByLabelText("可见性") as HTMLSelectElement).value).toBe("private");
  fireEvent.click(within(modal).getByLabelText("从插件源获取"));
  expect(within(modal).queryByLabelText("可见性")).toBeNull();
  expect(within(modal).getByText("插件源提供的书籍内容始终公开，所有登录用户均可阅读。")).toBeTruthy();
});
