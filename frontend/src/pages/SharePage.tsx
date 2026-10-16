import { useEffect, useState } from "react";
import { Link, useMatch, useParams, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { BookHero } from "../components/BookHero";
import { IconPin } from "../components/icons";
import { Reader } from "../components/Reader";
import type { Chapter, Position, ShareBookResponse, ShareView } from "../types";

/** Public book detail and reader; never writes reading progress. */
export function SharePage() {
  const { token = "" } = useParams();
  return <PublicShare key={token} token={token} />;
}

function InvalidShare({ message }: { message: string }) {
  const { t } = useTranslation();
  return <div className="card"><h1>{t("share.invalidTitle")}</h1><p role="alert">{message}</p></div>;
}

function PublicShare({ token }: { token: string }) {
  const { t } = useTranslation();
  const [search] = useSearchParams();
  const [view, setView] = useState<ShareView | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    api<ShareView>(`/shares/${token}`).then(
      (result) => { if (!cancelled) setView(result); },
      (err: unknown) => { if (!cancelled) setError(err instanceof Error ? err.message : t("share.invalid")); },
    );
    return () => { cancelled = true; };
  }, [token, t]);
  if (error) return <InvalidShare message={error} />;
  if (!view) return <div className="page-loading">{t("common.loading")}</div>;
  const fileId = search.get("file_id") ?? view.file.id;
  if (!(view.files ?? [view.file]).some((file) => file.id === fileId)) {
    return <InvalidShare message={t("share.unsharedFile")} />;
  }
  // Changing sources must drop both pending requests and the reader's chapter cache.
  return <SharedContent key={fileId} token={token} fileId={fileId} view={view} />;
}

function SharedContent({ token, fileId, view }: { token: string; fileId: string; view: ShareView }) {
  const { t } = useTranslation();
  const reading = Boolean(useMatch("/share/:token/read"));
  const [search] = useSearchParams();
  const [book, setBook] = useState<ShareBookResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const fileQuery = fileId === view.file.id ? "" : `?file_id=${encodeURIComponent(fileId)}`;
  useEffect(() => {
    let cancelled = false;
    api<ShareBookResponse>(`/shares/${token}/book${fileQuery}`).then(
      (result) => { if (!cancelled) setBook(result); },
      (err: unknown) => { if (!cancelled) setError(err instanceof Error ? err.message : t("share.invalid")); },
    );
    return () => { cancelled = true; };
  }, [token, fileQuery, t]);
  if (error) return <InvalidShare message={error} />;
  if (!book) return <div className="page-loading">{t("common.loading")}</div>;

  const files = view.files ?? [view.file];
  const overviewHref = `/share/${token}${fileQuery}`;
  const readHref = `/share/${token}/read${fileQuery}`;
  const chapterHref = (idx: number) => `${readHref}${fileQuery ? "&" : "?"}chapter=${idx}`;
  const firstChapter = book.chapters.find((chapter) => chapter.linear) ?? book.chapters[0];
  const session = fileId === view.file.id ? view.session : null;
  const requested = search.has("chapter") ? Number(search.get("chapter")) : null;
  const selectedChapter = requested !== null && Number.isInteger(requested)
    ? book.chapters.find((chapter) => chapter.idx === requested) : undefined;
  const initialPosition: Position = selectedChapter
    ? { chapter_idx: selectedChapter.idx, in_chapter: { fraction: 0 } }
    : session?.position ?? { chapter_idx: firstChapter?.idx ?? 0, in_chapter: { fraction: 0 } };

  if (reading && firstChapter) {
    return (
      <div className="reader-page">
        <Reader key={`${token}:${fileId}:${search.toString()}`} title={book.book.title} chapters={book.chapters} toc={book.toc}
          initialPosition={initialPosition} backHref={overviewHref} shareLink={readHref}
          loadChapter={(idx) => api<Chapter>(`/shares/${token}/chapters/${idx}${fileQuery}`)} readOnly />
      </div>
    );
  }
  return (
    <div className="book-detail">
      <BookHero book={book.book} fileCount={files.length} actions={firstChapter && <>
        <Link className="btn primary" to={chapterHref(firstChapter.idx)}>{t("book.startReading")}</Link>
        {session && <Link className="btn" to={readHref}>{t("share.readFromProgress")}</Link>}
      </>} />
      {session && (
        <div className="card share-snapshot">
          <h2><IconPin size={16} /> {t("share.progressOf", { owner: session.owner_username, label: session.label })}</h2>
          <p>{t("share.readTo")} <strong>{session.percent}%</strong>
            <span className="hint">{t("share.updatedAt", { time: new Date(session.updated_at).toLocaleString("zh-CN") })}</span>
          </p>
        </div>
      )}
      <section className="detail-section">
        <h2>{t("book.filesTitle", { count: files.length })}</h2>
        {files.map((file) => {
          const label = file.label || (file.source === "local" ? file.format : file.source);
          return (
            <div className="file-card" key={file.id}>
              <div className="file-head">
                <div className="file-title">
                  <span className="fmt-badge">{file.format}</span>
                  <span className="file-label">{label}</span>
                  {file.source && <span className="tag">{file.source}</span>}
                  <span className="hint">{t("common.chaptersTotal", { count: file.chapter_count })}</span>
                </div>
                <Link className={file.id === fileId ? "btn sm primary" : "btn sm"}
                  aria-current={file.id === fileId ? "true" : undefined}
                  aria-label={t("share.viewSource", { source: label })}
                  to={`/share/${token}${file.id === view.file.id ? "" : `?file_id=${encodeURIComponent(file.id)}`}`}>
                  {t(file.id === fileId ? "share.currentSource" : "share.viewContents")}
                </Link>
              </div>
            </div>
          );
        })}
      </section>
      <section className="detail-section">
        <h2>{t("reader.toc")}</h2>
        {!firstChapter ? <p className="hint">{t("share.noChapters")}</p> : (
          <nav aria-label={t("reader.toc")}><ol className="share-chapters">
            {book.chapters.map((chapter) => <li key={chapter.idx}><Link to={chapterHref(chapter.idx)}>{chapter.title}</Link></li>)}
          </ol></nav>
        )}
      </section>
    </div>
  );
}
