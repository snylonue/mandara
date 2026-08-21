import { useCallback, useEffect, useState, type FormEvent } from "react";
import { Link, useParams } from "react-router-dom";
import { api } from "../api";
import type { BookDetail, ReadingSession, SessionsResponse, ShareInfo } from "../types";

const fmtTime = (s: string) => new Date(s).toLocaleString("zh-CN", { hour12: false });

export function BookDetailPage() {
  const { id = "" } = useParams();
  const [detail, setDetail] = useState<BookDetail | null>(null);
  const [sessions, setSessions] = useState<ReadingSession[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [label, setLabel] = useState("");
  const [share, setShare] = useState<ShareInfo | null>(null);

  const load = useCallback(async () => {
    try {
      const d = await api<BookDetail>(`/books/${id}`);
      setDetail(d);
      const s = await api<SessionsResponse>(`/books/${id}/sessions`);
      setSessions(s.sessions);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "加载失败");
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  async function createSession(e: FormEvent) {
    e.preventDefault();
    try {
      await api<ReadingSession>(`/books/${id}/sessions`, {
        method: "POST",
        body: JSON.stringify({ label }),
      });
      setLabel("");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "创建失败");
    }
  }

  async function deleteSession(sessionId: string) {
    await api(`/sessions/${sessionId}`, { method: "DELETE" });
    await load();
  }

  async function makeShare(kind: "book" | "session", sessionId?: string) {
    try {
      const info = await api<ShareInfo>(`/books/${id}/shares`, {
        method: "POST",
        body: JSON.stringify({ kind, session_id: sessionId }),
      });
      setShare(info);
    } catch (err) {
      setError(err instanceof Error ? err.message : "创建分享失败");
    }
  }

  if (error) return <div className="error">{error}</div>;
  if (!detail) return <div className="page-loading">加载中…</div>;
  const { book, chapters } = detail;

  return (
    <div className="book-detail">
      <Link to="/" className="link-btn">
        ← 书架
      </Link>
      <h1>{book.title}</h1>
      <p className="hint">
        {book.authors.join(" / ") || "佚名"} · {book.chapter_count} 章 · 来源:{book.source}
      </p>
      {book.description && <p className="description">{book.description}</p>}

      <section className="card">
        <h2>阅读会话</h2>
        <p className="hint">
          每个会话独立记录进度（如手机、平板、电脑各一个），可分别续读与分享。
        </p>
        <table className="sessions">
          <thead>
            <tr>
              <th>会话</th>
              <th>进度</th>
              <th>更新时间</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {sessions.map((s) => {
              const chapter = chapters.find((c) => c.idx === s.position.chapter_idx);
              return (
                <tr key={s.id}>
                  <td>
                    <Link to={`/read/${id}?session=${s.id}`} className="strong">
                      {s.label}
                    </Link>
                  </td>
                  <td>
                    {chapter?.title ?? s.position.chapter_idx + 1} ·{" "}
                    {Math.round(s.position.fraction * 100)}%
                  </td>
                  <td className="hint">{fmtTime(s.updated_at)}</td>
                  <td className="row-actions">
                    <button className="link-btn" onClick={() => makeShare("session", s.id)}>
                      分享进度
                    </button>
                    <button className="link-btn danger" onClick={() => void deleteSession(s.id)}>
                      删除
                    </button>
                  </td>
                </tr>
              );
            })}
            {!sessions.length && (
              <tr>
                <td colSpan={4} className="hint">
                  还没有会话
                </td>
              </tr>
            )}
          </tbody>
        </table>
        <form className="inline-form" onSubmit={createSession}>
          <input
            placeholder="新会话名称，如：手机"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
          />
          <button className="primary" type="submit">
            新建会话
          </button>
        </form>
      </section>

      <section className="card">
        <h2>分享</h2>
        <div className="row">
          <button className="primary" onClick={() => void makeShare("book")}>
            生成本书分享链接（可匿名阅读）
          </button>
          <span className="hint">插件公共书可直接被任意用户生成分享。</span>
        </div>
        {share && (
          <div className="share-box">
            <span className="tag">{share.kind === "book" ? "书" : "进度"}分享</span>
            <code>{share.url}</code>
            <button
              className="link-btn"
              onClick={() => {
                void navigator.clipboard?.writeText(`${location.origin}${share.url}`);
              }}
            >
              复制
            </button>
          </div>
        )}
      </section>

      <section className="card">
        <h2>目录（{chapters.length} 章）</h2>
        <ol className="toc">
          {chapters.map((c) => (
            <li key={c.idx}>
              <Link to={`/read/${id}?chapter=${c.idx}`}>{c.title}</Link>
            </li>
          ))}
        </ol>
      </section>
    </div>
  );
}