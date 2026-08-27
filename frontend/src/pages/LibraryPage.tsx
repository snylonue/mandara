import { useCallback, useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useAuth } from "../auth";
import { BookCover } from "../components/BookCover";
import { AddDialog, type AddMode } from "../components/AddDialog";
import { useToast } from "../components/toast";
import { usePluginInstances } from "../components/SourceSearch";
import type { BookListEntry, BookMeta, SeriesBrief } from "../types";
import {
  IconBook,
  IconPlugin,
  IconPrivate,
  IconPublic,
} from "../components/icons";

/** Cover image: stored bytes first (404 → fallbacks), then remote URL. */

function VolumeBadge({ book }: { book: BookMeta }) {
  return book.volume_no > 0 ? <span className="tag tag-volume">第{book.volume_no}卷</span> : null;
}

function BookCard({ entry }: { entry: BookListEntry }) {
  const { t } = useTranslation();
  const { book, files } = entry;
  const authors = book.authors.length ? book.authors.join(" / ") : t("common.anonymous");
  const chapterCount = files.reduce((n, f) => n + f.chapter_count, 0);
  // Aggregate badges over all files: public wins over private; plugin
  // shown when any file comes from a plugin source.
  const anyPublic = files.some((f) => f.visibility === "public");
  const isPlugin = files.some((f) => f.source !== "local");
  return (
    <Link className="book-card" to={`/book/${book.id}`}>
      <div className="book-cover">
        <BookCover bookId={book.id} title={book.title} coverUrl={book.cover_url} />
        <div className="book-badges">
          <VolumeBadge book={book} />
          <span
            className={`badge-icon ${anyPublic ? "badge-public" : "badge-private"}`}
            title={anyPublic ? t("book.publicTag") : t("book.privateTag")}
          >
            {anyPublic ? <IconPublic size={12} /> : <IconPrivate size={12} />}
          </span>
          {isPlugin && (
            <span className="badge-icon badge-plugin" title={t("book.pluginTag")}>
              <IconPlugin size={12} />
            </span>
          )}
        </div>
      </div>
      <div className="book-card-body">
        <div className="book-title" title={book.title}>
          {book.title}
        </div>
        <div className="book-meta">
          {authors} · {chapterCount} {t("common.chapters")}
        </div>
      </div>
    </Link>
  );
}

/** Skeleton card shown while the library list loads. */
function BookCardSkeleton() {
  return (
    <div className="book-card book-card-skeleton" aria-hidden>
      <div className="book-cover skeleton" />
      <div className="book-card-body">
        <div className="skeleton skeleton-line w70" />
        <div className="skeleton skeleton-line w45" />
      </div>
    </div>
  );
}

export function LibraryPage() {
  const { user } = useAuth();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const instances = usePluginInstances();
  const toast = useToast();
  const [entries, setEntries] = useState<BookListEntry[] | null>(null);
  const [q, setQ] = useState("");
  // The unified add dialog (添加书籍 / 新建系列 → 一个对话框).
  const [addOpen, setAddOpen] = useState(false);
  const [addMode, setAddMode] = useState<AddMode>("book");

  const load = useCallback(async (query: string) => {
    try {
      const list = await api<BookListEntry[]>(`/books?q=${encodeURIComponent(query)}`);
      setEntries(list);
    } catch (err) {
      setEntries([]);
      toast.push("error", err instanceof Error ? err.message : t("common.failed"));
    }
  }, [toast, t]);

  useEffect(() => {
    void load(q);
  }, [load, q]);

  // Group the (series-sorted) list into series groups + standalone books,
  // preserving order.
  const groups: { series: SeriesBrief | null; entries: BookListEntry[] }[] = [];
  for (const entry of entries ?? []) {
    const key = entry.series?.id ?? null;
    const last = groups[groups.length - 1];
    if (last && last.series?.id === key) {
      last.entries.push(entry);
    } else {
      groups.push({ series: entry.series ?? null, entries: [entry] });
    }
  }

  return (
    <div>
      <div className="toolbar">
        <input
          className="search"
          placeholder={t("library.searchPlaceholder")}
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        {user?.role === "admin" && (
          <span className="hint admin-note" title={t("library.adminHint")}>
            {t("library.adminHint")}
          </span>
        )}
        <button
          className="primary"
          onClick={() => {
            setAddMode("book");
            setAddOpen(true);
          }}
        >
          {t("library.addBook")}
        </button>
        <button
          onClick={() => {
            setAddMode("series");
            setAddOpen(true);
          }}
        >
          {t("series.create")}
        </button>
      </div>
      {entries === null && (
        <div className="book-grid">
          {Array.from({ length: 8 }, (_, i) => (
            <BookCardSkeleton key={`sk-${i}`} />
          ))}
        </div>
      )}
      {entries !== null &&
        groups.map((group) => (
          <section className="series-group" key={group.series?.id ?? "standalone"}>
            {group.series && (
              <Link className="series-group-head" to={`/series/${group.series.id}`}>
                <h2>{group.series.title}</h2>
                <span className="tag tag-volume">
                  {t("series.volumes", { count: group.entries.length })}
                </span>
                {group.series.authors.length > 0 && (
                  <span className="hint">{group.series.authors.join(" / ")}</span>
                )}
              </Link>
            )}
            <div className="book-grid">
              {group.entries.map((e) => (
                <BookCard key={e.book.id} entry={e} />
              ))}
            </div>
          </section>
        ))}
      {entries !== null && entries.length === 0 && (
        <div className="empty-state">
          <IconBook size={40} className="empty-state-icon" />
          <p className="strong">{t("library.emptyTitle")}</p>
          <p className="hint">{t("library.empty")}</p>
        </div>
      )}

      <AddDialog
        open={addOpen}
        onClose={() => setAddOpen(false)}
        instances={instances}
        initialMode={addMode}
        onAdded={() => {
          setQ("");
          void load("");
        }}
        onSeriesCreated={(s) => {
          navigate(`/series/${s.id}`);
        }}
      />
    </div>
  );
}