import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { api } from "./api";
import { useAuth } from "./auth";
import { useToast } from "./components/toast";
import type { ReadingSession, SessionList } from "./types";

export const sessionStorageKey = (userId: string) => "mandara_reading_session:" + userId;

export function resolveSession(sessions: ReadingSession[], preferred: string | null) {
  return sessions.find(s => s.id === preferred) ?? sessions[0] ?? null;
}

interface SessionState {
  sessions: ReadingSession[];
  active: ReadingSession | null;
  error: string | null;
  resolved: boolean;
  select: (id: string) => void;
  create: (label: string) => Promise<void>;
  refresh: () => Promise<void>;
}
const Context = createContext<SessionState>({ sessions: [], active: null, error: null, resolved: false, select: () => {}, create: async () => {}, refresh: async () => {} });

export function ReadingSessionsProvider({ children }: { children: ReactNode }) {
  const { user } = useAuth();
  const [state, setState] = useState<{ userId: string; sessions: ReadingSession[]; selected: string | null }>({ userId: "", sessions: [], selected: null });
  const [error, setError] = useState<string | null>(null);
  const userId = user?.id ?? "";
  const currentUserId = useRef(userId);
  currentUserId.current = userId;
  const requestSeq = useRef(0);
  const refresh = useCallback(async () => {
    const seq = ++requestSeq.current;
    setError(null);
    if (!userId) return;
    try {
      const sessions = await api<ReadingSession[]>("/sessions");
      if (seq !== requestSeq.current || currentUserId.current !== userId) return;
      let preferred: string | null = null;
      try { preferred = localStorage.getItem(sessionStorageKey(userId)); } catch { /* Storage may be disabled. */ }
      const selected = resolveSession(sessions, preferred)?.id ?? null;
      setState({ userId, sessions, selected });
      setError(null);
      try {
        if (selected) localStorage.setItem(sessionStorageKey(userId), selected);
        else localStorage.removeItem(sessionStorageKey(userId));
      } catch { /* Best effort. */ }
    } catch (err) {
      if (seq === requestSeq.current && currentUserId.current === userId) setError(err instanceof Error ? err.message : String(err));
    }
  }, [userId]);
  useEffect(() => { void refresh(); }, [refresh]);
  const sessions = state.userId === userId ? state.sessions : [];
  const active = resolveSession(sessions, state.selected);
  const select = useCallback((id: string) => {
    setState(prev => {
      if (prev.userId !== userId || !prev.sessions.some(s => s.id === id)) return prev;
      try { localStorage.setItem(sessionStorageKey(userId), id); } catch { /* Best effort. */ }
      return { ...prev, selected: id };
    });
  }, [userId]);
  const create = useCallback(async (label: string) => {
    const created = await api<ReadingSession>("/sessions", { method: "POST", body: JSON.stringify({ label }) });
    if (currentUserId.current !== userId) return;
    ++requestSeq.current;
    setError(null);
    setState(prev => ({ userId, sessions: [...(prev.userId === userId ? prev.sessions : []).filter(s => s.id !== created.id), created], selected: created.id }));
    try { localStorage.setItem(sessionStorageKey(userId), created.id); } catch { /* Best effort. */ }
  }, [userId]);
  return <Context.Provider value={{ sessions, active, error, resolved: state.userId === userId, select, create, refresh }}>{children}</Context.Provider>;
}

export const useReadingSessions = () => useContext(Context);

/** Only the selected context contributes to shelf/series progress. */
export function useSelectedProgress(): SessionList {
  const { active } = useReadingSessions();
  const id = active?.id;
  const [state, setState] = useState<{ id: string; progress: SessionList } | null>(null);
  const toast = useToast();
  const { t } = useTranslation();
  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    void api<SessionList>("/sessions/" + id + "/progress").then(progress => {
      if (!cancelled) setState({ id, progress });
    }).catch(err => {
      if (!cancelled) toast.push("error", err instanceof Error ? err.message : t("common.failed"));
    });
    return () => { cancelled = true; };
  }, [id, toast, t]);
  return state && state.id === id ? state.progress : [];
}

export function SessionControl({ disabled = false }: { disabled?: boolean }) {
  const { sessions, active, error, resolved, select, create, refresh } = useReadingSessions();
  const { t } = useTranslation();
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  async function add() {
    const label = window.prompt(t("reader.newSessionPrompt"));
    if (!label?.trim()) return;
    setBusy(true);
    try { await create(label); }
    catch (err) { toast.push("error", err instanceof Error ? err.message : t("reader.createFailed")); }
    finally { setBusy(false); }
  }
  if (error) return <button className="btn sm" onClick={() => void refresh()} title={error}>{t("reader.retrySessions")}</button>;
  if (!resolved) return <span>{t("common.loading")}</span>;
  return <div className="reader-session-bar">
    {active && <select aria-label={t("reader.sessionLabel")} value={active.id} disabled={disabled || busy} onChange={e => select(e.target.value)}>
      {sessions.map(s => <option key={s.id} value={s.id}>{s.label}</option>)}
    </select>}
    <button className="btn sm" disabled={disabled || busy} onClick={() => void add()}>{t("reader.newSession")}</button>
  </div>;
}
