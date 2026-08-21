import { useCallback, useEffect, useState, type ChangeEvent } from "react";
import { Link, useParams } from "react-router-dom";
import { api } from "../api";
import {
  type BookDetail,
  type FileMeta,
  type ReadingSession,
  type SessionsResponse,
  type ShareInfo,
  type Visibility,
} from "../types";

const fmtTime = (s: string) => new Date(s).toLocaleString("zh-CN", { hour12: false });
const fmtPercent = (s: ReadingSession) => `${Math.round(s.position.fraction * 100)}%`;

/// One file row: actions per file (read, visibility toggle, share, delete)
/// plus its sessions.
function FileSection({
  file,
  onChanged,
  onShare,
  setError,
}: {
  file: FileMeta;
  onChanged: () => Promise<void>;
  onShare: (fileId: string, kind: "book" | "session", sessionId?: string) => Promise<void>;
  setError: (msg: string | null) => void;
}) {
  const [sessions, setSessions] = useState<ReadingSession[]>([]);

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
      setError(err instanceof Error ? err.message : "操作失败");
    }
  }

  async function deleteFile() {
    if (!window.confirm(`删除文件「${file.label || file.format}」？该文件的所有章节与会话将被删除。`)) return;
    try {
      await api(`/files/${file.id}`, { method: "DELETE" });
      await onChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : "删除失败");
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
          {file.source !== "local" && <span className="tag">plugin</span>}
          <span className={`tag ${file.visibility === "public" ? "tag-public" : ""}`}>
            {file.visibility === "public" ? "公开" : "隐藏"}
          </span>
        </div>
        <div className="row-actions">
          {readable && (
            <Link className="link-btn" to={`/read/${file.id}`}>
              阅读
            </Link>
          )}
          {file.source === "local" && (
            <>
              <button className="link-btn" onClick={() => void toggleVisibility()}>
                {file.visibility === "public" ? "设为隐藏" : "设为公开"}
              </button>
              <button
                className="link-btn"
                onClick={() => void onShare(file.id, "book")}
              >
                分享
              </button>
              <button className="link-btn danger" onClick={() => void deleteFile()}>
                删除
              </button>
            </>
          )}
        </div>
      </div>
      <div className="file-meta hint">
        {file.chapter_count} 章 · 上传于 {fmtTime(file.created_at)}
      </div>

      {sessions.length > 0 && (
        <table className="sessions">
          <thead>
            <tr>
              <th>会话</th>
              <th>进度</th>
              <th>更新时间</th>
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
                  <td>第 {s.position.chapter_idx + 1} 章 · {fmtPercent(s)}</td>
                  <td className="hint">{fmtTime(s.updated_at)}</td>
                  <td className="row-actions">
                    <button
                      className="link-btn"
                      onClick={() => void onShare(file.id, "session", s.id)}
                    >
                      分享进度
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
  const [detail, setDetail] = useState<BookDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [share, setShare] = useState<ShareInfo | null>(null);
  const [attachLabel, setAttachLabel] = useState("");
  const [attachVisibility, setAttachVisibility] = useState<Visibility>("public");
  const [attaching, setAttaching] = useState(false);
  const attachRef = { current: null as HTMLInputElement | null };

  const load = useCallback(async () => {
    try {
      const d = await api<BookDetail>(`/books/${id}`);
      setDetail(d);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "加载失败");
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  async function attach(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setAttaching(true);
    setError(null);
    try {
      const form = new FormData();
      form.append("file", file);
      form.append("visibility", attachVisibility);
      if (attachLabel.trim()) form.append("label", attachLabel.trim());
      await api<FileMeta>(`/books/${id}/files`, { method: "POST", body: form });
      setAttachLabel("");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "上传失败");
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
      setError(err instanceof Error ? err.message : "创建分享失败");
    }
  }

  if (error) return <div className="error">{error}</div>;
  if (!detail) return <div className="page-loading">加载中…</div>;
  const { book, files } = detail;

  return (
    <div className="book-detail">
      <Link to="/" className="link-btn">
        ← 书架
      </Link>
      <h1>{book.title}</h1>
      <p className="hint">
        {book.authors.join(" / ") || "佚名"} · {files.length} 个文件版本
      </p>
      {book.description && <p className="description">{book.description}</p>}

      <section className="card">
        <h2>文件版本（{files.length}）</h2>
        <p className="hint">
          同一本书可有多个文件（不同格式/版本），各文件独立设置公开/隐藏，阅读进度以文件为单位。
        </p>
        {files.map((f) => (
          <FileSection
            key={f.id}
            file={f}
            onChanged={load}
            onShare={makeShare}
            setError={setError}
          />
        ))}
      </section>

      <section className="card">
        <h2>添加文件到本书</h2>
        <div className="inline-form">
          <input
            placeholder="备注（如：epub 精校版）"
            value={attachLabel}
            onChange={(e) => setAttachLabel(e.target.value)}
          />
          <select
            value={attachVisibility}
            onChange={(e) => setAttachVisibility(e.target.value as Visibility)}
          >
            <option value="private">隐藏</option>
            <option value="public">公开</option>
          </select>
          <button className="primary" disabled={attaching} onClick={() => attachRef.current?.click()}>
            {attaching ? "上传中…" : "上传文件 (epub/txt)"}
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
        <h2>分享</h2>
        <div className="row">
          <button
            className="primary"
            onClick={() => files[0] && void makeShare(files[0].id, "book")}
          >
            生成本书分享链接（匿名可读）
          </button>
          <span className="hint">公开文件可直接被任意用户分享；私有文件仅所有者/管理员可分享。</span>
        </div>
        {share && (
          <div className="share-box">
            <span className="tag">{share.kind === "book" ? "书籍" : "进度"}分享</span>
            <code>{share.url}</code>
            <button
              className="link-btn"
              onClick={() => void navigator.clipboard?.writeText(`${location.origin}${share.url}`)}
            >
              复制
            </button>
          </div>
        )}
      </section>
    </div>
  );
}