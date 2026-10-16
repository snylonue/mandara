import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { ReaderPage } from "../src/pages/ReaderPage";
import { ReadingSessionsProvider } from "../src/readingSessions";

vi.mock("../src/auth", () => ({ useAuth: () => ({ user: { id: "user" } }) }));

beforeEach(() => {
  localStorage.clear();
  HTMLElement.prototype.scrollTo = vi.fn();
});
afterEach(() => {
  vi.unstubAllGlobals();
  delete (HTMLElement.prototype as Partial<HTMLElement>).scrollTo;
  localStorage.clear();
});

it("reads storage first and sends an explicit POST only after the user retries", async () => {
  const requests: string[] = [];
  let repaired = false;
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
    const method = init.method ?? "GET";
    requests.push(method + " " + url);
    const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
    if (url === "/api/sessions") return json([{ id: "session", user_id: "user", label: "默认" }]);
    if (url === "/api/files/file") return json({ book: { id: "book", title: "测试书籍" }, chapters: [{ idx: 0, title: "第一章", linear: true }], toc: [] });
    if (url === "/api/files/file/sessions") return json({ sessions: [] });
    if (url === "/api/files/file/chapters/0/retry" && method === "POST") {
      repaired = true;
      return json({ idx: 0, title: "第一章", content: "<p>补下载的正文</p>" });
    }
    if (url === "/api/files/file/chapters/0") return json({ error: "stored chapter" }, 404);
    if (method === "PUT") return json({});
    throw new Error("Unexpected API call " + method + " " + url);
  }));
  render(<MemoryRouter initialEntries={["/read/file"]}><ReadingSessionsProvider><Routes>
    <Route path="/read/:id" element={<ReaderPage />} />
  </Routes></ReadingSessionsProvider></MemoryRouter>);
  await screen.findByRole("alert");
  expect(repaired).toBe(false);
  expect(requests.filter(r => r.includes("/chapters/"))).toEqual(["GET /api/files/file/chapters/0"]);
  fireEvent.click(screen.getByRole("button", { name: "重新加载章节" }));
  await screen.findByText("补下载的正文");
  expect(repaired).toBe(true);
  expect(requests.filter(r => r.includes("/chapters/"))).toEqual([
    "GET /api/files/file/chapters/0", "POST /api/files/file/chapters/0/retry",
  ]);
});
