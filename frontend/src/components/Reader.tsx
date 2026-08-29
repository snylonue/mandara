import { useCallback, useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { IconArrowLeft, IconChevronLeft, IconChevronRight } from "./icons";
import {
  ReaderSettingsPanel,
  useReaderSettings,
} from "./ReaderSettings";
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
  /** Where the top-bar 返回 points (book detail page); hidden when absent. */
  backHref?: string;
}

export function Reader({
  title,
  chapters,
  toc,
  loadChapter,
  initialPosition,
  onProgress,
  readOnly,
  backHref,
}: ReaderProps) {
  const { t } = useTranslation();
  const [settings, setSettings] = useReaderSettings();
  const [tocOpen, setTocOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
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
  const lastFractionRef = useRef(0);
  useEffect(() => {
    function onScroll() {
      const max = document.documentElement.scrollHeight - window.innerHeight;
      const f = max > 0 ? Math.min(1, Math.max(0, window.scrollY / max)) : 0;
      setFraction(f);
      lastFractionRef.current = f;
      if (!onProgress || readOnly) return;
      window.clearTimeout(debounceRef.current);
      debounceRef.current = window.setTimeout(() => onProgress(f), 600);
    }
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      window.clearTimeout(debounceRef.current);
      // Flush the pending fraction on unmount (leaving the reader must
      // not lose the last 600 ms of scroll).
      if (onProgress && !readOnly && lastFractionRef.current > 0) {
        onProgress(lastFractionRef.current);
      }
    };
  }, [onProgress, readOnly]);

  useEffect(() => {
    return () => window.scrollTo(0, 0);
  }, []);

  const cur = chapters.find((c) => c.idx === idx);
  const next = chapters.find((c) => c.idx === idx + 1);
  const progress = chapters.length > 0 ? ((idx + fraction) / chapters.length) * 100 : 0;

  // Keep the highlighted chapter visible when the drawer opens or the
  // chapter changes while it is open.
  useEffect(() => {
    if (!tocOpen) return;
    requestAnimationFrame(() => {
      document
        .querySelector(".toc-drawer .toc-link.current")
        ?.scrollIntoView({ block: "center" });
    });
  }, [tocOpen, idx]);

  return (
    <div className="reader" data-theme={settings.theme}>
      {/* fixed top bar: 目录 / 返回 · book title · chapter title · percent · 设置 */}
      <header className="reader-topbar">
        <div className="reader-topbar-side">
          {(toc ?? []).length > 0 && (
            <button className="sm" onClick={() => setTocOpen(true)}>
              {t("reader.toc")}
            </button>
          )}
          {backHref && (
            <Link className="link-btn" to={backHref}>
              <IconArrowLeft size={14} /> {t("common.backToDetail")}
            </Link>
          )}
        </div>
        <div className="reader-topbar-center">
          <span className="reader-topbar-book">{title}</span>
          <span className="reader-topbar-chapter">
            {cur?.title ?? ""}
            {!readOnly && ` · ${Math.round(progress)}%`}
          </span>
        </div>
        <div className="reader-topbar-side reader-topbar-end">
          <button
            className={`sm${settingsOpen ? " active" : ""}`}
            onClick={() => setSettingsOpen((v) => !v)}
            aria-expanded={settingsOpen}
          >
            Aa
          </button>
        </div>
        {settingsOpen && (
          <ReaderSettingsPanel
            settings={settings}
            onChange={setSettings}
            onClose={() => setSettingsOpen(false)}
          />
        )}
      </header>

      {/* left slide-in table of contents */}
      {tocOpen && (
        <>
          <div className="toc-drawer-backdrop" onClick={() => setTocOpen(false)} />
          <nav className="toc-drawer" aria-label={t("reader.toc")}>
            <div className="toc-drawer-head">
              <h2>{t("reader.toc")}</h2>
              <button className="ghost sm" onClick={() => setTocOpen(false)} aria-label={t("common.close")}>
                ✕
              </button>
            </div>
            <div className="toc-drawer-body">
              <TocTree nodes={toc!} current={idx} onSelect={goto} onClose={() => setTocOpen(false)} />
            </div>
          </nav>
        </>
      )}

      <div className="progress-bar">
        <div className="progress-fill" style={{ width: `${progress}%` }} />
      </div>

      <article
        className={`reader-body font-${settings.font}`}
        style={{
          fontSize: `${settings.fontSize}px`,
          lineHeight: settings.lineHeight,
          maxWidth: `${settings.widthEm}em`,
        }}
      >
        {loading && <p className="hint">{t("common.loading")}</p>}
        {chapter &&
          // Canonical sanitized HTML for every source (storage
          // unification): epub chapters, txt paragraphs, plugin text with
          // illustration conventions expanded at the ingest boundary.
          <div
            className="epub-content"
            dangerouslySetInnerHTML={{ __html: chapter.content }}
          />}
      </article>

      {/* single prev/next control set — large buttons with chapter preview */}
      <div className="reader-bottom">
        <button
          className="reader-nav-btn"
          onClick={() => goto(idx - 1)}
          disabled={idx <= 0}
        >
          <small>
            <IconChevronLeft size={13} /> {t("reader.prev")}
          </small>
          <span>{chapters.find((c) => c.idx === idx - 1)?.title ?? ""}</span>
        </button>
        <button
          className="reader-nav-btn next"
          onClick={() => goto(idx + 1)}
          disabled={idx >= chapters.length - 1 || readOnly}
        >
          <small>
            {t("reader.next")} <IconChevronRight size={13} />
          </small>
          <span>{next?.title ?? ""}</span>
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
