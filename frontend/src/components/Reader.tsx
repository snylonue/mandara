import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Chapter, ChapterMeta, Position } from "../types";

interface ReaderProps {
  title: string;
  chapters: ChapterMeta[];
  /** Fetch a chapter by index (can be async, e.g. plugins materialize lazily). */
  loadChapter: (idx: number) => Promise<Chapter>;
  /** Initial position to restore. */
  initialPosition?: Position;
  /** Called (debounced) with the reader's current scroll fraction. */
  onProgress?: (fraction: number) => void;
  /** Disable prev/next persistence and session UI (public share view). */
  readOnly?: boolean;
}

export function Reader({
  title,
  chapters,
  loadChapter,
  initialPosition,
  onProgress,
  readOnly,
}: ReaderProps) {
  const { t } = useTranslation();
  const [idx, setIdx] = useState(() =>
    Math.min(initialPosition?.chapter_idx ?? 0, Math.max(chapters.length - 1, 0)),
  );
  const [chapter, setChapter] = useState<Chapter | null>(null);
  const [loading, setLoading] = useState(false);
  const [fraction, setFraction] = useState(initialPosition?.fraction ?? 0);
  const loadedRef = useRef<Map<number, Chapter>>(new Map());

  // Load chapter content (cached), and restore scroll by fraction once.
  useEffect(() => {
    const cached = loadedRef.current.get(idx);
    if (cached) {
      setChapter(cached);
      restoreScroll(fraction);
      return;
    }
    setLoading(true);
    void loadChapter(idx).then((c) => {
      loadedRef.current.set(idx, c);
      setChapter(c);
      setLoading(false);
      restoreScroll(fraction);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idx]);

  function restoreScroll(frac: number) {
    if (!frac) {
      window.scrollTo(0, 0);
      return;
    }
    // wait a tick for the content to render
    requestAnimationFrame(() => {
      const max = document.documentElement.scrollHeight - window.innerHeight;
      window.scrollTo(0, Math.round(max * frac));
    });
  }

  const goto = useCallback(
    (next: number) => {
      const clamped = Math.max(0, Math.min(next, chapters.length - 1));
      if (clamped === idx) return;
      setIdx(clamped);
      setFraction(0);
    },
    [chapters.length, idx],
  );

  // Debounced scroll tracking → progress callback.
  const debounceRef = useRef<number | undefined>(undefined);
  useEffect(() => {
    function onScroll() {
      const max = document.documentElement.scrollHeight - window.innerHeight;
      const f = max > 0 ? Math.min(1, Math.max(0, window.scrollY / max)) : 0;
      setFraction(f);
      if (!onProgress || readOnly) return;
      window.clearTimeout(debounceRef.current);
      debounceRef.current = window.setTimeout(() => onProgress(f), 600);
    }
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      window.clearTimeout(debounceRef.current);
    };
  }, [onProgress, readOnly]);

  useEffect(() => {
    return () => window.scrollTo(0, 0);
  }, []);

  const cur = chapters.find((c) => c.idx === idx);
  const progress = chapters.length > 0 ? ((idx + fraction) / chapters.length) * 100 : 0;

  return (
    <div className="reader">
      <div className="reader-top">
        <div className="reader-title">
          {title} <span className="reader-sub">{cur?.title ?? ""}</span>
        </div>
        <div className="reader-controls">
          <button onClick={() => goto(idx - 1)} disabled={idx <= 0}>
            {t("reader.prev")}
          </button>
          <select
            value={idx}
            onChange={(e) => goto(Number(e.target.value))}
            aria-label={t("reader.chapterSelect")}
          >
            {chapters.map((c) => (
              <option key={c.idx} value={c.idx}>
                {c.title}
              </option>
            ))}
          </select>
          <button onClick={() => goto(idx + 1)} disabled={idx >= chapters.length - 1}>
            {t("reader.next")}
          </button>
        </div>
      </div>
      <div className="progress-bar">
        <div className="progress-fill" style={{ width: `${progress}%` }} />
      </div>
      <article className="reader-content">
        {loading && <p className="hint">{t("common.loading")}</p>}
        {chapter &&
          (chapter.format === "html" ? (
            // Sanitized by the server (epub XHTML whitelist).
            <div
              className="epub-content"
              dangerouslySetInnerHTML={{ __html: chapter.content }}
            />
          ) : (
            <p className="chapter-text">{chapter.content}</p>
          ))}
      </article>
      <div className="reader-bottom">
        <button onClick={() => goto(idx - 1)} disabled={idx <= 0 || readOnly}>
          {t("reader.prev")}
        </button>
        <span className="hint">
          {t("reader.chapterOf", {
            current: idx + 1,
            total: chapters.length,
            percent: Math.round(progress),
          })}
        </span>
        <button onClick={() => goto(idx + 1)} disabled={idx >= chapters.length - 1 || readOnly}>
          {t("reader.next")}
        </button>
      </div>
    </div>
  );
}