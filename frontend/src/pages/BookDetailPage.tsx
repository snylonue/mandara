import { useCallback, useEffect, useState, type ChangeEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { ApiError, api } from "../api";
import { useAuth } from "../auth";
import { IconArrowLeft } from "../components/icons";
import { useToast } from "../components/toast";
import {
  SourceBrowserDialog,
  usePluginInstances,
  type SourcePick,
} from "../components/SourceSearch";
import type {
  BookDetail,
  BookMeta,
  FileMeta,
  PluginInstance,
  ReadingSession,
  SessionsResponse,
  ShareInfo,
  Visibility,
} from "../types";

const fmtTime = (s: string) => new Date(s).toLocaleString("zh-CN", { hour12: false });
const fmtPercent = (s: ReadingSession) => `${Math.round(s.position.fraction * 100)}%`;

/// One file row: actions per file (read, visibility toggle, share, delete)
/// plus its sessions. Manage actions (visibility/share/delete) only show
/// for the file's owner or an admin — other users' uploads are read-only.
function FileSection({
  file,
  canManage,
  onChanged,
  onShare,
}: {
  file: FileMeta;
  canManage: boolean;
  onChanged: () => Promise<void>;
  onShare: (fileId: string, kind: "book" | "session", sessionId?: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const instances = usePluginInstances();
  const [rebindOpen, setRebindOpen] = useState(false);
  const [rebindError, setRebindError] = useState<string | null>(null);
  const [rebinding, setRebinding] = useState(false);
  const [sessions, setSessions] = useState<ReadingSession[]>([]);

  async function rebind(pick: SourcePick) {
    setRebindError(null);
    setRebinding(true);
    try {
      const body: Record<string, string> = { content_source: pick.instance.id };
      // The file's external_id (its id in the *metadata* source) rarely
      // matches the content source's id; use the picked book's id unless
      // the user picked the default (self) of the current source.
      body.content_external_id = pick.item.id;
      await api<FileMeta>(`/files/${file.id}/content-source`, {
        method: "POST",
        body: JSON.stringify(body),
      });
      setRebindOpen(false);
      await onChanged();
    } catch (e) {
      setRebindError(e instanceof ApiError ? e.message : t("common.failed_verb"));
    } finally {
      setRebinding(false);
    }
  }

  const loadSessions = useCallback(async () => {
    try {
      const s = await api<SessionsResponse>(`/files/${file.id}/sessions`);
      setSessions(s.sessions);
    } catch {
      setSessions([]);
    }
  }, [file.id]);

  useEffect(() => {
    void loadSessions();
  }, [loadSessions]);

  async function toggleVisibility() {
    const next: Visibility = file.visibility === "public" ? "private" : "public";
    try {
      await api<FileMeta>(`/files/${file.id}`, {
        method: "PATCH",
        body: JSON.stringify({ visibility: next }),
      });
      await onChanged();
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("book.opFailed"));
    }
  }

  async function deleteFile() {
    if (!window.confirm(t("book.confirmDeleteFile", { label: file.label || file.format }))) return;
    try {
      await api(`/files/${file.id}`, { method: "DELETE" });
      await onChanged();
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("book.deleteFailed"));
    }
  }

  const readable = file.chapter_count > 0;

  return (
    <div className="file-card">
      <div className="file-head">
        <div>
          <Link to={`/read/${file.id}`} className="strong">
            {file.label || file.format}
          </Link>
          <span className="tag">{file.format}</span>
          {file.source !== "local" && <span className="tag">{t("book.pluginTag")}</span>}
          <span className={`tag ${file.visibility === "public" ? "tag-public" : ""}`}>
            {file.visibility === "public" ? t("book.publicTag") : t("book.privateTag")}
          </span>
        </div>
        <div className="row-actions">
          {readable && (
            <Link className="link-btn" to={`/read/${file.id}`}>
              {t("book.read")}
            </Link>
          )}
          {file.source === "local" && canManage && (
            <>
              <button className="link-btn" onClick={() => void toggleVisibility()}>
                {file.visibility === "public" ? t("book.setPrivate") : t("book.setPublic")}
              </button>
              <button
                className="link-btn"
                onClick={() => void onShare(file.id, "book")}
              >
                {t("book.share")}
              </button>
              <button className="link-btn danger" onClick={() => void deleteFile()}>
                {t("book.delete")}
              </button>
            </>
          )}
          {file.source !== "local" && canManage && (
            <>
              <button className="link-btn" onClick={() => setRebindOpen(true)}>
                {t("book.changeContentSource")}
              </button>
              <button className="link-btn" onClick={() => void onShare(file.id, "book")}>
                {t("book.share")}
              </button>
            </>
          )}
          {file.source === "local" && !canManage && (
            <span className="hint">{t("book.othersUpload")}</span>
          )}
        </div>
      </div>
      <div className="file-meta hint">
        {file.content_source
          ? t("book.contentSourceLine", {
              source: file.source,
              content: file.content_source,
              external: file.content_external_id ?? file.external_id,
            })
          : t("book.uploadedAt", { count: file.chapter_count, time: fmtTime(file.created_at) })}
      </div>
      <SourceBrowserDialog
        open={rebindOpen}
        onClose={() => setRebindOpen(false)}
        instances={instances}
        filter={(i: PluginInstance) => i.enabled && i.id !== file.source && i.capabilities.includes("content")}
        title={t("book.rebindTitle", { label: file.label || file.format })}
        pickLabel={() => t("book.useAsContentSource")}
        onPick={(pick) => (rebinding ? Promise.resolve() : rebind(pick))}
      />
      {rebindError && <div className="error">{rebindError}</div>}

      {sessions.length > 0 && (
        <table className="sessions">
          <thead>
            <tr>
              <th>{t("book.sessions")}</th>
              <th>{t("book.sessionProgress")}</th>
              <th>{t("book.sessionUpdated")}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {sessions.map((s) => {
              return (
                <tr key={s.id}>
                  <td>
                    <Link to={`/read/${file.id}?session=${s.id}`} className="strong">
                      {s.label}
                    </Link>
                  </td>
                  <td>
                    {t("book.sessionProgressAt", {
                      chapter: s.position.chapter_idx + 1,
                      percent: fmtPercent(s),
                    })}
                  </td>
                  <td className="hint">{fmtTime(s.updated_at)}</td>
                  <td className="row-actions">
                    <button
                      className="link-btn"
                      onClick={() => void onShare(file.id, "session", s.id)}
                    >
                      {t("book.shareProgress")}
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}

export function BookDetailPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { t } = useTranslation();
  const { user } = useAuth();
  const toast = useToast();
  const [detail, setDetail] = useState<BookDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [share, setShare] = useState<ShareInfo | null>(null);
  const [attachLabel, setAttachLabel] = useState("");
  const [attachVisibility, setAttachVisibility] = useState<Visibility>("public");
  const [attaching, setAttaching] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [fileCount409, setFileCount409] = useState<number | null>(null);
  const attachRef = { current: null as HTMLInputElement | null };

  const load = useCallback(async () => {
    try {
      const d = await api<BookDetail>(`/books/${id}`);
      setDetail(d);
      setLoadError(null);
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : t("common.failed"));
    }
  }, [id, t]);

  useEffect(() => {
    void load();
  }, [load]);

  async function attach(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setAttaching(true);
    try {
      const form = new FormData();
      form.append("file", file);
      form.append("visibility", attachVisibility);
      if (attachLabel.trim()) form.append("label", attachLabel.trim());
      await api<FileMeta>(`/books/${id}/files`, { method: "POST", body: form });
      setAttachLabel("");
      await load();
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("book.attachFailed"));
    } finally {
      setAttaching(false);
      if (attachRef.current) attachRef.current.value = "";
    }
  }

  /** Create a share for a specific file (book link) or its session. */
  async function makeShare(fileId: string, kind: "book" | "session", sessionId?: string) {
    try {
      const info = await api<ShareInfo>(`/files/${fileId}/shares`, {
        method: "POST",
        body: JSON.stringify({ kind, session_id: sessionId }),
      });
      setShare(info);
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("common.failed_verb"));
    }
  }

  /** Re-pull metadata from the plugin source(s) backing this book. */
  async function refreshFromSource() {
    try {
      await api<BookMeta>(`/books/${id}/refresh`, { method: "POST" });
      toast.push("success", t("book.refreshed"));
      await load();
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("book.opFailed"));
    }
  }

  /**
   * Delete the metadata entry only. Metadata deletion never cascades into
   * files: while files remain the server answers 409, and the caller may
   * then delete the files explicitly first (`withFiles`).
   */
  async function deleteMetadata(withFiles: boolean) {
    const book = detail?.book;
    if (!book) return;
    if (!window.confirm(t("book.confirmDeleteMetadata", { title: book.title }))) return;
    setDeleting(true);
    setFileCount409(null);
    try {
      if (withFiles && detail) {
        // Delete the files one by one (they are independent resources).
        for (const f of detail.files) {
          await api(`/files/${f.id}`, { method: "DELETE" });
        }
        await load();
      }
      await api(`/books/${id}`, { method: "DELETE" });
      navigate("/");
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        const files = (err.details as { files?: string[] } | undefined)?.files ?? [];
        setFileCount409(files.length || null);
        toast.push("warning", err.message);
      } else {
        toast.push("error", err instanceof Error ? err.message : t("book.opFailed"));
      }
    } finally {
      setDeleting(false);
    }
  }

  if (loadError)
    return (
      <div>
        <div className="error">{loadError}</div>
        <Link to="/" className="link-btn">
          {t("common.backToShelf")}
        </Link>
      </div>
    );
  if (!detail) return <div className="page-loading">{t("common.loading")}</div>;
  const { book, files } = detail;
  const canManage = user !== null && (user.role === "admin" || book.created_by === user.id);
  const pluginBacked = files.some((f) => f.source !== "local");

  return (
    <div className="book-detail">
      <Link to="/" className="link-btn">
        <IconArrowLeft size={14} /> {t("common.backToShelf")}
      </Link>
      <h1>{book.title}</h1>
      <p className="hint">
        {t("book.metaLine", {
          authors: book.authors.join(" / ") || t("common.anonymous"),
          count: files.length,
        })}
      </p>
      {book.description && <p className="description">{book.description}</p>}

      {canManage && (
        <div className="row">
          {pluginBacked && (
            <button className="link-btn" onClick={() => void refreshFromSource()}>
              {t("book.refreshFromSource")}
            </button>
          )}
          <button
            className="link-btn danger"
            disabled={deleting}
            onClick={() => void deleteMetadata(false)}
          >
            {t("book.deleteMetadata")}
          </button>
        </div>
      )}
      {fileCount409 !== null && (
        <div className="card">
          <p className="hint">
            {t("book.metadataHasFiles", { count: fileCount409 })}
          </p>
          <button
            className="danger"
            disabled={deleting}
            onClick={() => void deleteMetadata(true)}
          >
            {t("book.deleteMetadataAndFiles")}
          </button>
        </div>
      )}

      <section className="card">
        <h2>{t("book.filesTitle", { count: files.length })}</h2>
        <p className="hint">{t("book.filesHint")}</p>
        {files.map((f) => (
          <FileSection
            key={f.id}
            file={f}
            canManage={user !== null && (user.role === "admin" || f.owner_id === user.id)}
            onChanged={load}
            onShare={makeShare}
          />
        ))}
      </section>

      <section className="card">
        <h2>{t("book.attachTitle")}</h2>
        <div className="inline-form">
          <input
            placeholder={t("book.attachPlaceholder")}
            value={attachLabel}
            onChange={(e) => setAttachLabel(e.target.value)}
          />
          <select
            value={attachVisibility}
            onChange={(e) => setAttachVisibility(e.target.value as Visibility)}
          >
            <option value="private">{t("book.privateTag")}</option>
            <option value="public">{t("book.publicTag")}</option>
          </select>
          <button className="primary" disabled={attaching} onClick={() => attachRef.current?.click()}>
            {attaching ? t("library.uploading") : t("book.attach")}
          </button>
          <input
            ref={(el) => {
              attachRef.current = el;
            }}
            type="file"
            accept=".epub,.txt,.text"
            hidden
            onChange={attach}
          />
        </div>
      </section>

      <section className="card">
        <h2>{t("book.shareTitle")}</h2>
        <div className="row">
          <button
            className="primary"
            onClick={() => files[0] && void makeShare(files[0].id, "book")}
          >
            {t("book.shareBook")}
          </button>
          <span className="hint">{t("book.shareHint")}</span>
        </div>
        {share && (
          <div className="share-box">
            <span className="tag">
              {share.kind === "book" ? t("book.shareBookTag") : t("book.shareSessionTag")}
              {t("book.shareTag")}
            </span>
            <code>{share.url}</code>
            <button
              className="link-btn"
              onClick={() => void navigator.clipboard?.writeText(`${location.origin}${share.url}`)}
            >
              {t("book.copy")}
            </button>
          </div>
        )}
      </section>
    </div>
  );
}