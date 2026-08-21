import { useCallback, useEffect, useRef, useState, type ChangeEvent } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useAuth } from "../auth";
import type { BookDetail, BookListEntry, Visibility } from "../types";

function BookCard({ entry }: { entry: BookListEntry }) {
  const { t } = useTranslation();
  const { book, files } = entry;
  const authors = book.authors.length ? book.authors.join(" / ") : t("common.anonymous");
  return (
    <Link className="book-card" to={`/book/${book.id}`}>
      <div className="book-title">{book.title}</div>
      <div className="book-meta">{authors} · {files.reduce((n, f) => n + f.chapter_count, 0)} {t("common.chapters")}</div>
      <div className="book-tags">
        {files.map((f) => (
          <span key={f.id} className="tag">
            {f.format}
            {f.source !== "local" ? "·plugin" : ""}
            {f.visibility === "public" ? "·public" : "·private"}
          </span>
        ))}
      </div>
    </Link>
  );
}

export function LibraryPage() {
  const { user } = useAuth();
  const { t } = useTranslation();
  const [entries, setEntries] = useState<BookListEntry[]>([]);
  const [q, setQ] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const [visibility, setVisibility] = useState<Visibility>("private");
  const fileRef = useRef<HTMLInputElement>(null);

  const load = useCallback(async (query: string) => {
    try {
      const list = await api<BookListEntry[]>(`/books?q=${encodeURIComponent(query)}`);
      setEntries(list);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : t("common.failed"));
    }
  }, []);

  useEffect(() => {
    void load(q);
  }, [load, q]);

  async function upload(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setUploading(true);
    setError(null);
    try {
      const form = new FormData();
      form.append("file", file);
      form.append("visibility", visibility);
      await api<BookDetail>("/books", { method: "POST", body: form });
      setQ("");
      await load("");
    } catch (err) {
      setError(err instanceof Error ? err.message : t("library.uploadFailed"));
    } finally {
      setUploading(false);
      if (fileRef.current) fileRef.current.value = "";
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
        <select
          value={visibility}
          onChange={(e) => setVisibility(e.target.value as Visibility)}
          aria-label={t("library.visibilityLabel")}
        >
          <option value="private">{t("library.visibilityPrivate")}</option>
          <option value="public">{t("library.visibilityPublic")}</option>
        </select>
        <button className="primary" onClick={() => fileRef.current?.click()} disabled={uploading}>
          {uploading ? t("library.uploading") : t("library.upload")}
        </button>
        <input
          ref={fileRef}
          type="file"
          accept=".epub,.txt,.text"
          hidden
          onChange={upload}
        />
      </div>
      {error && <div className="error">{error}</div>}
      {user?.role === "admin" && <p className="hint">{t("library.adminHint")}</p>}
      <p className="hint">{t("library.metadataHint")}</p>
      <div className="book-grid">
        {entries.map((entry) => (
          <BookCard key={entry.book.id} entry={entry} />
        ))}
      </div>
      {!entries.length && !error && <p className="hint">{t("library.empty")}</p>}
    </div>
  );
}