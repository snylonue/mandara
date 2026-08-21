import { useCallback, useEffect, useRef, useState, type ChangeEvent } from "react";
import { Link } from "react-router-dom";
import { api } from "../api";
import { useAuth } from "../auth";
import type { BookDetail, BookListEntry, Visibility } from "../types";

function BookCard({ entry }: { entry: BookListEntry }) {
  const { book, files } = entry;
  const authors = book.authors.length ? book.authors.join(" / ") : "佚名";
  return (
    <Link className="book-card" to={`/book/${book.id}`}>
      <div className="book-title">{book.title}</div>
      <div className="book-meta">{authors} · {files.reduce((n, f) => n + f.chapter_count, 0)} 章</div>
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
      setError(err instanceof Error ? err.message : "failed to load");
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
      setError(err instanceof Error ? err.message : "upload failed");
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
        <select
          value={visibility}
          onChange={(e) => setVisibility(e.target.value as Visibility)}
          aria-label="可见性"
        >
          <option value="private">隐藏（仅自己可见）</option>
          <option value="public">公开（所有登录用户可见）</option>
        </select>
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
      {user?.role === "admin" && <p className="hint">管理员视图：可见全部书籍（包含隐私文件）。</p>}
      <p className="hint">
        一个条目可关联多个文件（不同格式/版本）：先在此上传新书，
        再到书籍详情页添加更多文件。
      </p>
      <div className="book-grid">
        {entries.map((entry) => (
          <BookCard key={entry.book.id} entry={entry} />
        ))}
      </div>
      {!entries.length && !error && <p className="hint">书架空空如也，上传一本 epub/txt 试试。</p>}
    </div>
  );
}