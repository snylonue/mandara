import { lazy, Suspense, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Link, NavLink, Navigate, Route, Routes, useMatch } from "react-router-dom";
import { useAuth } from "./auth";
import { ReadingSessionsProvider, SessionControl } from "./readingSessions";
import {
  IconLogout,
  IconMoon,
  IconSun,
  LogoMark,
} from "./components/icons";
import { ToastProvider } from "./components/toast";

const BookDetailPage = lazy(() => import("./pages/BookDetailPage").then(module => ({ default: module.BookDetailPage })));
const LibraryPage = lazy(() => import("./pages/LibraryPage").then(module => ({ default: module.LibraryPage })));
const LoginPage = lazy(() => import("./pages/AuthPages").then(module => ({ default: module.LoginPage })));
const RegisterPage = lazy(() => import("./pages/AuthPages").then(module => ({ default: module.RegisterPage })));
const PluginsPage = lazy(() => import("./pages/PluginsPage").then(module => ({ default: module.PluginsPage })));
const SeriesPage = lazy(() => import("./pages/SeriesPage").then(module => ({ default: module.SeriesPage })));
const ReaderPage = lazy(() => import("./pages/ReaderPage").then(module => ({ default: module.ReaderPage })));
const SharePage = lazy(() => import("./pages/SharePage").then(module => ({ default: module.SharePage })));

function RequireAuth({ children }: { children: ReactNode }) {
  const { health, user, resolved } = useAuth();
  const { t } = useTranslation();
  // Wait for /auth/me before deciding: redirecting on the first render
  // bounced every refresh / bookmark of a protected page to /login.
  if (!resolved) return <div className="page-loading">{t("common.loading")}</div>;
  if (!health) return <div className="page-loading">{t("common.loading")}</div>;
  if (!user) return <Navigate to="/login" replace />;
  return <>{children}</>;
}

/** App-wide color theme (light default), persisted to localStorage. */
type AppTheme = "dark" | "light";

const THEME_KEY = "mandara_theme";

function loadTheme(): AppTheme {
  const raw = localStorage.getItem(THEME_KEY);
  return raw === "dark" ? "dark" : "light";
}

export default function App() {
  const { user, logout } = useAuth();
  const reading = Boolean(useMatch("/read/:id"));
  const sharing = Boolean(useMatch("/share/:token/*"));
  const sharedReading = Boolean(useMatch("/share/:token/read"));
  const { t } = useTranslation();
  const [theme, setTheme] = useState<AppTheme>(loadTheme);
  const [menuOpen, setMenuOpen] = useState(false);
  const [scrolled, setScrolled] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem(THEME_KEY, theme);
  }, [theme]);

  // The sticky top bar casts a shadow only once content scrolls under it.
  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 4);
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

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
      <ReadingSessionsProvider>
        <div className={`app${reading || sharedReading ? " app-reader" : ""}`}>
          <header className={`nav${scrolled ? " scrolled" : ""}`}>
            <div className="nav-left">
              <Link to="/" className="brand">
                <LogoMark className="brand-mark" />
                Mandara
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
              {user && !reading && !sharing && <SessionControl />}
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
                    <div className="popover user-menu-panel">
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
                <Link className="btn link" to="/login">
                  {t("nav.login")}
                </Link>
              )}
            </div>
          </header>
          <main className="content">
            <Suspense fallback={<div className="page-loading">{t("common.loading")}</div>}>
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
                <Route path="/share/:token/read" element={<SharePage />} />
                <Route path="*" element={<Navigate to="/" replace />} />
              </Routes>
            </Suspense>
          </main>
        </div>
      </ReadingSessionsProvider>
    </ToastProvider>
  );
}
