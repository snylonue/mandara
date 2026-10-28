import { memo, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { IconArrowLeft, IconChevronLeft, IconChevronRight, IconList, IconType, IconCamera } from "./icons";
import { EpubDocument, type EpubDocumentHandle } from "./EpubDocument";
import { ReaderScreenshotDialog, type ReaderExcerpt } from "./ReaderScreenshotDialog";
import { ReaderImageDialog } from "./ReaderImageDialog";
import { readerImageAt, type ReaderImage } from "../readerImage";
import { selectedChapterText, selectedTextContent, type ScreenshotSelection } from "../readerScreenshot";
import {
  ReaderSettingsPanel,
  useReaderSettings,
} from "./ReaderSettings";
import type { Chapter, ChapterMeta, Position, TocNode } from "../types";
import { ApiError } from "../api";

// Keep native selections intact when reader controls or scroll progress rerender.
const TextChapterContent = memo(function TextChapterContent({ content }: { content: string }) {
  return <div className="epub-content" dangerouslySetInnerHTML={{ __html: content }} />;
});

interface ReaderProps {
  title: string;
  chapters: ChapterMeta[];
  /** Read a stored chapter by index; the request does not fetch from its source. */
  loadChapter: (idx: number) => Promise<Chapter>;
  /** Explicit user retry may fill a missing body; without it, retries only reread storage. */
  retryChapter?: (idx: number) => Promise<Chapter>;
  /** Initial position to restore. */
  initialPosition?: Position;
  /** Called (debounced) with the reader's current scroll fraction. */
  onProgress?: (fraction: number, chapterIdx: number) => void;
  /** Hierarchical table of contents (per the ebook's nav/NCX). */
  toc?: TocNode[];
  /** Disable progress persistence and session UI, but allow chapter navigation. */
  readOnly?: boolean;
  /** Where the top-bar 返回 points (book detail page); hidden when absent. */
  backHref?: string;
  sessionControls?: ReactNode;
  /** Public reader URL, or an explicit action that creates one for this file. */
  shareLink?: string;
  createShareLink?: () => Promise<string>;
}

export function Reader({
  title,
  chapters,
  toc,
  loadChapter,
  retryChapter,
  initialPosition,
  onProgress,
  readOnly,
  backHref,
  sessionControls,
  shareLink,
  createShareLink,
}: ReaderProps) {
  const { t } = useTranslation();
  const [settings, setSettings] = useReaderSettings();
  const [tocOpen, setTocOpen] = useState(false);
  const selectionRef = useRef<(ScreenshotSelection & { chapterIdx: number }) | null>(null);
  const [image, setImage] = useState<ReaderImage | null>(null);
  const [excerpt, setExcerpt] = useState<ReaderExcerpt | null>(null);
  const [screenshotHint, setScreenshotHint] = useState(false);
  const [createdShareLink, setCreatedShareLink] = useState<string>();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [idx, setIdx] = useState(() =>
    Math.min(initialPosition?.chapter_idx ?? 0, Math.max(chapters.length - 1, 0)),
  );
  const [chapter, setChapter] = useState<Chapter | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [fraction, setFraction] = useState(
    initialPosition?.in_chapter && "fraction" in initialPosition.in_chapter
      ? initialPosition.in_chapter.fraction
      : 0,
  );
  const loadedRef = useRef<Map<number, Chapter>>(new Map());
  const pendingFragRef = useRef<string | null>(null);
  const epubRef = useRef<EpubDocumentHandle>(null);
  const articleRef = useRef<HTMLElement>(null);
  const screenshotButtonRef = useRef<HTMLButtonElement>(null);
  const textRangeRef = useRef<Range | null>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const renderTarget = useRef<{ fragment: string | null; fraction: number }>({ fragment: null, fraction: 0 });
  const isDocument = chapter?.content.startsWith("<!doctype html>") ?? false;

  useEffect(() => {
    if (isDocument) return;
    const root = articleRef.current?.querySelector(".epub-content");
    if (!root) return;
    function updateSelection() {
      const current = document.getSelection();
      const text = selectedChapterText(root!, current);
      textRangeRef.current = text ? current!.getRangeAt(0).cloneRange() : null;
      selectionRef.current = { text, chapterIdx: idx };
    }
    document.addEventListener("selectionchange", updateSelection);
    return () => document.removeEventListener("selectionchange", updateSelection);
  }, [chapter, idx, isDocument]);

  function openScreenshot() {
    const selection = selectionRef.current;
    if (!chapter || !selection?.text || selection.chapterIdx !== idx || !articleRef.current) {
      setScreenshotHint(true);
      return;
    }
    const style = getComputedStyle(articleRef.current);
    const root = articleRef.current.querySelector<HTMLElement>(".epub-content");
    const selected = isDocument
      ? epubRef.current?.captureSelection()
      : root && textRangeRef.current ? selectedTextContent(root, textRangeRef.current) : selection;
    if (!selected?.text) {
      setScreenshotHint(true);
      return;
    }
    setExcerpt({
      ...selected, title, chapter: chapter.title, chapterIdx: idx,
      style: {
        background: style.backgroundColor === "rgba(0, 0, 0, 0)" ? style.getPropertyValue("--bg").trim() : style.backgroundColor,
        foreground: style.getPropertyValue("--fg").trim() || style.color,
        muted: style.getPropertyValue("--reader-muted").trim(),
        accent: style.getPropertyValue("--accent").trim(),
        fontFamily: style.fontFamily,
      },
    });
    setSettingsOpen(false);
    setScreenshotHint(false);
  }

  function scrollToFragId(frag: string | null) {
    // The chapter content carries the original element ids, so a TOC
    // fragment entry (`file.html#section`) can scroll into its section.
    requestAnimationFrame(() => {
      if (frag && epubRef.current) { epubRef.current.scrollToFragment(frag); return; }
      const el = frag ? Array.from(articleRef.current?.querySelectorAll("[id]") ?? []).find((el) => el.id === frag) : null;
      if (el) el.scrollIntoView({ block: "start" });
    });
  }

  // Load chapter content (cached), and restore scroll by fraction once.
  useEffect(() => {
    const cached = loadedRef.current.get(idx);
    const frag = pendingFragRef.current;
    pendingFragRef.current = null;
    let cancelled = false;
    renderTarget.current = { fragment: frag, fraction };
    setChapter(null);
    setLoadError(null);
    function display(c: Chapter) {
      if (cancelled) return;
      setChapter(c);
      setLoading(false);
      if (!c.content.startsWith("<!doctype html>")) {
        restoreScroll(fraction);
        scrollToFragId(frag);
      }
    }
    if (cached) display(cached);
    else {
      setLoading(true);
      const load = loadAttempt > 0 && retryChapter ? retryChapter : loadChapter;
      void load(idx).then((c) => {
        loadedRef.current.set(idx, c);
        display(c);
      }).catch((error: unknown) => {
        if (cancelled) return;
        setLoading(false);
        setLoadError(error instanceof ApiError && error.status === 404
          ? t(retryChapter ? "reader.chapterNotCached" : "reader.sharedChapterNotCached")
          : error instanceof Error ? error.message : t("common.failed"));
      });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
    return () => { cancelled = true; };
  }, [idx, loadAttempt]);

  function restoreScroll(frac: number) {
    if (!frac) {
      viewportRef.current?.scrollTo(0, 0);
      return;
    }
    // wait a tick for the content to render
    requestAnimationFrame(() => {
      const viewport = viewportRef.current;
      if (!viewport) return;
      const max = viewport.scrollHeight - viewport.clientHeight;
      viewport.scrollTo(0, Math.round(max * frac));
    });
  }

  const goto = useCallback(
    (next: number, frag?: string) => {
      const clamped = Math.max(0, Math.min(next, chapters.length - 1));
      if (clamped === idx) {
        // Same chapter: the content is already rendered — scroll directly.
        if (frag) scrollToFragId(frag);
        else {
          if (epubRef.current) epubRef.current.restoreFraction(0);
          else restoreScroll(0);
          setFraction(0);
          lastFractionRef.current = 0;
          if (onProgress && !readOnly) onProgress(0, clamped);
        }
        return;
      }
      window.clearTimeout(debounceRef.current);
      lastChapterRef.current = clamped;
      lastFractionRef.current = 0;
      if (onProgress && !readOnly) onProgress(0, clamped);
      setImage(null);
      selectionRef.current = null;
      textRangeRef.current = null;
      setScreenshotHint(false);
      setIdx(clamped);
      setLoadAttempt(0);
      setFraction(0);
      pendingFragRef.current = frag ?? null;
    },
    [chapters.length, idx, onProgress, readOnly],
  );

  // Debounced scroll tracking → progress callback.
  const debounceRef = useRef<number | undefined>(undefined);
  const lastFractionRef = useRef(fraction);
  const lastChapterRef = useRef(idx);
  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    function onScroll() {
      if (isDocument) return;
      const max = viewport!.scrollHeight - viewport!.clientHeight;
      const f = max > 0 ? Math.min(1, Math.max(0, viewport!.scrollTop / max)) : 0;
      setFraction(f);
      lastFractionRef.current = f;
      if (!onProgress || readOnly) return;
      window.clearTimeout(debounceRef.current);
      debounceRef.current = window.setTimeout(() => onProgress(f, idx), 600);
    }
    viewport.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      viewport.removeEventListener("scroll", onScroll);
      window.clearTimeout(debounceRef.current);
      // Flush the pending fraction on unmount (leaving the reader must
      // not lose the last 600 ms of scroll).
      if (onProgress && !readOnly && lastFractionRef.current > 0) {
        onProgress(lastFractionRef.current, lastChapterRef.current);
      }
    };
  }, [onProgress, readOnly, isDocument, idx]);

  const cur = chapters.find((c) => c.idx === idx);
  const linearChapters = chapters.filter(c => c.linear !== false);
  const linearRank = linearChapters.findIndex(c => c.idx === idx);
  const prev = linearChapters.filter(c => c.idx < idx).at(-1) ?? null;
  const next = linearChapters.find(c => c.idx > idx) ?? null;
  const progress = linearChapters.length > 0 && linearRank >= 0 ? ((linearRank + fraction) / linearChapters.length) * 100 : 0;

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
      {/* Reader controls stay visible while the content scrolls. */}
      <header className="reader-topbar">
        <div className="reader-topbar-side">
          {backHref && (
            <Link className="btn ghost reader-toolbar-icon reader-back" to={backHref} aria-label={t("common.backToDetail")} title={t("common.backToDetail")}>
              <IconArrowLeft size={18} />
            </Link>
          )}
          {(toc ?? []).length > 0 && (
            <button
              className="btn ghost reader-toolbar-icon"
              onClick={() => setTocOpen(true)}
              aria-expanded={tocOpen}
              aria-label={t("reader.toc")}
              title={t("reader.toc")}
            >
              <IconList size={18} />
            </button>
          )}
        </div>
        <div className="reader-topbar-center">
          <span className="reader-topbar-book" title={title}>{title}</span>
          <div className="reader-topbar-meta">
            <span className="reader-topbar-chapter" title={cur?.title}>{cur?.title ?? ""}</span>
            {!readOnly && <span className="reader-topbar-progress">{Math.round(progress)}%</span>}
          </div>
        </div>
        <div className="reader-topbar-side reader-topbar-end">
          <button ref={screenshotButtonRef} className="btn ghost reader-toolbar-icon"
            disabled={!chapter}
            onMouseDown={event => event.preventDefault()}
            onClick={openScreenshot}
            aria-label={t("screenshot.title")} title={t("screenshot.title")}>
            <IconCamera size={18} />
          </button>
          <button
            className="btn ghost reader-toolbar-icon"
            onClick={() => setSettingsOpen((v) => !v)}
            aria-expanded={settingsOpen}
            aria-label={t("reader.settings")}
            title={t("reader.settings")}
          >
            <IconType size={18} />
          </button>
        </div>
        {settingsOpen && (
          <ReaderSettingsPanel
            settings={settings}
            sessionControls={sessionControls}
            onChange={setSettings}
            onClose={() => setSettingsOpen(false)}
          />
        )}
      </header>

      {screenshotHint && <p className="screenshot-hint" role="status">{t("screenshot.selectHint")}</p>}
      {image && <ReaderImageDialog key={image.src} image={image} onClose={() => setImage(null)} />}
      {excerpt && <ReaderScreenshotDialog excerpt={excerpt} shareLink={shareLink ?? createdShareLink}
        createShareLink={createShareLink ? async () => {
          const link = await createShareLink();
          setCreatedShareLink(link);
          return link;
        } : undefined}
        onClose={() => { setExcerpt(null); requestAnimationFrame(() => screenshotButtonRef.current?.focus()); }} />}

      {/* left slide-in table of contents */}
      {tocOpen && (
        <>
          <div className="toc-drawer-backdrop" onClick={() => setTocOpen(false)} />
          <nav className="popover toc-drawer" aria-label={t("reader.toc")}>
            <div className="toc-drawer-head">
              <h2>{t("reader.toc")}</h2>
              <button className="btn ghost sm" onClick={() => setTocOpen(false)} aria-label={t("common.close")}>
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

      <div ref={viewportRef} className={`reader-viewport${isDocument ? " reader-viewport-document" : ""}`}>
        <article
          ref={articleRef}
          onClick={event => {
            if (isDocument || event.defaultPrevented || event.button !== 0 || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey || document.getSelection()?.toString()) return;
            const selected = readerImageAt(event.target as Element);
            if (selected) setImage(selected);
          }}
          className={`reader-body font-${settings.font}${isDocument ? " reader-body-document" : ""}`}
          style={{
            fontSize: `${settings.fontSize}px`,
            lineHeight: settings.lineHeight,
            maxWidth: `${settings.widthEm}em`,
          }}
        >
          {loading && <p className="hint">{t("common.loading")}</p>}
          {loadError !== null && <div>
            <p className="error" role="alert">{loadError}</p>
            <button className="btn" onClick={() => setLoadAttempt((attempt) => attempt + 1)}>
              {t("reader.retryChapter")}
            </button>
          </div>}
          {chapter && (isDocument ? <EpubDocument
            ref={epubRef}
            content={chapter.content}
            title={chapter.title}
            settings={settings}
            onNavigate={goto}
            onImageOpen={setImage}
            onSelectionChange={selected => { selectionRef.current = { ...selected, chapterIdx: idx }; }}
            onReady={() => {
              const target = renderTarget.current;
              epubRef.current?.restoreFraction(target.fraction);
              if (target.fragment) epubRef.current?.scrollToFragment(target.fragment);
            }}
            onFraction={(f) => {
              setFraction(f);
              lastFractionRef.current = f;
              if (!onProgress || readOnly) return;
              window.clearTimeout(debounceRef.current);
              debounceRef.current = window.setTimeout(() => onProgress(f, idx), 600);
            }}
          /> : <TextChapterContent content={chapter.content} />)}
        </article>
      </div>

      {/* single prev/next control set — large buttons with chapter preview */}
      <div className="reader-bottom">
        <button
          className="reader-nav-btn"
          onClick={() => prev && goto(prev.idx)}
          disabled={!prev}
        >
          <small>
            <IconChevronLeft size={13} /> {t("reader.prev")}
          </small>
          <span>{prev?.title ?? ""}</span>
        </button>
        <button
          className="reader-nav-btn next"
          onClick={() => next && goto(next.idx)}
          disabled={!next}
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
