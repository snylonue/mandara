import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { api } from "../api";
import { Reader } from "../components/Reader";
import type { BookDetail, Chapter, Position, ReadingSession, SessionsResponse } from "../types";

export function ReaderPage() {
  const { id = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const [detail, setDetail] = useState<BookDetail | null>(null);
  const [sessions, setSessions] = useState<ReadingSession[]>([]);
  const [active, setActive] = useState<ReadingSession | null>(null);
  const [error, setError] = useState<string | null>(null);

  const sessionId = params.get("session");
  const chapterIdx = params.get("chapter");

  const load = useCallback(async () => {
    try {
      const [d, s] = await Promise.all([
        api<BookDetail>(`/books/${id}`),
        api<SessionsResponse>(`/books/${id}/sessions`),
      ]);
      setDetail(d);
      setSessions(s.sessions);
      // pick the requested session, or the most recent one
      const wanted = s.sessions.find((x) => x.id === sessionId) ?? s.sessions[0] ?? null;
      setActive(wanted);
    } catch (err) {
      setError(err instanceof Error ? err.message : "加载失败");
    }
  }, [id, sessionId]);

  useEffect(() => {
    void load();
  }, [load]);

  const initialPosition = useMemo<Position>(() => {
    if (chapterIdx !== null) {
      return { chapter_idx: Number(chapterIdx) || 0, offset: 0, fraction: 0 };
    }
    return (
      active?.position ?? { chapter_idx: 0, offset: 0, fraction: 0 }
    );
  }, [active, chapterIdx]);

  async function createSession() {
    const label = window.prompt("新会话名称（如：平板）", "平板");
    if (!label) return;
    try {
      const created = await api<ReadingSession>(`/books/${id}/sessions`, {
        method: "POST",
        body: JSON.stringify({ label }),
      });
      setSessions((prev) => [...prev, created]);
      setParams({ session: created.id });
    } catch (err) {
      setError(err instanceof Error ? err.message : "创建失败");
    }
  }

  function switchSession(nextId: string) {
    setParams({ session: nextId });
  }

  const save = useCallback(
    (fraction: number) => {
      if (!active || fraction < 0.001) return;
      void api<ReadingSession>(`/sessions/${active.id}`, {
        method: "PUT",
        body: JSON.stringify({ fraction }),
      });
    },
    [active],
  );

  if (error) return <div className="error">{error}</div>;
  if (!detail) return <div className="page-loading">加载中…</div>;

  return (
    <div>
      {active ? (
        <div className="reader-session-bar">
          <select
            value={active.id}
            onChange={(e) => switchSession(e.target.value)}
            aria-label="会话"
          >
            {sessions.map((s) => (
              <option key={s.id} value={s.id}>
                {s.label}
              </option>
            ))}
          </select>
          <button className="link-btn" onClick={() => void createSession()}>
            ＋ 新会话
          </button>
        </div>
      ) : (
        <div className="reader-session-bar">
          <button className="link-btn" onClick={() => void createSession()}>
            ＋ 创建会话开始阅读
          </button>
        </div>
      )}
      <Reader
        title={detail.book.title}
        chapters={detail.chapters}
        loadChapter={async (idx) => {
          const c = await api<Chapter>(`/books/${id}/chapters/${idx}`);
          return c;
        }}
        initialPosition={initialPosition}
        onProgress={save}
      />
      <p className="hint center">
        <Link to={`/book/${id}`}>返回书籍详情</Link>
      </p>
    </div>
  );
}