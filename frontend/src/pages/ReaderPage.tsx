import { useCallback, useEffect, useMemo, useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { IconPlus } from "../components/icons";
import { Reader } from "../components/Reader";
import { useToast } from "../components/toast";
import type { Chapter, FileDetail, Position, ReadingSession, SessionsResponse } from "../types";

/// Reader page for one file (one format/edition of a book).
export function ReaderPage() {
  const { id = "" } = useParams();
  const { t } = useTranslation();
  const toast = useToast();
  const [params, setParams] = useSearchParams();
  const [detail, setDetail] = useState<FileDetail | null>(null);
  const [sessions, setSessions] = useState<ReadingSession[]>([]);
  const [active, setActive] = useState<ReadingSession | null>(null);
  const [error, setError] = useState<string | null>(null);

  const sessionId = params.get("session");
  const chapterIdx = params.get("chapter");

  const load = useCallback(async () => {
    try {
      const [d, s] = await Promise.all([
        api<FileDetail>(`/files/${id}`),
        api<SessionsResponse>(`/files/${id}/sessions`),
      ]);
      setDetail(d);
      setSessions(s.sessions);
      const wanted = s.sessions.find((x) => x.id === sessionId) ?? s.sessions[0] ?? null;
      setActive(wanted);
    } catch (err) {
      setError(err instanceof Error ? err.message : t("common.failed"));
    }
  }, [id, sessionId]);

  useEffect(() => {
    void load();
  }, [load]);

  const initialPosition = useMemo<Position>(() => {
    if (chapterIdx !== null) {
      return { chapter_idx: Number(chapterIdx) || 0, in_chapter: { fraction: 0 } };
    }
    return active?.position ?? { chapter_idx: 0, in_chapter: { fraction: 0 } };
  }, [active, chapterIdx]);

  async function createSession() {
    const label = window.prompt(t("reader.newSessionPrompt"), t("reader.defaultSessionLabel"));
    if (!label) return;
    try {
      const created = await api<ReadingSession>(`/files/${id}/sessions`, {
        method: "POST",
        body: JSON.stringify({ label }),
      });
      setSessions((prev) => [...prev, created]);
      setParams({ session: created.id });
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("reader.createFailed"));
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
        body: JSON.stringify({ in_chapter: { fraction } }),
      });
    },
    [active],
  );

  if (error) return <div className="error">{error}</div>;
  if (!detail) return <div className="page-loading">{t("common.loading")}</div>;

  const { file, book, chapters } = detail;

  return (
    <div>
      <div className="reader-session-bar">
        {active ? (
          <>
            <select
              value={active.id}
              onChange={(e) => switchSession(e.target.value)}
              aria-label={t("reader.sessionLabel")}
            >
              {sessions.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.label}
                </option>
              ))}
            </select>
            <button className="btn link" onClick={() => void createSession()}>
              <IconPlus size={13} /> {t("reader.newSession")}
            </button>
          </>
        ) : (
          <button className="btn link" onClick={() => void createSession()}>
            <IconPlus size={13} /> {t("reader.createSession")}
          </button>
        )}
        <span className="hint">
          {book.title} · {file.label || file.format}
        </span>
      </div>
      <Reader
        title={book.title}
        chapters={chapters}
        toc={detail.toc}
        backHref={`/book/${book.id}`}
        loadChapter={async (idx) => {
          const c = await api<Chapter>(`/files/${id}/chapters/${idx}`);
          return c;
        }}
        initialPosition={initialPosition}
        onProgress={save}
      />
    </div>
  );
}