import { useCallback, useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { useParams, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { Reader } from "../components/Reader";
import { useToast } from "../components/toast";
import { SessionControl, useReadingSessions } from "../readingSessions";
import type { Chapter, FileDetail, Position, ReadingProgress, SessionsResponse } from "../types";

export function ReaderPage() {
  const { id = "" } = useParams();
  const { t } = useTranslation();
  const toast = useToast();
  const [params, setParams] = useSearchParams();
  const { sessions, active, select, resolved, error: sessionError } = useReadingSessions();
  const [loaded, setLoaded] = useState<{ key: string; detail: FileDetail; progress: ReadingProgress[]; position: Position } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [source, setSource] = useState("");
  const saveQueue = useRef<Promise<unknown>>(Promise.resolve());
  const lastPosition = useRef<Position | null>(null);
  const selectedId = active?.id;
  const key = id + ":" + selectedId;
  const [requestKey, setRequestKey] = useState(key);
  if (requestKey !== key) {
    setRequestKey(key);
    setLoaded(null);
  }
  const requestedSession = params.get("session");
  const chapterIdx = params.get("chapter");
  const previousContext = useRef<string | null>(null);
  useEffect(() => {
    if (!selectedId || requestedSession) return;
    if (previousContext.current && previousContext.current !== selectedId && params.has("chapter")) {
      const next = new URLSearchParams(params);
      next.delete("chapter");
      setParams(next, { replace: true });
    }
    previousContext.current = selectedId;
  }, [selectedId, requestedSession, params, setParams]);

  // Explicit progress links adopt the context once; subsequent navigation
  // uses the persisted global selection, not a stale query parameter.
  useEffect(() => {
    if (!requestedSession || !resolved) return;
    if (sessions.some(s => s.id === requestedSession)) select(requestedSession);
    const next = new URLSearchParams(params);
    next.delete("session");
    setParams(next, { replace: true });
  }, [requestedSession, resolved, sessions, select, params, setParams]);

  useEffect(() => {
    if (!selectedId || requestedSession) return;
    let cancelled = false;
    setError(null);
    setSource("");
    async function load() {
      try {
        // The previous reader flushes on unmount. Wait for its writes before
        // reading a context again (including rapid A -> B -> A switches).
        await saveQueue.current;
        const [detail, response] = await Promise.all([
          api<FileDetail>("/files/" + id),
          api<SessionsResponse>("/files/" + id + "/sessions"),
        ]);
        if (cancelled) return;
        const position = response.sessions.find(p => p.session_id === selectedId)?.position ?? { chapter_idx: 0, in_chapter: { fraction: 0 } };
        lastPosition.current = position;
        setLoaded({ key, detail, progress: response.sessions, position });
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : t("common.failed"));
      }
    }
    void load();
    return () => { cancelled = true; };
  }, [id, selectedId, key, requestedSession, t]);

  const save = useCallback((fraction: number, chapterIdx: number) => {
    if (!selectedId) return;
    lastPosition.current = { chapter_idx: chapterIdx, in_chapter: { fraction } };
    saveQueue.current = saveQueue.current.catch(() => undefined).then(() => api<ReadingProgress>(
      "/sessions/" + selectedId + "/files/" + id + "/progress",
      { method: "PUT", body: JSON.stringify({ chapter_idx: chapterIdx, in_chapter: { fraction } }) },
    )).catch(err => { toast.push("error", err instanceof Error ? err.message : t("reader.saveFailed")); });
  }, [id, selectedId, toast, t]);

  async function refreshSources() {
    try {
      const response = await api<SessionsResponse>("/files/" + id + "/sessions");
      setLoaded(prev => prev?.key === key ? { ...prev, progress: response.sessions } : prev);
      setSource("");
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("reader.syncFailed"));
    }
  }

  async function sync() {
    if (!selectedId || !source || !loaded || syncing) return;
    if (!window.confirm(t("reader.confirmSync"))) return;
    // Unmount and flush the old reader before overwriting progress; otherwise
    // a delayed scroll write can undo the explicit sync.
    flushSync(() => setSyncing(true));
    try {
      await saveQueue.current;
      const retained = lastPosition.current;
      if (retained) setLoaded(prev => prev?.key === key ? { ...prev, position: retained } : prev);
      const progress = await api<ReadingProgress>("/sessions/" + selectedId + "/files/" + id + "/sync", {
        method: "POST", body: JSON.stringify({ source_session_id: source }),
      });
      setLoaded(prev => prev?.key === key ? { ...prev, position: progress.position, progress: [...prev.progress.filter(p => p.session_id !== selectedId), progress] } : prev);
      const next = new URLSearchParams(params);
      next.delete("chapter");
      setParams(next, { replace: true });
      toast.push("success", t("reader.syncDone"));
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("reader.syncFailed"));
    } finally { setSyncing(false); }
  }

  if (sessionError || error) return <div className="error">{sessionError || error}<SessionControl /></div>;
  if (resolved && !selectedId) return <div className="page-loading"><SessionControl /></div>;
  if (!loaded || loaded.key !== key || requestedSession || syncing) return <div className="page-loading">{t("common.loading")}</div>;
  const { detail } = loaded;
  const initialPosition: Position = chapterIdx === null ? loaded.position : { chapter_idx: Number(chapterIdx) || 0, in_chapter: { fraction: 0 } };
  const sources = loaded.progress.filter(p => p.session_id !== selectedId);
  return <div className="reader-page">
    <Reader key={key + ":" + (chapterIdx ?? "resume")}
      sessionControls={<div className="reader-session-controls">
        <SessionControl />
        <select aria-label={t("reader.syncSource")} value={source} onChange={e => setSource(e.target.value)} disabled={!sources.length}>
          <option value="">{t("reader.syncSource")}</option>
          {sources.map(p => <option key={p.session_id} value={p.session_id}>
            {p.label}
            {t("reader.syncPosition", { chapter: p.position.chapter_idx + 1 })}
          </option>)}
        </select>
        <button className="btn sm" onClick={() => void refreshSources()}>{t("reader.refreshSources")}</button>
        <button className="btn sm" disabled={!source} onClick={() => void sync()}>{t("reader.syncProgress")}</button>
      </div>}
      title={detail.book.title} chapters={detail.chapters} toc={detail.toc}
      backHref={"/book/" + detail.book.id}
      loadChapter={idx => api<Chapter>("/files/" + id + "/chapters/" + idx)}
      initialPosition={initialPosition} onProgress={save}
    />
  </div>;
}
