import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { MemoryRouter } from "react-router-dom";
import type { ReactNode } from "react";
import App from "../src/App";

const state = vi.hoisted(() => ({
  resolved: true,
  user: { id: "user", username: "测试用户", role: "admin" } as { id: string; username: string; role: string } | null,
  loads: [] as string[],
}));
vi.mock("../src/auth", () => ({ useAuth: () => ({
  resolved: state.resolved, user: state.user, health: {}, logout: vi.fn(),
}) }));
vi.mock("../src/readingSessions", () => ({
  ReadingSessionsProvider: ({ children }: { children: ReactNode }) => <>{children}</>,
  SessionControl: () => null,
}));
vi.mock("../src/pages/ReaderPage", () => {
  state.loads.push("reader");
  return { ReaderPage: () => <p>阅读器页面</p> };
});
vi.mock("../src/pages/LibraryPage", () => {
  state.loads.push("library");
  return { LibraryPage: () => <p>书架页面</p> };
});
vi.mock("../src/pages/AuthPages", () => {
  state.loads.push("auth");
  return { LoginPage: () => <p>登录页面</p>, RegisterPage: () => <p>注册页面</p> };
});
vi.mock("../src/pages/SharePage", () => {
  state.loads.push("share");
  return { SharePage: () => <p>分享页面</p> };
});
vi.mock("../src/pages/BookDetailPage", () => {
  state.loads.push("book");
  return { BookDetailPage: () => <p>书籍详情页面</p> };
});
vi.mock("../src/pages/SeriesPage", () => {
  state.loads.push("series");
  return { SeriesPage: () => <p>系列页面</p> };
});
vi.mock("../src/pages/PluginsPage", () => {
  state.loads.push("plugins");
  return { PluginsPage: () => <p>插件页面</p> };
});

beforeEach(() => {
  localStorage.clear();
  state.resolved = true;
  state.user = { id: "user", username: "测试用户", role: "admin" };
  state.loads = [];
});

it("waits for authentication and loads only the visited page modules", async () => {
  state.resolved = false;
  const view = render(<MemoryRouter initialEntries={["/read/book"]}><App /></MemoryRouter>);
  expect(screen.getByText("加载中…")).not.toBeNull();
  expect(state.loads).toEqual([]);
  state.resolved = true;
  view.rerender(<MemoryRouter initialEntries={["/read/book"]}><App /></MemoryRouter>);
  await screen.findByText("阅读器页面");
  expect(state.loads).toEqual(["reader"]);
  expect(view.container.querySelector(".app-reader")).not.toBeNull();
  fireEvent.click(screen.getByRole("link", { name: "书架" }));
  await screen.findByText("书架页面");
  expect(state.loads).toEqual(["reader", "library"]);
  expect(view.container.querySelector(".app-reader")).toBeNull();
});

it("redirects anonymous protected routes to the lazy login page", async () => {
  state.user = null;
  render(<MemoryRouter initialEntries={["/read/book"]}><App /></MemoryRouter>);
  await screen.findByText("登录页面");
  expect(state.loads).toEqual(["auth"]);
  expect(screen.queryByText("阅读器页面")).toBeNull();
});

it("loads the public shared reader without waiting for authentication", async () => {
  state.user = null;
  state.resolved = false;
  const view = render(<MemoryRouter initialEntries={["/share/token/read"]}><App /></MemoryRouter>);
  await screen.findByText("分享页面");
  expect(state.loads).toEqual(["share"]);
  expect(view.container.querySelector(".app-reader")).not.toBeNull();
});

it.each([
  ["/register", "注册页面"], ["/book/book", "书籍详情页面"],
  ["/series/series", "系列页面"], ["/plugins", "插件页面"],
])("resolves the lazy route %s", async (url, content) => {
  render(<MemoryRouter initialEntries={[url]}><App /></MemoryRouter>);
  await screen.findByText(content);
});
