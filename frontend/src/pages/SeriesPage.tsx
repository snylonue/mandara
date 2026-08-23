// Series page: metadata + member volumes in order, with manage controls
// for the series creator/admin (edit metadata, reorder, add/remove books,
// delete the series). Volumes are ordinary books (`BookMeta.volume_no`).

import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useAuth } from "../auth";
import { IconArrowLeft } from "../components/icons";
import { Modal } from "../components/Modal";
import { useToast } from "../components/toast";
import type { BookDetail, BookListEntry, SeriesBrief, SeriesDetail } from "../types";

function Cover({ bookId, title }: { bookId: string; title: string }) {
  const [failed, setFailed] = useState(false);
  return failed ? (
    <div className="book-cover-fallback">
      <span>{title.trim().charAt(0) || "书"}</span>
    </div>
  ) : (
    <img
      src={`/api/books/${bookId}/cover`}
      alt=""
      loading="lazy"
      onError={() => setFailed(true)}
    />
  );
}

/// One volume card of the series. In manage mode the card shows the
/// reorder/remove controls instead of linking.
function VolumeCard({
  entry,
  canManage,
  orderIndex,
  total,
  onMove,
  onRemove,
}: {
  entry: BookDetail;
  canManage: boolean;
  orderIndex: number;
  total: number;
  onMove: (from: number, to: number) => void;
  onRemove: (index: number) => void;
}) {
  const { t } = useTranslation();
  const { book, files } = entry;
  const readable = files.length > 0;
  const chapterCount = files.reduce((n, f) => n + f.chapter_count, 0);
  const inner = (
    <>
      <div className="book-cover">
        <Cover bookId={book.id} title={book.title} />
        <div className="book-badges">
          <span className="tag tag-volume">第{book.volume_no || orderIndex + 1}卷</span>
        </div>
      </div>
      <div className="book-card-body">
        <div className="book-title" title={book.title}>
          {book.title}
        </div>
        <div className="book-meta">
          {t("series.chaptersOfVolume", { count: chapterCount })}
        </div>
      </div>
    </>
  );
  return (
    <div className={`book-card ${!readable ? "book-card-muted" : ""}`}>
      {readable && !canManage ? <Link to={`/book/${book.id}`}>{inner}</Link> : inner}
      {canManage && (
        <div className="series-card-actions">
          <button
            disabled={orderIndex === 0}
            onClick={() => onMove(orderIndex, orderIndex - 1)}
            title={t("series.moveUp")}
          >
            ↑
          </button>
          <button
            disabled={orderIndex === total - 1}
            onClick={() => onMove(orderIndex, orderIndex + 1)}
            title={t("series.moveDown")}
          >
            ↓
          </button>
          <button className="danger" onClick={() => onRemove(orderIndex)}>
            {t("series.remove")}
          </button>
        </div>
      )}
    </div>
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
        <Link to="/" className="link-btn">
          {t("common.backToShelf")}
        </Link>
      </div>
    );
  if (!detail) return <div className="page-loading">{t("common.loading")}</div>;

  return (
    <div className="book-detail">
      <Link to="/" className="link-btn">
        <IconArrowLeft size={14} /> {t("common.backToShelf")}
      </Link>
      <h1>{series?.title}</h1>
      <p className="hint">
        {t("series.metaLine", {
          authors: series?.authors.join(" / ") || t("common.anonymous"),
          count: series?.volume_count ?? 0,
        })}
      </p>
      {series?.description && <p className="description">{series.description}</p>}

      {canManage && (
        <div className="row">
          <button className="link-btn" onClick={() => setEditOpen(true)}>
            {t("series.edit")}
          </button>
          <button className="link-btn" onClick={() => void openAddDialog()}>
            {t("series.addBooks")}
          </button>
          {orderDirty ? (
            <>
              <button className="link-btn" onClick={() => void saveOrder()}>
                {t("series.saveOrder")}
              </button>
              <button className="link-btn" onClick={() => setOrder(null)}>
                {t("series.cancelOrder")}
              </button>
            </>
          ) : (
            <button className="link-btn danger" onClick={() => void deleteSeries()}>
              {t("series.delete")}
            </button>
          )}
        </div>
      )}
      {orderDirty && <p className="hint">{t("series.orderHint")}</p>}

      <div className="book-grid">
        {membersByOrder.map((entry, i) => (
          <VolumeCard
            key={entry.book.id}
            entry={entry}
            canManage={canManage}
            orderIndex={i}
            total={membersByOrder.length}
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
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Initialize the form whenever the dialog opens.
  useEffect(() => {
    if (open && series) {
      setTitle(series.title);
      setAuthors(series.authors.join("，"));
      setDescription(series.description ?? "");
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
      </div>
      {error && <div className="error">{error}</div>}
      <div className="modal-actions">
        <button onClick={onClose}>{t("library.cancel")}</button>
        <button className="primary" onClick={() => void submit()} disabled={saving}>
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
          className="primary"
          disabled={saving || pick.size === 0}
          onClick={() => void submit()}
        >
          {saving ? t("library.uploading") : t("series.add")}
        </button>
      </div>
    </Modal>
  );
}