import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Chapter, ChapterMeta, Position, TocNode } from "../types";

interface ReaderProps {
  title: string;
  chapters: ChapterMeta[];
  /** Fetch a chapter by index (can be async, e.g. plugins materialize lazily). */
  loadChapter: (idx: number) => Promise<Chapter>;
  /** Initial position to restore. */
  initialPosition?: Position;
  /** Called (debounced) with the reader's current scroll fraction. */
  onProgress?: (fraction: number) => void;
  /** Hierarchical table of contents (per the ebook's nav/NCX). */
  toc?: TocNode[];
  /** Disable prev/next persistence and session UI (public share view). */
  readOnly?: boolean;
}

export function Reader({
  title,
  chapters,
  toc,
  loadChapter,
  initialPosition,
  onProgress,
  readOnly,
}: ReaderProps) {
  const { t } = useTranslation();
  const [tocOpen, setTocOpen] = useState(false);
  const [idx, setIdx] = useState(() =>
    Math.min(initialPosition?.chapter_idx ?? 0, Math.max(chapters.length - 1, 0)),
  );
  const [chapter, setChapter] = useState<Chapter | null>(null);
  const [loading, setLoading] = useState(false);
  const [fraction, setFraction] = useState(initialPosition?.fraction ?? 0);
  const loadedRef = useRef<Map<number, Chapter>>(new Map());
  const pendingFragRef = useRef<string | null>(null);

  function scrollToFragId(frag: string | null) {
    // The chapter content carries the original element ids, so a TOC
    // fragment entry (`file.html#section`) can scroll into its section.
    requestAnimationFrame(() => {
      const el = frag ? document.getElementById(frag) : null;
      if (el) el.scrollIntoView({ block: "start" });
    });
  }

  // Load chapter content (cached), and restore scroll by fraction once.
  useEffect(() => {
    const cached = loadedRef.current.get(idx);
    const frag = pendingFragRef.current;
    pendingFragRef.current = null;
    if (cached) {
      setChapter(cached);
      restoreScroll(fraction);
      scrollToFragId(frag);
      return;
    }
    setLoading(true);
    void loadChapter(idx).then((c) => {
      loadedRef.current.set(idx, c);
      setChapter(c);
      setLoading(false);
      restoreScroll(fraction);
      scrollToFragId(frag);
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
    (next: number, frag?: string) => {
      const clamped = Math.max(0, Math.min(next, chapters.length - 1));
      if (clamped === idx) {
        // Same chapter: the content is already rendered — scroll directly.
        if (frag) scrollToFragId(frag);
        return;
      }
      setIdx(clamped);
      setFraction(0);
      pendingFragRef.current = frag ?? null;
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
          <button
            className={tocOpen ? "active" : undefined}
            onClick={() => setTocOpen((v) => !v)}
            aria-expanded={tocOpen}
          >
            {t("reader.toc")}
          </button>
          <button onClick={() => goto(idx - 1)} disabled={idx <= 0}>
            {t("reader.prev")}
          </button>
          <button onClick={() => goto(idx + 1)} disabled={idx >= chapters.length - 1}>
            {t("reader.next")}
          </button>
        </div>
      </div>
      {tocOpen && (toc ?? []).length > 0 && (
        <nav className="toc-panel" aria-label={t("reader.toc")}>
          <TocTree nodes={toc!} current={idx} onSelect={goto} onClose={() => setTocOpen(false)} />
        </nav>
      )}
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

/// Recursive renderer for the hierarchical TOC.
/// Group nodes (`idx == null`) are collapsible; leaf nodes jump to their
/// chapter. All groups start expanded so the full hierarchy is visible
/// (click a group to collapse it).
function TocTree({
  nodes,
  current,
  onSelect,
  onClose,
  depth = 0,
}: {
  nodes: TocNode[];
  current: number;
  onSelect: (idx: number, frag?: string) => void;
  onClose: () => void;
  depth?: number;
}) {
  return (
    <ul className="toc-list">
      {nodes.map((n, i) => {
        const key = `${n.idx ?? "group"}-${i}`;
        const leaf = n.children.length === 0;
        const sel = () => {
          if (n.idx !== null) {
            onSelect(n.idx, n.frag ?? undefined);
            onClose();
          }
        };
        if (leaf) {
          return (
            <li key={key} className={depth > 0 ? "toc-nested" : undefined}>
              <button
                className={`toc-link${current === n.idx ? " current" : ""}`}
                onClick={sel}
              >
                {n.title}
              </button>
            </li>
          );
        }
        return (
          <li key={key} className={depth > 0 ? "toc-nested" : undefined}>
            <details open>
              <summary>
                {n.idx !== null ? (
                  <button
                    className={`toc-link${current === n.idx ? " current" : ""}`}
                    onClick={sel}
                  >
                    {n.title}
                  </button>
                ) : (
                  <span className="toc-group">{n.title}</span>
                )}
              </summary>
              <TocTree
                nodes={n.children}
                current={current}
                onSelect={onSelect}
                onClose={onClose}
                depth={depth + 1}
              />
            </details>
          </li>
        );
      })}
    </ul>
  );
}