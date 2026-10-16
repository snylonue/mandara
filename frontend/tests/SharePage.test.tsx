import { render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { api } from "../src/api";
import { SharePage } from "../src/pages/SharePage";
import type { ShareBookResponse, ShareView } from "../src/types";

vi.mock("../src/api", () => ({ api: vi.fn() }));
const book: ShareBookResponse = {
  book: { id: "book", title: "测试书籍", authors: ["测试作者"], description: "书籍简介" },
  file: { id: "file", format: "epub", label: "精校版", chapter_count: 2 },
  chapters: [{ idx: 0, title: "序章", linear: true }, { idx: 1, title: "第一章", linear: true }],
  toc: [],
};
const view: ShareView = {
  kind: "book", book: book.book, file: book.file, session: null,
  created_at: "2026-01-01T00:00:00Z", expires_at: null,
};
beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  HTMLElement.prototype.scrollTo = vi.fn();
  vi.mocked(api).mockImplementation(async (path) => {
    if (path === "/shares/test") return view;
    if (path === "/shares/test/book") return book;
    if (path === "/shares/test/chapters/0") return { idx: 0, title: "序章", content: "<p>序章正文</p>" };
    if (path === "/shares/test/chapters/1") return { idx: 1, title: "第一章", content: "<p>第一章正文</p>" };
    throw new Error("Unexpected API call: " + path);
  });
});
function sharePage() {
  return render(<MemoryRouter initialEntries={["/share/test"]}><Routes>
    <Route path="/share/:token" element={<SharePage />} />
  </Routes></MemoryRouter>);
}

it("does not show the promotional footer on public shares", async () => {
  sharePage();
  await screen.findByText("序章正文");
  expect(screen.queryByText(/分享自 Mandara/)).toBeNull();
  expect(screen.queryByRole("link", { name: "自己部署一个 Mandara" })).toBeNull();
});
