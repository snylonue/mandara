import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Link, Navigate, Route, Routes } from "react-router-dom";
import { useAuth } from "./auth";
import { BookDetailPage } from "./pages/BookDetailPage";
import { LibraryPage } from "./pages/LibraryPage";
import { LoginPage, RegisterPage } from "./pages/AuthPages";
import { PluginsPage } from "./pages/PluginsPage";
import { ReaderPage } from "./pages/ReaderPage";
import { SharePage } from "./pages/SharePage";

function RequireAuth({ children }: { children: ReactNode }) {
  const { health, user } = useAuth();
  const { t } = useTranslation();
  if (!health) return <div className="page-loading">{t("common.loading")}</div>;
  if (health.auth_enabled && !user) return <Navigate to="/login" replace />;
  return <>{children}</>;
}

export default function App() {
  const { user, health, logout } = useAuth();
  const { t } = useTranslation();

  return (
    <div className="app">
      <header className="nav">
        <Link to="/" className="brand">
          {t("nav.brand")}
        </Link>
        <div className="nav-right">
          {health && !health.auth_enabled && (
            <span className="badge" title="BOOKSHELF_AUTH_ENABLED=false">
              {t("nav.localMode")}
            </span>
          )}
          {user ? (
            <>
              {user.role === "admin" && (
                <Link className="link-btn" to="/plugins">
                  {t("nav.plugins")}
                </Link>
              )}
              <span className="username">{user.username}</span>
              <button className="link-btn" onClick={logout}>
                {t("nav.logout")}
              </button>
            </>
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
  );
}