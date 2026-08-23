import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useAuth } from "../auth";
import { AddBookDialog } from "../components/AddBookDialog";
import { useToast } from "../components/toast";
import { usePluginInstances } from "../components/SourceSearch";
import type { BookListEntry, BookMeta } from "../types";
import {
  IconBook,
  IconPlugin,
  IconPrivate,
  IconPublic,
} from "../components/icons";

/** Cover image: stored bytes first (404 → fallbacks), then remote URL. */
function BookCover({ book }: { book: BookMeta }) {
  const [storedFailed, setStoredFailed] = useState(false);
  if (!storedFailed) {
    return (
      <img
        src={`/api/books/${book.id}/cover`}
        alt=""
        loading="lazy"
        onError={() => setStoredFailed(true)}
      />
    );
  }
  if (book.cover_url) {
    return <img src={book.cover_url} alt="" loading="lazy" />;
  }
  return (
    <div className="book-cover-fallback">
      <span>{book.title.trim().charAt(0) || "书"}</span>
    </div>
  );
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
        <BookCover book={book} />
        <div className="book-badges">
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
  const instances = usePluginInstances();
  const toast = useToast();
  const [entries, setEntries] = useState<BookListEntry[] | null>(null);
  const [q, setQ] = useState("");
  // The single add-book dialog (获取书籍 → 添加元数据) covering uploads
  // and plugin sources alike.
  const [addOpen, setAddOpen] = useState(false);

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

  return (
    <div>
      <div className="toolbar">
        <input
          className="search"
          placeholder={t("library.searchPlaceholder")}
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <button className="primary" onClick={() => setAddOpen(true)}>
          {t("library.addBook")}
        </button>
      </div>
      {user?.role === "admin" && <p className="hint">{t("library.adminHint")}</p>}
      <p className="hint">{t("library.metadataHint")}</p>
      <div className="book-grid">
        {(entries ?? Array.from({ length: 8 }, () => null)).map((entry, i) =>
          entry ? (
            <BookCard key={entry.book.id} entry={entry} />
          ) : (
            <BookCardSkeleton key={`sk-${i}`} />
          ),
        )}
      </div>
      {entries !== null && entries.length === 0 && (
        <div className="empty-state">
          <IconBook size={40} className="empty-state-icon" />
          <p className="strong">{t("library.emptyTitle")}</p>
          <p className="hint">{t("library.empty")}</p>
        </div>
      )}

      <AddBookDialog
        open={addOpen}
        onClose={() => setAddOpen(false)}
        instances={instances}
        onAdded={() => {
          setQ("");
          void load("");
        }}
      />
    </div>
  );
}