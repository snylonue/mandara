import { useCallback, useEffect, useState, type ChangeEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { ApiError, api, downloadBinary } from "../api";
import { useAuth } from "../auth";
import { BookCover } from "../components/BookCover";
import {
  IconArrowLeft,
  IconEdit,
  IconLayers,
  IconRefresh,
  IconShare,
  IconTrash,
} from "../components/icons";
import { Modal } from "../components/Modal";
import { ExtMetaForm, ExtMetaTable, extMergePatch, type ExtRecord } from "../components/ExtMetaForm";
import { useToast } from "../components/toast";
import {
  SourceBrowserDialog,
  SourceSearchPane,
  usePluginInstances,
  type SourcePick,
} from "../components/SourceSearch";
import type {
  AcquireResult,
  BookDetail,
  BookMeta,
  FileMeta,
  PluginInstance,
  ReadingSession,
  SeriesBrief,
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

  async function rematerialize() {
    try {
      await api<FileMeta>(`/files/${file.id}/rematerialize`, { method: "POST" });
      toast.push("success", t("book.rematerializeDone"));
      await onChanged();
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("book.opFailed"));
    }
  }

  const readable = file.chapter_count > 0;

  return (
    <div className="file-card">
      <div className="file-head">
        <div className="file-title">
          <Link to={`/read/${file.id}`} className="file-label">
            {file.label || file.format}
          </Link>
          <span className={`fmt-badge fmt-${file.format}`}>{file.format}</span>
          {file.source !== "local" && file.format !== "plugin" && (
            <span className="tag" title={t("book.contentSourceLine", {
              source: file.source,
              content: file.content_source,
              external: file.content_external_id ?? file.external_id,
            })}>
              {t("book.pluginTag")}
            </span>
          )}
          <span className={`tag ${file.visibility === "public" ? "tag-public" : ""}`}>
            {file.visibility === "public" ? t("book.publicTag") : t("book.privateTag")}
          </span>
        </div>
        <div className="row-actions">
          {readable && (
            <Link className="mini-btn primary" to={`/read/${file.id}`}>
              {t("book.read")}
            </Link>
          )}
          <button className="mini-btn" onClick={() => void onShare(file.id, "book")}>
            {t("book.share")}
          </button>
          {file.original && (
            <button
              className="mini-btn"
              title={t("book.downloadOriginal")}
              onClick={() =>
                void downloadBinary(
                  `/files/${file.id}/download`,
                  `${file.label || file.format}.${file.original!.ext}`,
                ).catch((err) =>
                  toast.push("error", err instanceof Error ? err.message : t("book.opFailed")),
                )
              }
            >
              {t("book.download")}
            </button>
          )}
          {canManage && file.source === "local" && (
            <>
              <button className="mini-btn" onClick={() => void toggleVisibility()}>
                {file.visibility === "public" ? t("book.setPrivate") : t("book.setPublic")}
              </button>
              <button className="mini-btn danger" onClick={() => void deleteFile()}>
                {t("book.delete")}
              </button>
            </>
          )}
          {canManage && file.source !== "local" && (
            <>
              <button
                className="mini-btn"
                title={t("book.rematerializeHint")}
                onClick={() => void rematerialize()}
              >
                {t("book.rematerialize")}
              </button>
              <button className="mini-btn" onClick={() => setRebindOpen(true)}>
                {t("book.changeContentSource")}
              </button>
            </>
          )}
        </div>
      </div>
      {!canManage && file.source === "local" && (
        <div className="hint file-sub">{t("book.othersUpload")}</div>
      )}
      <div className="hint file-sub">
        {t("book.uploadedAt", { count: file.chapter_count, time: fmtTime(file.created_at) })}
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
  const instances = usePluginInstances();
  const [detail, setDetail] = useState<BookDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [share, setShare] = useState<ShareInfo | null>(null);
  const [attachLabel, setAttachLabel] = useState("");
  const [attachVisibility, setAttachVisibility] = useState<Visibility>("public");
  const [attaching, setAttaching] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [descExpanded, setDescExpanded] = useState(false);
  const [fileCount409, setFileCount409] = useState<number | null>(null);
  // Extended-metadata edit dialog (creator/admin).
  const [extOpen, setExtOpen] = useState(false);
  const [extDraft, setExtDraft] = useState<ExtRecord>({});
  const [extSaving, setExtSaving] = useState(false);
  const attachRef = { current: null as HTMLInputElement | null };
  // Unified "add a file to this book" dialog: upload tab + plugin tab.
  const [attachOpen, setAttachOpen] = useState(false);
  const [attachTab, setAttachTab] = useState<"upload" | "plugin">("upload");
  const [attachPluginPick, setAttachPluginPick] = useState<SourcePick | null>(null);
  const [attachBusy, setAttachBusy] = useState(false);

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

  /** Attach a plugin book's content to this metadata entry. The unified
   *  acquisition endpoint handles `book_id` (attach) + plugin content. */
  async function attachPlugin(pick: SourcePick) {
    setAttachBusy(true);
    try {
      const form = new FormData();
      form.append("book_id", id);
      form.append("plugin_source", pick.instance.id);
      form.append("plugin_book_id", pick.item.id);
      form.append("visibility", attachVisibility);
      if (attachLabel.trim()) form.append("label", attachLabel.trim());
      const result = await api<AcquireResult>("/books", { method: "POST", body: form });
      const file = result.books[0]?.files[0];
      if (!file) throw new Error(t("book.attachNoFile"));
      setAttachLabel("");
      setAttachPluginPick(null);
      toast.push("success", t("book.attachedPlugin", { title: pick.item.title }));
      await load();
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("book.attachFailed"));
    } finally {
      setAttachBusy(false);
    }
  }

  function closeAttachDialog() {
    setAttachOpen(false);
    setAttachPluginPick(null);
    setAttachTab("upload");
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

  // Series membership (assign / move / unassign).
  const [seriesOpen, setSeriesOpen] = useState(false);
  const [seriesList, setSeriesList] = useState<SeriesBrief[]>([]);
  const [seriesPickId, setSeriesPickId] = useState("");
  const [seriesVolume, setSeriesVolume] = useState("");
  const [seriesError, setSeriesError] = useState<string | null>(null);
  const [seriesSaving, setSeriesSaving] = useState(false);

  async function openSeriesDialog() {
    setSeriesError(null);
    setSeriesVolume("");
    try {
      const list = await api<SeriesBrief[]>("/series");
      // Prefer the book's current series; else the first manageable one.
      setSeriesList(list);
      setSeriesPickId(detail?.book.series_id ?? list.find((s) => s.created_by === user?.id)?.id ?? "");
      setSeriesOpen(true);
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("common.failed"));
    }
  }

  async function saveSeries() {
    const book = detail?.book;
    if (!book) return;
    setSeriesSaving(true);
    setSeriesError(null);
    try {
      const body: Record<string, unknown> = {};
      if (seriesPickId) {
        body.series_id = seriesPickId;
        if (seriesVolume.trim()) body.volume_no = Number(seriesVolume.trim());
      } else {
        body.series_id = null;
      }
      await api<BookMeta>(`/books/${book.id}`, { method: "PATCH", body: JSON.stringify(body) });
      setSeriesOpen(false);
      toast.push("success", t("book.seriesSaved"));
      await load();
    } catch (err) {
      setSeriesError(err instanceof Error ? err.message : t("common.failed_verb"));
    } finally {
      setSeriesSaving(false);
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

  /** Save the extended-metadata edit (merge-patch: null clears a key). */
  async function saveExt() {
    const book = detail?.book;
    if (!book) return;
    setExtSaving(true);
    try {
      const patch = extMergePatch((book.ext ?? {}) as ExtRecord, extDraft);
      if (patch) {
        await api<BookMeta>(`/books/${book.id}`, {
          method: "PATCH",
          body: JSON.stringify({ ext: patch }),
        });
      }
      setExtOpen(false);
      toast.push("success", t("book.extSaved"));
      await load();
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("book.opFailed"));
    } finally {
      setExtSaving(false);
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
        // A 404 mid-loop (file already gone) is not fatal: keep going
        // with the remaining files instead of aborting the whole delete.
        for (const f of detail.files) {
          try {
            await api(`/files/${f.id}`, { method: "DELETE" });
          } catch (e) {
            if (e instanceof ApiError && e.status === 404) continue;
            throw e;
          }
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
  const firstReadable = files.find((f) => f.chapter_count > 0);
  const descLong = (book.description?.length ?? 0) > 120;

  return (
    <div className="book-detail">
      <div className="page-back">
        <Link to="/" className="link-btn">
          <IconArrowLeft size={14} /> {t("common.backToShelf")}
        </Link>
      </div>

      {/* hero: cover + metadata + actions */}
      <div className="detail-hero">
        <div className="detail-cover">
          <BookCover bookId={book.id} title={book.title} coverUrl={book.cover_url} />
        </div>
        <div className="detail-info">
          {(detail.series || book.volume_no > 0) && (
            <div>
              {detail.series ? (
                <Link className="chip" to={`/series/${detail.series.id}`}>
                  <IconLayers size={13} />
                  {t("book.seriesLine", {
                    title: detail.series.title,
                    volume: book.volume_no,
                  })}
                </Link>
              ) : (
                <span className="chip">第{book.volume_no}卷</span>
              )}
            </div>
          )}
          <h1>{book.title}</h1>
          <div className="detail-meta">
            <span>{book.authors.join(" / ") || t("common.anonymous")}</span>
            <span>·</span>
            <span>{t("book.metaCount", { count: files.length })}</span>
          </div>
          <ExtMetaTable kind="book" value={(book.ext ?? {}) as ExtRecord} />
          {book.description && (
            <p className={`detail-desc ${descExpanded ? "expanded" : ""}`}>
              {book.description}
            </p>
          )}
          {descLong && (
            <button
              className="detail-desc-toggle"
              onClick={() => setDescExpanded((v) => !v)}
            >
              {descExpanded ? t("book.descCollapse") : t("book.descExpand")} ▾
            </button>
          )}
          <div className="detail-actions">
            {firstReadable && (
              <button
                className="primary"
                onClick={() => navigate(`/read/${firstReadable.id}`)}
              >
                {t("book.startReading")}
              </button>
            )}
            {files[0] && (
              <button className="ghost" onClick={() => void makeShare(files[0].id, "book")}>
                <IconShare size={14} /> {t("book.shareBookShort")}
              </button>
            )}
            {canManage && pluginBacked && (
              <button className="ghost" disabled={deleting} onClick={() => void refreshFromSource()}>
                <IconRefresh size={14} /> {t("book.refreshFromSource")}
              </button>
            )}
            {canManage && (
              <button className="ghost" onClick={() => void openSeriesDialog()}>
                {t("book.manageSeries")}
              </button>
            )}
            {canManage && (
              <button
                className="ghost"
                onClick={() => {
                  setExtDraft({ ...(book.ext ?? {}) });
                  setExtOpen(true);
                }}
              >
                <IconEdit size={14} /> {t("book.editInfo")}
              </button>
            )}
            {canManage && (
              <button
                className="ghost danger-text"
                disabled={deleting}
                onClick={() => void deleteMetadata(false)}
              >
                <IconTrash size={14} /> {t("book.deleteMetadata")}
              </button>
            )}
          </div>
        </div>
      </div>
      {share && (
        <div className="share-box">
          <span className="tag">
            {share.kind === "book" ? t("book.shareBookTag") : t("book.shareSessionTag")}
            {t("book.shareTag")}
          </span>
          <code>{share.url}</code>
          <button
            className="mini-btn"
            onClick={() => void navigator.clipboard?.writeText(`${location.origin}${share.url}`)}
          >
            {t("book.copy")}
          </button>
        </div>
      )}
      {fileCount409 !== null && (
        <div className="error">
          <p className="hint">{t("book.metadataHasFiles", { count: fileCount409 })}</p>
          <button
            className="danger"
            disabled={deleting}
            onClick={() => void deleteMetadata(true)}
          >
            {t("book.deleteMetadataAndFiles")}
          </button>
        </div>
      )}

      <section className="detail-section">
        <h2>
          {t("book.filesTitle", { count: files.length })}
        </h2>
        {files.length === 0 && <p className="hint">{t("book.noFiles")}</p>}
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

      <section className="detail-section">
        <h2>{t("book.attachTitle")}</h2>
        <div className="panel-box">
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
            <button className="primary" onClick={() => setAttachOpen(true)}>
              {t("book.attachOpen")}
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
        </div>
      </section>

      {/* Unified attach dialog: upload a file or pull content from a
          plugin source (metadata stays on this book). */}
      <Modal
        open={attachOpen}
        onClose={closeAttachDialog}
        title={t("book.attachTitle")}
        wide
      >
        <div className="row">
          <label className="radio">
            <input
              type="radio"
              checked={attachTab === "upload"}
              onChange={() => setAttachTab("upload")}
            />
            {t("library.tabUpload")}
          </label>
          <label className="radio">
            <input
              type="radio"
              checked={attachTab === "plugin"}
              onChange={() => setAttachTab("plugin")}
            />
            {t("library.tabPlugin")}
          </label>
        </div>

        {attachTab === "upload" && (
          <div className="field">
            <p className="hint">{t("book.attachUploadHint")}</p>
            <button
              className="file-picker"
              disabled={attaching}
              onClick={() => attachRef.current?.click()}
            >
              {attaching ? t("library.uploading") : t("book.attach")}
            </button>
            <input
              ref={(el) => {
                attachRef.current = el;
              }}
              type="file"
              accept=".epub,.txt,.text"
              hidden
              onChange={(e) => {
                void attach(e);
                setAttachOpen(false);
              }}
            />
          </div>
        )}

        {attachTab === "plugin" && (
          <div className="field">
            <SourceSearchPane
              instances={instances}
              filter={(i) =>
                i.enabled &&
                (i.capabilities.includes("content") ||
                  i.capabilities.includes("book-file"))
              }
              pickLabel={() => t("book.useAsContentSource")}
              onPick={(pick) => {
                setAttachPluginPick(pick);
              }}
            />
            {attachPluginPick && (
              <p className="hint">
                {t("book.attachPluginPicked", {
                  title: attachPluginPick.item.title,
                  plugin: attachPluginPick.instance.name,
                })}
              </p>
            )}
          </div>
        )}

        <div className="modal-actions">
          <button onClick={closeAttachDialog}>{t("library.cancel")}</button>
          <button
            className="primary"
            disabled={!attachPluginPick || attachBusy}
            onClick={() => attachPluginPick && void attachPlugin(attachPluginPick)}
          >
            {attachBusy ? t("library.uploading") : t("book.attachConfirm")}
          </button>
        </div>
      </Modal>

      {/* Series membership control (creator/admin of the book). */}
      <Modal
        open={seriesOpen}
        onClose={() => setSeriesOpen(false)}
        title={t("book.seriesTitle")}
      >
        <div className="field">
          <select
            value={seriesPickId}
            onChange={(e) => setSeriesPickId(e.target.value)}
            aria-label={t("book.seriesLabel")}
          >
            <option value="">{t("book.seriesNone")}</option>
            {seriesList.map((s) => (
              <option key={s.id} value={s.id}>
                {s.title}
              </option>
            ))}
          </select>
          <input
            type="number"
            min={1}
            placeholder={t("book.seriesVolumePlaceholder")}
            value={seriesVolume}
            onChange={(e) => setSeriesVolume(e.target.value)}
            disabled={!seriesPickId}
          />
          <p className="hint">{t("book.seriesHint")}</p>
        </div>
        {seriesError && <div className="error">{seriesError}</div>}
        <div className="modal-actions">
          <button onClick={() => setSeriesOpen(false)}>{t("library.cancel")}</button>
          <button className="primary" disabled={seriesSaving} onClick={() => void saveSeries()}>
            {seriesSaving ? t("library.uploading") : t("series.save")}
          </button>
        </div>
      </Modal>

      {/* Extended metadata edit (creator/admin). */}
      <Modal open={extOpen} onClose={() => setExtOpen(false)} title={t("book.editInfoTitle")}>
        <ExtMetaForm kind="book" value={extDraft} onChange={setExtDraft} />
        <div className="modal-actions">
          <button onClick={() => setExtOpen(false)}>{t("library.cancel")}</button>
          <button className="primary" disabled={extSaving} onClick={() => void saveExt()}>
            {extSaving ? t("library.uploading") : t("series.save")}
          </button>
        </div>
      </Modal>
    </div>
  );
}