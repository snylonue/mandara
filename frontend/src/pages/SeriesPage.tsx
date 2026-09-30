// Series page: metadata + member volumes in order, with manage controls
// for the series creator/admin (edit metadata, reorder, add/remove books,
// delete the series). Volumes are ordinary books (`BookMeta.volume_no`).

import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useAuth } from "../auth";
import { BookCover } from "../components/BookCover";
import { ShelfBookCard } from "../components/ShelfCard";
import { ExtMetaForm, ExtMetaTable, extMergePatch, type ExtRecord } from "../components/ExtMetaForm";
import {
  IconArrowLeft,
  IconEdit,
  IconPlus,
  IconTrash,
} from "../components/icons";
import { Modal } from "../components/Modal";
import { useToast } from "../components/toast";
import {
  FINISHED_AT,
  attachProgress,
  hasStarted,
  isFinished,
  progressByFile,
  sessionPercent,
  type WithProgress,
} from "../progress";
import type {
  BookDetail,
  BookListEntry,
  SeriesBrief,
  SeriesDetail,
  SessionList,
} from "../types";

/// One volume card of the series. Manageable cards additionally show
/// hover-revealed reorder/remove controls overlaid on the cover; the card
/// itself links to the book whenever the caller may open its page (a
/// readable file, or a metadata-only volume they can attach content to).
function VolumeCard({
  entry,
  seriesTitle,
  canManage,
  orderIndex,
  total,
  onMove,
  onRemove,
}: {
  entry: WithProgress<BookDetail>;
  seriesTitle: string;
  canManage: boolean;
  orderIndex: number;
  total: number;
  onMove: (from: number, to: number) => void;
  onRemove: (index: number) => void;
}) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const { book, files } = entry;
  const readable = files.length > 0;
  // A metadata-only volume (e.g. one 卷 of a bangumi series import) has
  // no file yet: its page is the only place to attach content, so its
  // owner/admin must still be able to open it. Volumes whose files all
  // belong to someone else stay unlinked — `GET /api/books/{id}` answers
  // 404 for them.
  const openable =
    readable || book.created_by === user?.id || user?.role === "admin";
  const chapterCount = files.reduce((n, f) => n + f.chapter_count, 0);
  const volumeNo = book.volume_no || orderIndex + 1;
  // Bangumi-style series repeat the same title on every volume: lead with
  // 第 N 卷 there, and keep the real title when a volume has its own.
  const label =
    book.title === seriesTitle
      ? t("series.volumeLabel", { volume: volumeNo })
      : book.title;
  return (
    <ShelfBookCard
      entry={entry}
      to={openable ? undefined : null}
      muted={!readable}
      finished={isFinished(entry.progress)}
      volumeNo={volumeNo}
      title={label}
      meta={t("series.chaptersOfVolume", { count: chapterCount })}
      actions={
        canManage && (
          <div className="series-card-actions">
            <button
              className="btn sm"
              disabled={orderIndex === 0}
              onClick={() => onMove(orderIndex, orderIndex - 1)}
              title={t("series.moveUp")}
            >
              ↑
            </button>
            <button
              className="btn sm"
              disabled={orderIndex === total - 1}
              onClick={() => onMove(orderIndex, orderIndex + 1)}
              title={t("series.moveDown")}
            >
              ↓
            </button>
            <button className="btn sm danger" onClick={() => onRemove(orderIndex)}>
              {t("series.remove")}
            </button>
          </div>
        )
      }
    />
  );
}

export function SeriesPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { t } = useTranslation();
  const { user } = useAuth();
  const toast = useToast();

  const [detail, setDetail] = useState<SeriesDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionList>([]);
  // Local working copy of the member order while managing (saved on 保存).
  const [order, setOrder] = useState<string[] | null>(null);
  const [editOpen, setEditOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [pick, setPick] = useState<Set<string>>(new Set());
  const [unassigned, setUnassigned] = useState<BookListEntry[]>([]);

  const load = useCallback(async () => {
    try {
      const d = await api<SeriesDetail>(`/series/${id}`);
      setDetail(d);
      setOrder(null);
      setLoadError(null);
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : t("common.failed"));
    }
  }, [id, t]);

  useEffect(() => {
    void load();
  }, [load]);

  // Reading progress of every volume (one request for the whole page).
  useEffect(() => {
    void api<SessionList>("/sessions")
      .then(setSessions)
      .catch(() => setSessions([]));
  }, []);

  const canManage = useMemo(() => {
    if (!user || !detail) return false;
    return user.role === "admin" || detail.series.created_by === user.id;
  }, [user, detail]);

  const series: SeriesBrief | null = detail?.series ?? null;

  // Members in working order (or the server order before any change).
  const memberIds = order ?? detail?.books.map((b) => b.book.id) ?? [];
  const membersByOrder = useMemo(() => {
    const byId = new Map(detail?.books.map((b) => [b.book.id, b]) ?? []);
    return memberIds.map((mid) => byId.get(mid)).filter((b): b is BookDetail => !!b);
  }, [detail, memberIds]);

  // Progress by file, then by volume (the busiest session per file wins).
  const byFile = useMemo(() => {
    const counts = new Map<string, number>();
    for (const entry of detail?.books ?? []) {
      for (const file of entry.files) counts.set(file.id, file.chapter_count);
    }
    return progressByFile(sessions, (fid) => counts.get(fid) ?? 0);
  }, [detail, sessions]);

  const volumes = useMemo(() => attachProgress(membersByOrder, byFile), [membersByOrder, byFile]);
  const readVolumes = volumes.filter((v) => hasStarted(v.progress)).length;
  const totalChapters = membersByOrder.reduce(
    (n, e) => n + e.files.reduce((m, f) => m + f.chapter_count, 0),
    0,
  );

  // 继续阅读: the first readable volume that is not finished yet — the
  // button disappears once every volume is read.
  const continueTarget = useMemo(() => {
    const readable = volumes.filter((v) => v.files.some((f) => f.chapter_count > 0));
    const pick = readable.find((v) => !isFinished(v.progress));
    if (!pick) return null;
    const file = pick.files.find((f) => f.id === pick.progress?.fileId) ?? pick.files[0];
    if (!file) return null;
    const session = pick.progress?.session;
    const resume = session && sessionPercent(session, file.chapter_count) < FINISHED_AT;
    return {
      volumeNo: pick.book.volume_no,
      href: `/read/${file.id}${resume ? `?session=${session!.id}` : ""}`,
      finished: isFinished(pick.progress),
    };
  }, [volumes]);

  const orderDirty = order !== null && order.join(",") !== (detail?.books.map((b) => b.book.id) ?? []).join(",");

  async function openAddDialog() {
    try {
      const list = await api<BookListEntry[]>("/books");
      // Only books the caller may manage (server checks it too).
      const managed = list.filter(
        (e) =>
          !e.book.series_id &&
          (user?.role === "admin" || e.book.created_by === user?.id),
      );
      setUnassigned(managed);
      setPick(new Set());
      setAddOpen(true);
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("common.failed"));
    }
  }

  async function saveOrder() {
    if (!series) return;
    try {
      await api<SeriesDetail>(`/series/${series.id}/members`, {
        method: "PUT",
        body: JSON.stringify({ book_ids: memberIds }),
      });
      toast.push("success", t("series.orderSaved"));
      await load();
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("common.failed_verb"));
    }
  }

  async function deleteSeries() {
    if (!series) return;
    if (!window.confirm(t("series.confirmDelete", { title: series.title }))) return;
    try {
      await api(`/series/${series.id}`, { method: "DELETE" });
      navigate("/");
    } catch (err) {
      toast.push("error", err instanceof Error ? err.message : t("common.failed_verb"));
    }
  }

  if (loadError)
    return (
      <div>
        <div className="error">{loadError}</div>
        <Link to="/" className="btn link">
          {t("common.backToShelf")}
        </Link>
      </div>
    );
  if (!detail) return <div className="page-loading">{t("common.loading")}</div>;

  return (
    <div className="book-detail">
      <div className="page-back">
        <Link to="/" className="btn link">
          <IconArrowLeft size={14} /> {t("common.backToShelf")}
        </Link>
      </div>

      <div className="detail-hero">
        {membersByOrder[0] && (
          <div className="detail-hero-bg" aria-hidden>
            <BookCover
              bookId={membersByOrder[0].book.id}
              title={membersByOrder[0].book.title}
              coverUrl={membersByOrder[0].book.cover_url}
            />
          </div>
        )}
        <div className="detail-hero-inner">
        {membersByOrder[0] && (
          <div className="detail-cover">
            <BookCover
              bookId={membersByOrder[0].book.id}
              title={membersByOrder[0].book.title}
              coverUrl={membersByOrder[0].book.cover_url}
            />
          </div>
        )}
        <div className="detail-info">
          {series?.authors.length ? (
            <span className="tag lg">{series.authors.join(" / ")}</span>
          ) : null}
          <h1>{series?.title}</h1>
          <div className="detail-meta">
            <span>{t("series.volumes", { count: series?.volume_count ?? 0 })}</span>
            {totalChapters > 0 && (
              <>
                <span>·</span>
                <span>{t("common.chaptersTotal", { count: totalChapters })}</span>
              </>
            )}
            {readVolumes > 0 && (
              <>
                <span>·</span>
                <span>{t("library.seriesRead", { read: readVolumes, total: volumes.length })}</span>
              </>
            )}
            {series?.status && (
              <>
                <span>·</span>
                <span className={`tag ext-status-${series.status}`}>{t(`ext.${series.status}`)}</span>
              </>
            )}
            {series?.ext?.total_volumes != null && (
              <>
                <span>·</span>
                <span>{t("ext.totalVolumesLine", { count: series.ext.total_volumes })}</span>
              </>
            )}
          </div>
          {series?.description && <p className="detail-desc">{series.description}</p>}
          <ExtMetaTable kind="series" value={(series?.ext ?? {}) as ExtRecord} />
          <div className="detail-actions">
            {continueTarget && (
              <Link className="btn primary" to={continueTarget.href}>
                {t("series.continueReading", { volume: continueTarget.volumeNo })}
              </Link>
            )}
            {canManage && (
              <>
                {!continueTarget && (
                  <button className="btn primary" onClick={() => void openAddDialog()}>
                    <IconPlus size={14} /> {t("series.addBooks")}
                  </button>
                )}
                <button className="btn ghost" onClick={() => setEditOpen(true)}>
                  <IconEdit size={14} /> {t("series.edit")}
                </button>
                {continueTarget && (
                  <button className="btn ghost" onClick={() => void openAddDialog()}>
                    <IconPlus size={14} /> {t("series.addBooks")}
                  </button>
                )}
                {orderDirty && (
                  <>
                    <button className="btn primary" onClick={() => void saveOrder()}>
                      {t("series.saveOrder")}
                    </button>
                    <button onClick={() => setOrder(null)}>{t("series.cancelOrder")}</button>
                    <span className="hint">{t("series.orderHint")}</span>
                  </>
                )}
                {!orderDirty && (
                  <button
                    className="btn ghost danger-text push-end"
                    onClick={() => void deleteSeries()}
                  >
                    <IconTrash size={14} /> {t("series.delete")}
                  </button>
                )}
              </>
            )}
          </div>
        </div>
        </div>
      </div>

      <div className="shelf-section-title">
        {t("series.volumes", { count: volumes.length })}
      </div>
      <div className="book-grid">
        {volumes.map((entry, i) => (
          <VolumeCard
            key={entry.book.id}
            entry={entry}
            seriesTitle={series?.title ?? ""}
            canManage={canManage}
            orderIndex={i}
            total={volumes.length}
            onMove={(from, to) => {
              const next = [...memberIds];
              const [x] = next.splice(from, 1);
              next.splice(to, 0, x);
              setOrder(next);
            }}
            onRemove={(index) => {
              const next = [...memberIds];
              next.splice(index, 1);
              setOrder(next);
            }}
          />
        ))}
      </div>

      <EditSeriesDialog
        open={editOpen}
        series={series}
        onClose={() => setEditOpen(false)}
        onSaved={async () => {
          setEditOpen(false);
          await load();
        }}
      />

      <AddMembersDialog
        open={addOpen}
        seriesId={series?.id ?? ""}
        members={memberIds}
        unassigned={unassigned}
        pick={pick}
        setPick={setPick}
        onClose={() => setAddOpen(false)}
        onSaved={async () => {
          setAddOpen(false);
          await load();
        }}
        onError={(msg) => toast.push("error", msg)}
      />
    </div>
  );
}

function EditSeriesDialog({
  open,
  series,
  onClose,
  onSaved,
}: {
  open: boolean;
  series: SeriesBrief | null;
  onClose: () => void;
  onSaved: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [title, setTitle] = useState("");
  const [authors, setAuthors] = useState("");
  const [description, setDescription] = useState("");
  const [ext, setExt] = useState<ExtRecord>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Initialize the form whenever the dialog opens.
  useEffect(() => {
    if (open && series) {
      setTitle(series.title);
      setAuthors(series.authors.join("，"));
      setDescription(series.description ?? "");
      setExt({ ...(series.ext ?? {}) });
      setError(null);
    }
  }, [open, series]);

  async function submit() {
    if (!series) return;
    setSaving(true);
    setError(null);
    try {
      const body: Record<string, unknown> = {};
      if (title.trim()) body.title = title.trim();
      if (authors.trim()) body.authors = authors.split(/[,，]/).map((s) => s.trim()).filter(Boolean);
      if (description.trim()) body.description = description.trim();
      const extPatch = extMergePatch((series.ext ?? {}) as ExtRecord, ext);
      if (extPatch) body.ext = extPatch;
      await api<SeriesBrief>(`/series/${series.id}`, { method: "PATCH", body: JSON.stringify(body) });
      toast.push("success", t("series.saved"));
      await onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : t("common.failed_verb"));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal open={open} onClose={onClose} title={t("series.editTitle")}>
      <div className="field">
        <input placeholder={t("library.bookPlaceholder")} value={title} onChange={(e) => setTitle(e.target.value)} />
        <input placeholder={t("library.authorsPlaceholder")} value={authors} onChange={(e) => setAuthors(e.target.value)} />
        <textarea placeholder={t("library.descriptionPlaceholder")} value={description} onChange={(e) => setDescription(e.target.value)} />
        <ExtMetaForm kind="series" value={ext} onChange={setExt} />
      </div>
      {error && <div className="error">{error}</div>}
      <div className="modal-actions">
        <button onClick={onClose}>{t("library.cancel")}</button>
        <button className="btn primary" onClick={() => void submit()} disabled={saving}>
          {saving ? t("library.uploading") : t("series.save")}
        </button>
      </div>
    </Modal>
  );
}

function AddMembersDialog({
  open,
  seriesId,
  members,
  unassigned,
  pick,
  setPick,
  onClose,
  onSaved,
  onError,
}: {
  open: boolean;
  seriesId: string;
  members: string[];
  unassigned: BookListEntry[];
  pick: Set<string>;
  setPick: (p: Set<string>) => void;
  onClose: () => void;
  onSaved: () => Promise<void>;
  onError: (msg: string) => void;
}) {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);

  async function submit() {
    setSaving(true);
    try {
      const added = unassigned.filter((e) => pick.has(e.book.id)).map((e) => e.book.id);
      await api<SeriesDetail>(`/series/${seriesId}/members`, {
        method: "PUT",
        body: JSON.stringify({ book_ids: [...members, ...added] }),
      });
      await onSaved();
    } catch (err) {
      onError(err instanceof Error ? err.message : t("common.failed_verb"));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal open={open} onClose={onClose} title={t("series.addBooks")} wide>
      {unassigned.length === 0 ? (
        <p className="hint">{t("series.noUnassigned")}</p>
      ) : (
        <ul className="picker-list">
          {unassigned.map((e) => (
            <li key={e.book.id}>
              <label className="row">
                <input
                  type="checkbox"
                  checked={pick.has(e.book.id)}
                  onChange={(ev) => {
                    const next = new Set(pick);
                    if (ev.target.checked) next.add(e.book.id);
                    else next.delete(e.book.id);
                    setPick(next);
                  }}
                />
                <span>{e.book.title}</span>
                <span className="hint">
                  {e.book.authors.join(" / ") || t("common.anonymous")}
                </span>
              </label>
            </li>
          ))}
        </ul>
      )}
      <div className="modal-actions">
        <button onClick={onClose}>{t("library.cancel")}</button>
        <button
          className="btn primary"
          disabled={saving || pick.size === 0}
          onClick={() => void submit()}
        >
          {saving ? t("library.uploading") : t("series.add")}
        </button>
      </div>
    </Modal>
  );
}