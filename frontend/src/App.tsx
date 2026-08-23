import { useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Link, NavLink, Navigate, Route, Routes } from "react-router-dom";
import { useAuth } from "./auth";
import {
  IconLogout,
  IconMoon,
  IconSun,
  LogoMark,
} from "./components/icons";
import { ToastProvider } from "./components/toast";
import { BookDetailPage } from "./pages/BookDetailPage";
import { LibraryPage } from "./pages/LibraryPage";
import { LoginPage, RegisterPage } from "./pages/AuthPages";
import { PluginsPage } from "./pages/PluginsPage";
import { SeriesPage } from "./pages/SeriesPage";
import { ReaderPage } from "./pages/ReaderPage";
import { SharePage } from "./pages/SharePage";

function RequireAuth({ children }: { children: ReactNode }) {
  const { health, user } = useAuth();
  const { t } = useTranslation();
  if (!health) return <div className="page-loading">{t("common.loading")}</div>;
  if (health.auth_enabled && !user) return <Navigate to="/login" replace />;
  return <>{children}</>;
}

/** App-wide color theme (dark default), persisted to localStorage. */
type AppTheme = "dark" | "light";

const THEME_KEY = "bookshelf_theme";

function loadTheme(): AppTheme {
  const raw = localStorage.getItem(THEME_KEY);
  return raw === "light" ? "light" : "dark";
}

export default function App() {
  const { user, health, logout } = useAuth();
  const { t } = useTranslation();
  const [theme, setTheme] = useState<AppTheme>(loadTheme);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem(THEME_KEY, theme);
  }, [theme]);

  // Close the user dropdown on outside clicks.
  useEffect(() => {
    if (!menuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) setMenuOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [menuOpen]);

  return (
    <ToastProvider>
      <div className="app">
        <header className="nav">
          <div className="nav-left">
            <Link to="/" className="brand">
              <LogoMark className="brand-mark" />
              Bookshelf
            </Link>
            {user && (
              <nav className="nav-items">
                <NavLink to="/" end className="nav-item">
                  {t("nav.shelf")}
                </NavLink>
                {user.role === "admin" && (
                  <NavLink to="/plugins" className="nav-item">
                    {t("nav.plugins")}
                  </NavLink>
                )}
              </nav>
            )}
          </div>
          <div className="nav-right">
            {health && !health.auth_enabled && (
              <span className="badge" title="BOOKSHELF_AUTH_ENABLED=false">
                {t("nav.localMode")}
              </span>
            )}
            <button
              className="icon-btn"
              onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
              aria-label={t("nav.toggleTheme")}
              title={t("nav.toggleTheme")}
            >
              {theme === "dark" ? <IconSun size={16} /> : <IconMoon size={16} />}
            </button>
            {user ? (
              <div className="user-menu" ref={menuRef}>
                <button
                  className="user-menu-btn"
                  onClick={() => setMenuOpen((v) => !v)}
                  aria-expanded={menuOpen}
                >
                  {user.username}
                </button>
                {menuOpen && (
                  <div className="user-menu-panel">
                    <button
                      onClick={() => {
                        setMenuOpen(false);
                        logout();
                      }}
                    >
                      <IconLogout size={14} />
                      {t("nav.logout")}
                    </button>
                  </div>
                )}
              </div>
            ) : (
              <Link className="link-btn" to="/login">
                {t("nav.login")}
              </Link>
            )}
          </div>
        </header>
        <main className="content">
          <Routes>
            <Route path="/login" element={<LoginPage />} />
            <Route path="/register" element={<RegisterPage />} />
            <Route
              path="/"
              element={
                <RequireAuth>
                  <LibraryPage />
                </RequireAuth>
              }
            />
            <Route
              path="/book/:id"
              element={
                <RequireAuth>
                  <BookDetailPage />
                </RequireAuth>
              }
            />
            <Route
              path="/series/:id"
              element={
                <RequireAuth>
                  <SeriesPage />
                </RequireAuth>
              }
            />
            <Route
              path="/read/:id"
              element={
                <RequireAuth>
                  <ReaderPage />
                </RequireAuth>
              }
            />
            <Route
              path="/plugins"
              element={
                <RequireAuth>
                  <PluginsPage />
                </RequireAuth>
              }
            />
            <Route path="/share/:token" element={<SharePage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </main>
      </div>
    </ToastProvider>
  );
}
