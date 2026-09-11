// Library / shelf: one flat grid of books and collapsed series (a series
// with ≥2 visible volumes is one stacked card; clicking it opens the series
// page). Sort / group / view live in the URL; the last used combination is
// remembered in localStorage and adopted by the next plain shelf visit.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useAuth } from "../auth";
import { AddDialog, type AddMode } from "../components/AddDialog";
import {
  ShelfBookCard,
  ShelfCardSkeleton,
  ShelfListRow,
  ShelfSeriesCard,
  ShelfSeriesRow,
} from "../components/ShelfCard";
import { useToast } from "../components/toast";
import { usePluginInstances } from "../components/SourceSearch";
import { IconBook, IconGrid, IconGridCompact, IconList } from "../components/icons";
import { attachProgress, progressByFile, type ShelfEntry } from "../progress";
import type { BookListEntry, SeriesBrief, SessionList } from "../types";

type SortKey = "default" | "title" | "recent";
type GroupKey = "series" | "flat" | "author";
type ViewKey = "grid" | "compact" | "list";

interface ShelfPrefs {
  sort: SortKey;
  group: GroupKey;
  view: ViewKey;
}

const PREFS_KEY = "bookshelf_shelf_prefs";
const SCROLL_KEY = "bookshelf_shelf_scroll";

const SHELF_PREF_DEFAULTS: ShelfPrefs = { sort: "default", group: "series", view: "grid" };

/** Read one preference out of the URL, ignoring anything unexpected. */
function asPref<T extends string>(raw: string | null, allowed: readonly T[], fallback: T): T {
  return allowed.includes(raw as T) ? (raw as T) : fallback;
}

/** Last used shelf preferences (used to normalize the URL once on load). */
function loadPrefs(): ShelfPrefs {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    return raw ? { ...SHELF_PREF_DEFAULTS, ...(JSON.parse(raw) as Partial<ShelfPrefs>) } : SHELF_PREF_DEFAULTS;
  } catch {
    return SHELF_PREF_DEFAULTS;
  }
}

/** One rendered shelf item: a standalone book or a collapsed series. */
type ShelfItem =
  | { kind: "book"; entry: ShelfEntry }
  | { kind: "series"; series: SeriesBrief; volumes: ShelfEntry[] };

function itemKey(item: ShelfItem): string {
  return item.kind === "series" ? `s-${item.series.id}` : `b-${item.entry.book.id}`;
}

/** Collapse runs of series members into one card each (order preserved). */
function groupBySeries(list: ShelfEntry[]): ShelfItem[] {
  const items: ShelfItem[] = [];
  const at = new Map<string, number>();
  for (const entry of list) {
    const series = entry.series;
    if (!series) {
      items.push({ kind: "book", entry });
      continue;
    }
    const seen = at.get(series.id);
    if (seen === undefined) {
      at.set(series.id, items.length);
      items.push({ kind: "series", series, volumes: [entry] });
    } else {
      const item = items[seen];
      if (item.kind === "series") item.volumes.push(entry);
    }
  }
  // A series with a single visible volume is just a book.
  return items.map((item) =>
    item.kind === "series" && item.volumes.length === 1
      ? { kind: "book" as const, entry: item.volumes[0] }
      : item,
  );
}

const lastReadAt = (entry: ShelfEntry): number =>
  entry.progress ? Date.parse(entry.progress.session.updated_at) : 0;

function sortEntries(list: ShelfEntry[], sort: SortKey): ShelfEntry[] {
  if (sort === "default") return list;
  const copy = [...list];
  if (sort === "title") {
    copy.sort((a, b) => a.book.title.localeCompare(b.book.title, "zh-Hans-CN"));
  } else {
    copy.sort((a, b) => lastReadAt(b) - lastReadAt(a));
  }
  return copy;
}

export function LibraryPage() {
  const { user } = useAuth();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const instances = usePluginInstances();
  const toast = useToast();
  // Sort / group / view live in the URL (`?sort=&group=&view=`) so a
  // bookmarked or back-navigated shelf keeps the view the user had.
  const [params, setParams] = useSearchParams();
  const prefs: ShelfPrefs = {
    sort: asPref(params.get("sort"), ["default", "title", "recent"], "default"),
    group: asPref(params.get("group"), ["series", "flat", "author"], "series"),
    view: asPref(params.get("view"), ["grid", "compact", "list"], "grid"),
  };
  const setPrefs = (next: ShelfPrefs) => {
    const p = new URLSearchParams(params);
    for (const [key, value] of Object.entries(next)) {
      if (value === SHELF_PREF_DEFAULTS[key as keyof ShelfPrefs]) p.delete(key);
      else p.set(key, value);
    }
    setParams(p, { replace: true });
  };
  const [entries, setEntries] = useState<BookListEntry[] | null>(null);
  const syncedSearch = useRef<string | null>(null);
  const [sessions, setSessions] = useState<SessionList>([]);
  const [q, setQ] = useState("");
  // The unified add dialog (添加书籍 / 新建系列 → 一个对话框).
  const [addOpen, setAddOpen] = useState(false);
  const [addMode, setAddMode] = useState<AddMode>("book");
  // Monotonic sequence for the search requests: only the latest response
  // lands (a slow earlier query must not clobber a newer one).
  const searchSeq = useRef(0);
  const restored = useRef(false);

  const load = useCallback(
    async (query: string) => {
      const mySeq = ++searchSeq.current;
      try {
        const list = await api<BookListEntry[]>(`/books?q=${encodeURIComponent(query)}`);
        if (searchSeq.current === mySeq) setEntries(list);
      } catch (err) {
        if (searchSeq.current !== mySeq) return;
        setEntries([]);
        toast.push("error", err instanceof Error ? err.message : t("common.failed"));
      }
    },
    [toast, t],
  );

  // Debounced search: typing is not a request per keystroke, and the
  // latest query always wins (see searchSeq above).
  useEffect(() => {
    const timer = window.setTimeout(() => void load(q), 250);
    return () => window.clearTimeout(timer);
  }, [load, q]);

  // Reading progress is fetched once per visit (`GET /api/sessions`) and
  // mapped onto the books by file id.
  useEffect(() => {
    void api<SessionList>("/sessions")
      .then(setSessions)
      .catch(() => setSessions([]));
  }, []);

  // Entering the shelf without parameters (mount, 书架 nav, back button)
  // adopts the last used preferences and writes them into the URL.
  useEffect(() => {
    const search = params.toString();
    if (search === syncedSearch.current) return;
    syncedSearch.current = search;
    if (search) return;
    const saved = loadPrefs();
    if (JSON.stringify(saved) !== JSON.stringify(SHELF_PREF_DEFAULTS)) setPrefs(saved);
  }, [params, setPrefs]);

  useEffect(() => {
    localStorage.setItem(PREFS_KEY, JSON.stringify(prefs));
  }, [prefs]);

  // Restore the scroll position once after returning from a book/series
  // (cleared afterwards, so the 书架 nav link opens at the top).
  useEffect(() => {
    const onScroll = () => sessionStorage.setItem(SCROLL_KEY, String(window.scrollY));
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  useEffect(() => {
    if (!entries || restored.current) return;
    restored.current = true;
    const saved = sessionStorage.getItem(SCROLL_KEY);
    sessionStorage.removeItem(SCROLL_KEY);
    if (saved && Number(saved) > 0) {
      requestAnimationFrame(() => window.scrollTo(0, Number(saved)));
    }
  }, [entries]);

  const chapterCountOf = useCallback(
    (fileId: string) => {
      for (const entry of entries ?? []) {
        for (const file of entry.files) if (file.id === fileId) return file.chapter_count;
      }
      return 0;
    },
    [entries],
  );

  const byFile = useMemo(
    () => progressByFile(sessions, chapterCountOf),
    [sessions, chapterCountOf],
  );

  const shelf = useMemo(() => attachProgress(entries ?? [], byFile), [entries, byFile]);

  const authors = useMemo(() => {
    const map = new Map<string, ShelfEntry[]>();
    for (const entry of sortEntries(shelf, prefs.sort)) {
      const key = entry.book.authors[0] ?? "";
      const list = map.get(key);
      if (list) list.push(entry);
      else map.set(key, [entry]);
    }
    return [...map.entries()].map(([author, list]) => ({ author, list }));
  }, [shelf, prefs.sort]);

  const items = useMemo(() => {
    if (prefs.group === "series") return groupBySeries(sortEntries(shelf, prefs.sort));
    return sortEntries(shelf, prefs.sort).map(
      (entry): ShelfItem => ({ kind: "book", entry }),
    );
  }, [shelf, prefs]);

  function renderCards(list: ShelfItem[]) {
    if (prefs.view === "list") {
      return (
        <div className="shelf-list">
          {list.map((item) =>
            item.kind === "series" ? (
              <ShelfSeriesRow key={itemKey(item)} series={item.series} volumes={item.volumes} />
            ) : (
              <ShelfListRow key={itemKey(item)} entry={item.entry} />
            ),
          )}
        </div>
      );
    }
    return (
      <div className={`shelf-grid${prefs.view === "compact" ? " shelf-grid--compact" : ""}`}>
        {list.map((item) =>
          item.kind === "series" ? (
            <ShelfSeriesCard key={itemKey(item)} series={item.series} volumes={item.volumes} />
          ) : (
            <ShelfBookCard key={itemKey(item)} entry={item.entry} />
          ),
        )}
      </div>
    );
  }

  return (
    <div className="shelf-page">
      <div className="shelf-toolbar">
        <input
          className="search"
          placeholder={t("library.searchPlaceholder")}
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        {user?.role === "admin" && (
          <span className="admin-chip" title={t("library.adminHint")}>
            {t("library.adminShort")}
          </span>
        )}
        <button
          className="primary"
          onClick={() => {
            setAddMode("book");
            setAddOpen(true);
          }}
        >
          {t("library.addBook")}
        </button>
        <button
          onClick={() => {
            setAddMode("series");
            setAddOpen(true);
          }}
        >
          {t("series.create")}
        </button>
      </div>

      <div className="shelf-bar">
        <h2 className="shelf-title">{t("library.shelfTitle")}</h2>
        <div className="shelf-controls">
          <select
            value={prefs.sort}
            onChange={(e) => setPrefs({ ...prefs, sort: e.target.value as SortKey })}
            aria-label={t("library.sort")}
            title={t("library.sort")}
          >
            <option value="default">{t("library.sortDefault")}</option>
            <option value="title">{t("library.sortTitle")}</option>
            <option value="recent">{t("library.sortRecent")}</option>
          </select>
          <select
            value={prefs.group}
            onChange={(e) => setPrefs({ ...prefs, group: e.target.value as GroupKey })}
            aria-label={t("library.group")}
            title={t("library.group")}
          >
            <option value="series">{t("library.groupSeries")}</option>
            <option value="flat">{t("library.groupFlat")}</option>
            <option value="author">{t("library.groupAuthor")}</option>
          </select>
          <div className="segmented" role="group" aria-label={t("library.view")}>
            {(
              [
                ["grid", <IconGrid size={15} key="g" />, t("library.viewGrid")],
                ["compact", <IconGridCompact size={15} key="c" />, t("library.viewCompact")],
                ["list", <IconList size={15} key="l" />, t("library.viewList")],
              ] as [ViewKey, React.ReactNode, string][]
            ).map(([key, icon, label]) => (
              <button
                key={key}
                className={prefs.view === key ? "active" : ""}
                aria-pressed={prefs.view === key}
                title={label}
                onClick={() => setPrefs({ ...prefs, view: key })}
              >
                {icon}
              </button>
            ))}
          </div>
        </div>
      </div>

      {entries === null && (
        <div className="shelf-grid">
          {Array.from({ length: 12 }, (_, i) => (
            <ShelfCardSkeleton key={`sk-${i}`} />
          ))}
        </div>
      )}

      {entries !== null &&
        (prefs.group === "author" ? (
          authors.map(({ author, list }) => (
            <section className="shelf-section" key={author || "anonymous"}>
              <h2 className="shelf-section-title">
                {author || t("common.anonymous")}
                <span className="count">{list.length}</span>
              </h2>
              {renderCards(list.map((entry): ShelfItem => ({ kind: "book", entry })))}
            </section>
          ))
        ) : (
          renderCards(items)
        ))}

      {entries !== null && entries.length === 0 && (
        <div className="empty-state">
          <IconBook size={40} className="empty-state-icon" />
          <p className="strong">{t("library.emptyTitle")}</p>
          <p className="hint">{t("library.empty")}</p>
        </div>
      )}

      <AddDialog
        open={addOpen}
        onClose={() => setAddOpen(false)}
        instances={instances}
        initialMode={addMode}
        onAdded={() => {
          setQ("");
          void load("");
        }}
        onSeriesCreated={(s) => {
          navigate(`/series/${s.id}`);
        }}
      />
    </div>
  );
}
