import { useEffect, useState } from "react";
import { Link, useMatch, useParams, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { IconPin } from "../components/icons";
import { Reader } from "../components/Reader";
import type { Chapter, Position, ShareBookResponse, ShareView } from "../types";

/// Public share view: no login required, no progress saving.
export function SharePage() {
  const { token = "" } = useParams();
  // A different token must never reuse the previous book or reader cache.
  return <PublicShare key={token} token={token} />;
}

function PublicShare({ token }: { token: string }) {
  const { t } = useTranslation();
  const reading = Boolean(useMatch("/share/:token/read"));
  const [search] = useSearchParams();
  const [data, setData] = useState<{ view: ShareView; book: ShareBookResponse } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [view, book] = await Promise.all([
          api<ShareView>(`/shares/${token}`),
          api<ShareBookResponse>(`/shares/${token}/book`),
        ]);
        if (!cancelled) setData({ view, book });
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : t("share.invalid"));
      }
    })();
    return () => { cancelled = true; };
  }, [token, t]);

  if (error) {
    return (
      <div className="card">
        <h1>{t("share.invalidTitle")}</h1>
        <p role="alert">{error}</p>
      </div>
    );
  }
  if (!data) return <div className="page-loading">{t("common.loading")}</div>;

  const { view, book } = data;
  const overviewHref = `/share/${token}`;
  const readHref = `${overviewHref}/read`;
  const requestedChapter = search.has("chapter") ? Number(search.get("chapter")) : null;
  const selectedChapter = requestedChapter !== null && Number.isInteger(requestedChapter)
    ? book.chapters.find((c) => c.idx === requestedChapter)
    : undefined;
  const initialPosition: Position = selectedChapter
    ? { chapter_idx: selectedChapter.idx, in_chapter: { fraction: 0 } }
    : view.session?.position ?? { chapter_idx: 0, in_chapter: { fraction: 0 } };

  if (reading && book.chapters.length > 0) {
    return (
      <div className="reader-page">
        <Reader
          key={`${token}:${search.toString()}`}
          title={book.book.title}
          chapters={book.chapters}
          toc={book.toc}
          initialPosition={initialPosition}
          backHref={overviewHref}
          loadChapter={(idx) => api<Chapter>(`/shares/${token}/chapters/${idx}`)}
          readOnly
        />
      </div>
    );
  }

  return (
    <div className="share-book">
      <section className="detail-section">
        <h1>{book.book.title}</h1>
        <p className="detail-meta">{book.book.authors.join(" / ") || t("common.anonymous")}</p>
        {book.book.description && <p className="share-description">{book.book.description}</p>}
        <p className="detail-meta">
          <span className="fmt-badge">{book.file.format}</span>
          {book.file.label && <span>{book.file.label}</span>}
          <span>{t("common.chaptersTotal", { count: book.chapters.length })}</span>
        </p>
        {book.chapters.length > 0 && (
          <div className="detail-actions">
            <Link className="btn primary" to={`${readHref}?chapter=0`}>{t("book.startReading")}</Link>
            {view.session && <Link className="btn" to={readHref}>{t("share.readFromProgress")}</Link>}
          </div>
        )}
      </section>
      {view.session && (
        <div className="card share-snapshot">
          <h2>
            <IconPin size={16} />{" "}
            {t("share.progressOf", { owner: view.session.owner_username, label: view.session.label })}
          </h2>
          <p>
            {t("share.readTo")}{" "}<strong>{view.session.percent}%</strong>
            <span className="hint">
              {t("share.updatedAt", { time: new Date(view.session.updated_at).toLocaleString("zh-CN") })}
            </span>
          </p>
        </div>
      )}
      <section className="detail-section">
        <h2>{t("reader.toc")}</h2>
        {book.chapters.length === 0 ? <p className="hint">{t("share.noChapters")}</p> : (
          <nav aria-label={t("reader.toc")}>
            <ol className="share-chapters">
              {book.chapters.map((chapter) => (
                <li key={chapter.idx}>
                  <Link to={`${readHref}?chapter=${chapter.idx}`}>{chapter.title}</Link>
                </li>
              ))}
            </ol>
          </nav>
        )}
      </section>
    </div>
  );
}
