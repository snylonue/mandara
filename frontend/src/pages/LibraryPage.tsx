import { useCallback, useEffect, useRef, useState, type ChangeEvent } from "react";
import { Link } from "react-router-dom";
import { api } from "../api";
import { useAuth } from "../auth";
import type { BookMeta } from "../types";

function BookCard({ book }: { book: BookMeta }) {
  const authors = book.authors.length ? book.authors.join(" / ") : "佚名";
  return (
    <Link className="book-card" to={`/book/${book.id}`}>
      <div className="book-title">{book.title}</div>
      <div className="book-meta">
        {authors} · {book.chapter_count} 章
      </div>
      <div className="book-tags">
        {book.source !== "local" && <span className="tag">插件来源</span>}
        {book.visibility === "public" && <span className="tag">公开</span>}
      </div>
    </Link>
  );
}

export function LibraryPage() {
  const { user } = useAuth();
  const [books, setBooks] = useState<BookMeta[]>([]);
  const [q, setQ] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  const load = useCallback(async (query: string) => {
    try {
      const list = await api<BookMeta[]>(`/books?q=${encodeURIComponent(query)}`);
      setBooks(list);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "加载失败");
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
      await api<BookMeta>("/books", { method: "POST", body: form });
      setQ("");
      await load("");
    } catch (err) {
      setError(err instanceof Error ? err.message : "上传失败");
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
          placeholder="搜索书名…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <button className="primary" onClick={() => fileRef.current?.click()} disabled={uploading}>
          {uploading ? "上传中…" : "上传书籍 (epub/txt)"}
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
      {user?.role === "admin" && (
        <p className="hint">
          管理员视图：可见全部书籍（包含隐私书籍）。
        </p>
      )}
      <div className="book-grid">
        {books.map((b) => (
          <BookCard key={b.id} book={b} />
        ))}
      </div>
      {!books.length && !error && <p className="hint">书架空空如也，上传一本 epub/txt 试试。</p>}
    </div>
  );
}