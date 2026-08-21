import type { ReactNode } from "react";
import { Link, Navigate, Route, Routes } from "react-router-dom";
import { useAuth } from "./auth";
import { BookDetailPage } from "./pages/BookDetailPage";
import { LibraryPage } from "./pages/LibraryPage";
import { LoginPage, RegisterPage } from "./pages/AuthPages";
import { ReaderPage } from "./pages/ReaderPage";
import { SharePage } from "./pages/SharePage";

function RequireAuth({ children }: { children: ReactNode }) {
  const { health, user } = useAuth();
  if (!health) return <div className="page-loading">加载中…</div>;
  if (health.auth_enabled && !user) return <Navigate to="/login" replace />;
  return <>{children}</>;
}

export default function App() {
  const { user, health, logout } = useAuth();

  return (
    <div className="app">
      <header className="nav">
        <Link to="/" className="brand">
          📚 Bookshelf
        </Link>
        <div className="nav-right">
          {health && !health.auth_enabled && (
            <span className="badge" title="BOOKSHELF_AUTH_ENABLED=false">
              本地模式
            </span>
          )}
          {user ? (
            <>
              <span className="username">{user.username}</span>
              <button className="link-btn" onClick={logout}>
                退出
              </button>
            </>
          ) : (
            <Link className="link-btn" to="/login">
              登录
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
          <Route path="/share/:token" element={<SharePage />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}