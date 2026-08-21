import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { api } from "../api";
import { Reader } from "../components/Reader";
import type { Chapter, ShareBookResponse, ShareView } from "../types";

/// Public share view: no login required, no progress saving.
export function SharePage() {
  const { token = "" } = useParams();
  const [view, setView] = useState<ShareView | null>(null);
  const [book, setBook] = useState<ShareBookResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        const [v, b] = await Promise.all([
          api<ShareView>(`/shares/${token}`),
          api<ShareBookResponse>(`/shares/${token}/book`),
        ]);
        setView(v);
        setBook(b);
      } catch (err) {
        setError(err instanceof Error ? err.message : "链接无效或已过期");
      }
    })();
  }, [token]);

  if (error) {
    return (
      <div className="card">
        <h1>链接不可用</h1>
        <p>{error}</p>
      </div>
    );
  }
  if (!view || !book) return <div className="page-loading">加载中…</div>;

  return (
    <div>
      {view.session && (
        <div className="card share-snapshot">
          <h2>
            📍 {view.session.owner_username} 的进度 · 会话「{view.session.label}」
          </h2>
          <p>
            <strong>{book.book.title}</strong> · 已读到{" "}
            <span className="strong">{view.session.percent}%</span>
            <span className="hint">（更新于 {new Date(view.session.updated_at).toLocaleString("zh-CN")}）</span>
          </p>
        </div>
      )}
      <Reader
        title={book.book.title}
        chapters={book.chapters}
        loadChapter={async (idx) => {
          const c = await api<Chapter>(`/shares/${token}/chapters/${idx}`);
          return c;
        }}
        readOnly
      />
      <p className="hint center">
        分享自 Bookshelf · <Link to="/">自己部署一个 →</Link>
      </p>
    </div>
  );
}