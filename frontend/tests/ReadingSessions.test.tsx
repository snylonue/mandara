import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { useEffect } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { api } from "../src/api";
import { ReadingSessionsProvider, SessionControl, sessionStorageKey, useSelectedProgress } from "../src/readingSessions";
import { ReaderPage } from "../src/pages/ReaderPage";
import type { Position, ReadingProgress, ReadingSession } from "../src/types";

vi.mock("../src/api", () => ({ api: vi.fn() }));
let userId = "u";
vi.mock("../src/auth", () => ({ useAuth: () => ({ user: { id: userId } }) }));
let pendingPosition: Position | null = null;
vi.mock("../src/components/Reader", () => ({ Reader: ({ initialPosition, onProgress, sessionControls }: {
  initialPosition: Position; onProgress: (fraction: number, chapter: number) => void; sessionControls: React.ReactNode;
}) => {
  useEffect(() => () => {
    if (pendingPosition) { const p = pendingPosition; pendingPosition = null; onProgress("fraction" in p.in_chapter ? p.in_chapter.fraction : 0, p.chapter_idx); }
  }, [onProgress]);
  return <div>{sessionControls}<output data-testid="position">{JSON.stringify(initialPosition)}</output>
    <button onClick={() => { pendingPosition = { chapter_idx: 7, in_chapter: { fraction: 0.7 } }; }}>滚动</button>
  </div>;
} }));
const date = "2026-10-01T00:00:00Z";
const contexts: ReadingSession[] = [
  { id: "default", user_id: "u", label: "default", created_at: date },
  { id: "phone", user_id: "u", label: "手机", created_at: date },
];
const position = (chapter_idx: number, fraction: number): Position => ({ chapter_idx, in_chapter: { fraction } });
const progress = (session_id: string, pos: Position): ReadingProgress => ({ id: "p-" + session_id, session_id, file_id: "book", user_id: "u", label: session_id, position: pos, updated_at: date });
let saved: ReadingProgress[];
let available: ReadingSession[];
let events: string[];
let holdSave: Promise<void> | null;
let syncFailure: boolean;
beforeEach(() => {
  vi.clearAllMocks(); localStorage.clear(); userId = "u"; pendingPosition = null; holdSave = null; syncFailure = false;
  available = [...contexts];
  saved = [progress("default", position(4, 0.4)), progress("phone", position(2, 0.2))];
  events = [];
  vi.spyOn(window, "confirm").mockReturnValue(true);
  vi.mocked(api).mockImplementation(async (path, init) => {
    if (path === "/sessions" && init?.method === "POST") {
      const created = { ...contexts[1], id: "new", label: JSON.parse(String(init.body)).label };
      available = [...available, created]; return created;
    }
    if (path === "/sessions") return available;
    if (path === "/files/book") return { book: { id: "b", title: "测试书" }, chapters: [], toc: [] };
    if (path === "/files/book/sessions") return { file_id: "book", sessions: saved };
    const selected = path.split("/")[2];
    if (path.endsWith("/progress") && init?.method === "PUT") {
      events.push("save-start-" + selected);
      await holdSave;
      const p = progress(selected, JSON.parse(String(init.body)));
      saved = [...saved.filter(p => p.session_id !== selected), p];
      events.push("save-end-" + selected); return p;
    }
    if (path.endsWith("/sync")) {
      events.push("sync");
      if (syncFailure) throw new Error("同步失败");
      const source = JSON.parse(String(init?.body)).source_session_id;
      const p = progress(selected, saved.find(p => p.session_id === source)!.position);
      saved = [...saved.filter(p => p.session_id !== selected), p]; return p;
    }
    if (path.endsWith("/progress")) return saved.filter(p => p.session_id === selected);
    throw new Error("Unexpected API call " + path);
  });
});
function reader(url = "/read/book") {
  return render(<MemoryRouter initialEntries={[url]}><ReadingSessionsProvider><Routes>
    <Route path="/read/:id" element={<ReaderPage />} />
  </Routes></ReadingSessionsProvider></MemoryRouter>);
}
const shown = () => JSON.parse(screen.getByTestId("position").textContent!);

it("automatically selects the first session and persists a global selection across visits", async () => {
  const first = reader();
  await waitFor(() => expect(shown()).toEqual(position(4, 0.4)));
  fireEvent.change(screen.getByLabelText("阅读档案"), { target: { value: "phone" } });
  await waitFor(() => expect(shown()).toEqual(position(2, 0.2)));
  expect(localStorage.getItem(sessionStorageKey("u"))).toBe("phone");
  first.unmount(); reader();
  await waitFor(() => expect(shown()).toEqual(position(2, 0.2)));
});

it("adopts explicit context links and falls back from a deleted saved context", async () => {
  localStorage.setItem(sessionStorageKey("u"), "deleted");
  const view = reader("/read/book?session=phone");
  await waitFor(() => expect(shown()).toEqual(position(2, 0.2)));
  expect(localStorage.getItem(sessionStorageKey("u"))).toBe("phone");
  view.unmount(); localStorage.setItem(sessionStorageKey("u"), "deleted"); reader();
  await waitFor(() => expect(shown()).toEqual(position(4, 0.4)));
  expect(localStorage.getItem(sessionStorageKey("u"))).toBe("default");
});

it("falls back to the first session without treating a default label specially", async () => {
  available = [contexts[1], contexts[0]];
  localStorage.setItem(sessionStorageKey("u"), "deleted"); reader();
  await waitFor(() => expect(shown()).toEqual(position(2, 0.2)));
  expect(localStorage.getItem(sessionStorageKey("u"))).toBe("phone");
});

it("allows creating a session after all sessions were deleted without recreating one automatically", async () => {
  available = []; saved = [];
  localStorage.setItem(sessionStorageKey("u"), "deleted");
  reader("/read/book?session=deleted");
  await screen.findByText("新建档案");
  expect(screen.queryByLabelText("阅读档案")).toBeNull();
  expect(screen.queryByText("加载中…")).toBeNull();
  expect(localStorage.getItem(sessionStorageKey("u"))).toBeNull();
  expect(vi.mocked(api).mock.calls.filter(([, init]) => init?.method === "POST")).toHaveLength(0);
  vi.spyOn(window, "prompt").mockReturnValueOnce("重新开始");
  fireEvent.click(screen.getByText("新建档案"));
  await waitFor(() => expect(shown()).toEqual(position(0, 0)));
  expect(localStorage.getItem(sessionStorageKey("u"))).toBe("new");
});

it("creates an empty global context without creating a per-book session", async () => {
  reader(); await screen.findByTestId("position");
  vi.spyOn(window, "prompt").mockReturnValueOnce("平板");
  fireEvent.click(screen.getByText("新建档案"));
  await waitFor(() => expect(shown()).toEqual(position(0, 0)));
  expect(localStorage.getItem(sessionStorageKey("u"))).toBe("new");
  expect(api).toHaveBeenCalledWith("/sessions", { method: "POST", body: JSON.stringify({ label: "平板" }) });
  expect(events).toEqual([]);
});

it("clears an explicit chapter override when switching contexts", async () => {
  reader("/read/book?chapter=8");
  await waitFor(() => expect(shown()).toEqual(position(8, 0)));
  fireEvent.change(screen.getByLabelText("阅读档案"), { target: { value: "phone" } });
  await waitFor(() => expect(shown()).toEqual(position(2, 0.2)));
});

it("refreshes sync sources without moving the current reading position", async () => {
  saved = [progress("default", position(4, 0.4))]; reader();
  await screen.findByTestId("position");
  expect((screen.getByLabelText("选择同步来源") as HTMLSelectElement).disabled).toBe(false);
  expect((screen.getByRole("option", { name: "手机 · 本书暂无进度" }) as HTMLOptionElement).disabled).toBe(true);
  expect(screen.getByText(/其他档案尚无当前书籍文件/).id).toBe(screen.getByLabelText("选择同步来源").getAttribute("aria-describedby"));
  expect((screen.getByText("同步本书进度") as HTMLButtonElement).disabled).toBe(true);
  saved = [...saved, progress("phone", position(1, 0.1))];
  fireEvent.click(screen.getByText("刷新同步来源"));
  await waitFor(() => expect((screen.getByRole("option", { name: "手机 · 第 2 章" }) as HTMLOptionElement).disabled).toBe(false));
  expect(screen.queryByText(/其他档案尚无当前书籍文件/)).toBeNull();
  fireEvent.change(screen.getByLabelText("选择同步来源"), { target: { value: "phone" } });
  expect((screen.getByText("同步本书进度") as HTMLButtonElement).disabled).toBe(false);
  expect(shown()).toEqual(position(4, 0.4));
});

it("explains when no other sessions exist without disabling the source selector", async () => {
  available = [contexts[0]]; saved = [saved[0]]; reader();
  await screen.findByTestId("position");
  const select = screen.getByLabelText("选择同步来源") as HTMLSelectElement;
  expect(select.disabled).toBe(false);
  expect(select.options).toHaveLength(1);
  expect(select.options[0].textContent).toBe("暂无其他阅读档案");
  expect(screen.getByText(/请先新建其他阅读档案/).id).toBe(select.getAttribute("aria-describedby"));
  expect((screen.getByText("同步本书进度") as HTMLButtonElement).disabled).toBe(true);
  expect(events).toEqual([]);
});

it("includes a newly available source even if the global session list is stale", async () => {
  available = [contexts[0]]; saved = [saved[0]]; reader();
  await screen.findByTestId("position");
  saved = [...saved, progress("tablet", position(1, 0.1))];
  fireEvent.click(screen.getByText("刷新同步来源"));
  await screen.findByRole("option", { name: "tablet · 第 2 章" });
  fireEvent.change(screen.getByLabelText("选择同步来源"), { target: { value: "tablet" } });
  fireEvent.click(screen.getByText("同步本书进度"));
  await waitFor(() => expect(shown()).toEqual(position(1, 0.1)));
  expect(events).toEqual(["sync"]);
});

it("starts an untouched context at zero without borrowing another context's progress", async () => {
  saved = [progress("default", position(4, 0.4))];
  localStorage.setItem(sessionStorageKey("u"), "phone"); reader();
  await waitFor(() => expect(shown()).toEqual(position(0, 0)));
  expect(events).toEqual([]);
});

it("flushes pending writes before sync and restores the copied chapter and fraction", async () => {
  reader(); await screen.findByTestId("position");
  fireEvent.click(screen.getByText("滚动"));
  fireEvent.change(screen.getByLabelText("选择同步来源"), { target: { value: "phone" } });
  let release!: () => void;
  holdSave = new Promise<void>(resolve => { release = resolve; });
  fireEvent.click(screen.getByText("同步本书进度"));
  await waitFor(() => expect(events).toEqual(["save-start-default"]));
  expect(screen.queryByTestId("position")).toBeNull();
  await act(async () => release());
  await waitFor(() => expect(shown()).toEqual(position(2, 0.2)));
  expect(events).toEqual(["save-start-default", "save-end-default", "sync"]);
  expect(saved.find(p => p.session_id === "phone")!.position).toEqual(position(2, 0.2));
});

it("does not sync when cancelled and keeps the flushed position if syncing fails", async () => {
  reader(); await screen.findByTestId("position");
  fireEvent.change(screen.getByLabelText("选择同步来源"), { target: { value: "phone" } });
  vi.mocked(window.confirm).mockReturnValueOnce(false);
  fireEvent.click(screen.getByText("同步本书进度")); expect(events).toEqual([]);
  fireEvent.click(screen.getByText("滚动")); syncFailure = true;
  fireEvent.click(screen.getByText("同步本书进度"));
  await waitFor(() => expect(shown()).toEqual(position(7, 0.7)));
  expect(events).toEqual(["save-start-default", "save-end-default", "sync"]);
});

it("waits for pending saves before switching back to a context", async () => {
  reader(); await screen.findByTestId("position");
  fireEvent.click(screen.getByText("滚动"));
  fireEvent.change(screen.getByLabelText("阅读档案"), { target: { value: "phone" } });
  await waitFor(() => expect(shown()).toEqual(position(2, 0.2)));
  fireEvent.change(screen.getByLabelText("阅读档案"), { target: { value: "default" } });
  await waitFor(() => expect(shown()).toEqual(position(7, 0.7)));
});

it("shows shelf progress only from the selected context", async () => {
  function Shelf() {
    const rows = useSelectedProgress();
    return <><SessionControl /><output data-testid="progress">{JSON.stringify(rows)}</output></>;
  }
  render(<ReadingSessionsProvider><Shelf /></ReadingSessionsProvider>);
  await waitFor(() => expect(JSON.parse(screen.getByTestId("progress").textContent!)).toEqual([saved[0]]));
  fireEvent.change(screen.getByLabelText("阅读档案"), { target: { value: "phone" } });
  await waitFor(() => expect(JSON.parse(screen.getByTestId("progress").textContent!)).toEqual([saved[1]]));
});

it("isolates selection by account and ignores a late session list from the previous account", async () => {
  localStorage.setItem(sessionStorageKey("u"), "phone");
  let release!: (value: unknown) => void;
  const delayed = new Promise(resolve => { release = resolve; });
  vi.mocked(api).mockImplementationOnce(() => delayed);
  const view = render(<ReadingSessionsProvider><SessionControl /></ReadingSessionsProvider>);
  userId = "v";
  vi.mocked(api).mockImplementation(async () => [{ ...contexts[0], id: "v-default", user_id: "v" }]);
  view.rerender(<ReadingSessionsProvider><SessionControl /></ReadingSessionsProvider>);
  await waitFor(() => expect((screen.getByLabelText("阅读档案") as HTMLSelectElement).value).toBe("v-default"));
  await act(async () => release(contexts));
  expect((screen.getByLabelText("阅读档案") as HTMLSelectElement).value).toBe("v-default");
  expect(localStorage.getItem(sessionStorageKey("u"))).toBe("phone");
});
