// Reusable plugin source picker (design v2/v3, lazy catalog browsing).
//
// Interaction abstraction: a source is presented according to its
// *interaction kind* (`sourceInteraction`, derived from the plugin's
// `source-info` declaration and its capabilities). Kinds:
//   - "search"    — the catalog is browsed with a query box
//     (`search-books`, paginated results + pick buttons);
//   - "manual-id" — no search (wenku8-style login-walled sources); users
//     enter a book id directly (`lookup`), with a per-source hint from
//     `source_info.id_hint`;
//   - "browse"    — a declared catalog, already synced at startup;
//     there is nothing to pick.
// The declared kind wins only when it matches this frontend's
// capabilities-derived reality; unsupported future kinds degrade to the
// capability default, so one new plugin cannot break the picker.
//
// Used by:
//   - the unified add-book dialog (获取书籍 step, file/plugin tabs),
//   - book detail "添加内容" (add an independent content item).

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ApiError } from "../api";
import { Modal } from "./Modal";
import { ContentFilePicker } from "./ContentFilePicker";
import type { PluginInstance, PluginSearchItem, PluginSearchResponse, PluginSeriesLookupResponse } from "../types";

export const SEARCH_PAGE_SIZE = 10;

export type SeriesSourcePick = {
  instance: PluginInstance;
  item: PluginSeriesLookupResponse["series"];
  books: PluginSeriesLookupResponse["books"];
};

export type SourcePick = {
  instance: PluginInstance;
  item: PluginSearchItem;
};

/**
 * The acquisition affordances of a source. Search and manual-id entry
 * are independent: a source with both `search` and `lookup` capabilities
 * shows both (e.g. bangumi — search the catalog *and* enter an id
 * directly), while a `lookup`-only source shows just the manual entry
 * (wenku8-style) and a `declare`-only source offers nothing here.
 */
export type SourceInteraction = {
  /** The primary interaction (kept for callers that render one mode). */
  kind: "search" | "manual-id" | "browse";
  /** The catalog is browsable with a query box (`search` capability). */
  canSearch: boolean;
  /** A book id can be entered directly (`lookup` capability). */
  canManualId: boolean;
};

/** Whether a source supports the given capability. */
function has(inst: PluginInstance, cap: string): boolean {
  return inst.capabilities.includes(cap);
}

/**
 * Resolve a source's acquisition affordances. The plugin's `source-info`
 * declaration wins when it matches what this frontend can actually do
 * with the source's capabilities; otherwise the affordances are derived
 * from capabilities (so a stale/unknown declared kind degrades
 * gracefully). Search and manual-id are independent booleans.
 */
export function sourceInteraction(inst: PluginInstance): SourceInteraction {
  const declared = inst.source_info?.kind;
  const canSearch = has(inst, "search");
  const canManualId = has(inst, "lookup");
  if (declared === "manual-id") {
    return {
      kind: canManualId ? "manual-id" : canSearch ? "search" : "browse",
      canSearch,
      canManualId,
    };
  }
  if (canSearch) {
    return { kind: "search", canSearch, canManualId };
  }
  if (canManualId) {
    return { kind: "manual-id", canSearch, canManualId };
  }
  return { kind: "browse", canSearch, canManualId };
}

/** The search-box placeholder of a source (its declared hint, else the
 * locale fallback). */
export function searchPlaceholder(inst: PluginInstance, fallback: string): string {
  return inst.source_info?.search_hint?.trim() || fallback;
}

/** The manual-id entry hint of a source (its declared hint, else the
 * locale fallback). */
export function idHint(inst: PluginInstance, fallback: string): string {
  return inst.source_info?.id_hint?.trim() || fallback;
}

/** Fetch one search page of an instance's catalog. */
export async function searchSource(
  instanceId: string,
  q: string,
  offset: number,
): Promise<PluginSearchResponse> {
  const params = new URLSearchParams({
    q,
    offset: String(offset),
    limit: String(SEARCH_PAGE_SIZE),
  });
  return api<PluginSearchResponse>(`/plugins/${instanceId}/search?${params}`);
}

type SourceSearchProps = {
  purpose?: "metadata" | "content";
  instances: PluginInstance[];
  filter: (i: PluginInstance) => boolean;
  pickLabel?: (item: PluginSearchItem) => string;
  disabled?: boolean;
  onLoadingChange?: (loading: boolean) => void;
} & ({
  target?: "book";
  onPick: (pick: SourcePick) => Promise<void> | void;
  onClear?: () => void;
} | {
  target: "series";
  onPick: (pick: SeriesSourcePick) => Promise<void> | void;
  onClear: () => void;
});

export function SourceSearchPane(props: SourceSearchProps) {
  const { instances, filter, pickLabel } = props;
  const { t } = useTranslation();
  const options = instances.filter(filter);
  const [sel, setSel] = useState("");
  const [q, setQ] = useState("");
  const [result, setResult] = useState<PluginSearchResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [picking, setPicking] = useState<string | null>(null);
  const [pickError, setPickError] = useState<string | null>(null);
  const [manual, setManual] = useState("");
  const [nextOffset, setNextOffset] = useState(0);
  const seq = useRef(0);
  useEffect(() => () => { ++seq.current; }, []);
  useEffect(() => { props.onLoadingChange?.(loading || picking !== null); }, [loading, picking, props.onLoadingChange]);

  const selected = options.find((i) => i.id === sel);
  const interaction = selected ? sourceInteraction(selected) : null;
  // Manual-id entry needs the source's `lookup` capability and is shown
  // whenever the source has it — alongside the search box when the
  // source also supports search (bangumi: search *and* direct id).
  const contentList = props.purpose === "content" && selected?.capabilities.includes("content-list");
  const showSearch = !contentList && (interaction?.canSearch ?? false);
  const showManualId = !contentList && (interaction?.canManualId ?? false);

  // Keep the selection valid when the instance list changes (e.g. after
  // registration in the admin page).
  useEffect(() => {
    if (sel && !options.some((i) => i.id === sel)) setSel("");
  }, [options, sel]);

  async function load(instanceId: string, query: string, offset: number) {
    const mySeq = ++seq.current;
    setLoading(true);
    setError(null);
    try {
      const r = await searchSource(instanceId, query, offset);
      if (seq.current === mySeq) { setResult(r); setNextOffset(offset + r.items.length); }
    } catch (e) {
      if (seq.current !== mySeq) return;
      setError(e instanceof Error ? e.message : t("common.failed"));
      setResult(null);
    } finally {
      if (seq.current === mySeq) setLoading(false);
    }
  }

  // Re-search when the instance or the query changes. Only searchable
  // sources are searched; others pick books by id. A query change also
  // clears the stale result page (old pagination does not survive a new
  // search term).
  useEffect(() => {
    ++seq.current;
    props.onClear?.();
    setNextOffset(0);
    setPickError(null);
    if (!sel || !showSearch) {
      setLoading(false);
      setResult(null);
      setError(null);
      return;
    }
    setResult(null); // drop the previous page while the new search loads
    void load(sel, q, 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sel, q, showSearch, props.target]);

  const total = result?.total ?? 0;
  const items = (result?.items ?? []).filter((item) => props.target !== "series" || item.kind === "series");

  async function lookup(key: string) {
    if (!selected || props.disabled) return;
    const mySeq = ++seq.current;
    props.onClear?.();
    setLoading(true);
    setError(null);
    try {
      const params = new URLSearchParams({ q: key });
      if (props.target === "series") {
        const r = await api<PluginSeriesLookupResponse>(`/plugins/${selected.id}/series?${params}`);
        if (seq.current === mySeq) {
          setResult({ total: 1, items: [r.series] });
          setNextOffset(1);
          await props.onPick({ instance: selected, item: r.series, books: r.books });
        }
      } else {
        const r = await api<PluginSearchResponse>(`/plugins/${selected.id}/lookup?${params}`);
        if (seq.current === mySeq) { setResult(r); setNextOffset(r.total); }
      }
    } catch (e) {
      if (seq.current === mySeq) {
        setResult(null);
        setError(e instanceof Error ? e.message : t("common.failed"));
      }
    } finally {
      if (seq.current === mySeq) setLoading(false);
    }
  }

  function pick(item: PluginSearchItem) {
    if (!selected || props.disabled) return;
    if (props.target === "series") { if (item.kind === "series") void lookup(item.id); return; }
    if (item.kind === "series") { void lookup(item.id); return; }
    setPickError(null);
    setPicking(item.id);
    Promise.resolve(props.onPick({ instance: selected, item }))
      .catch((e) => setPickError(e instanceof ApiError ? e.message : String(e)))
      .finally(() => setPicking(null));
  }

  return (
    <div className="source-search">
      <div className="row">
        <select disabled={props.disabled} value={sel} onChange={(e) => setSel(e.target.value)}>
          <option value="">{t("plugin.chooseSource")}</option>
          {options.map((i) => (
            <option key={i.id} value={i.id}>
              {i.name}（{i.id}）
            </option>
          ))}
        </select>
        {sel && showSearch && selected && (
          <input
            className="search"
            disabled={props.disabled}
            placeholder={props.target === "series" ? t("plugin.seriesSearchPlaceholder") : searchPlaceholder(selected, t("plugin.searchPlaceholder"))}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        )}
      </div>

      {contentList && selected && <ContentFilePicker
        key={selected.id} instance={selected} disabled={props.disabled} multiple={props.target === "series"}
        onClear={props.onClear} onLoadingChange={props.onLoadingChange}
        onPick={(options, reference) => {
          const books: PluginSeriesLookupResponse["books"] = options.map((option) => ({
            kind: "book", id: option.id, title: option.title, authors: [], book_id: null,
          }));
          if (props.target === "series") {
            return props.onPick({ instance: selected, item: { kind: "series", id: reference, title: reference, authors: [], book_id: null }, books });
          }
          return props.onPick({ instance: selected, item: books[0] });
        }}
      />}
      {error && <div className="error">{error}</div>}
      {loading && <p className="hint">{t("common.loading")}</p>}
      {!loading && !error && sel && result && items.length === 0 && (
        <p className="hint">{t(props.target === "series" ? "plugin.noSeriesResults" : "plugin.noResults")}</p>
      )}
      {showManualId && sel && selected && (
        <div className="field">
          {props.target === "series" ? <p className="hint">{t("plugin.manualSeriesIdAlso")}</p> : <>
            {!showSearch && <p className="hint">{t("plugin.noSearch")}</p>}
            {showSearch && <p className="hint">{t("plugin.manualIdAlso")}</p>}
          </>}
          <div className="row">
            <input
              disabled={props.disabled}
              placeholder={props.target === "series" ? t("plugin.seriesIdPlaceholder") : idHint(selected, t("plugin.manualIdPlaceholder"))}
              value={manual}
              onChange={(e) => {
                setManual(e.target.value);
                ++seq.current; props.onClear?.(); setResult(null); setLoading(false);
              }}
              aria-label={t(props.target === "series" ? "plugin.seriesIdPlaceholder" : "plugin.manualIdPlaceholder")}
            />
            {manual.trim() && (
              <button
                className="btn link"
                disabled={props.disabled || picking !== null || loading}
                onClick={() => void lookup(manual.trim())}
              >
                {picking !== null ? t("common.loading") : t("plugin.useManualId")}
              </button>
            )}
          </div>
        </div>
      )}
      {!contentList && !loading && !error && interaction?.kind === "browse" && sel && (
        <p className="hint">{t("plugin.browseSynced")}</p>
      )}
      <ul className="source-results">
        {items.map((item) => (
          <li key={item.id} className="source-result">
            <div>
              <span className="strong">{item.title}</span>
              <span className="tag">{item.id}</span>
              {item.book_id && <span className="tag">{t("plugin.inLibrary")}</span>}
              <div className="hint">
                {item.authors.join(" / ") || t("common.anonymous")}
                {item.description ? ` · ${item.description}` : ""}
              </div>
            </div>
            <button
              className="btn link"
              disabled={props.disabled || picking !== null || loading}
              onClick={() => pick(item)}
            >
              {picking === item.id ? t("common.loading") : (item.kind === "series" && props.target !== "series" ? t("plugin.browseSeries") : (pickLabel?.(item) ?? t("plugin.addToShelf")))}
            </button>
          </li>
        ))}
      </ul>
      {!loading && total > nextOffset && (
        <div className="row">
          <button
            className="btn link"
            disabled={props.disabled || sel === "" || loading}
            onClick={() => void load(sel, q, nextOffset)}
          >
            {t("plugin.more", { total: String(total - nextOffset) })}
          </button>
        </div>
      )}
      {pickError && <div className="error">{pickError}</div>}
    </div>
  );
}

/** Modal wrapper around the pane, for one-shot pick dialogs. */
export function SourceBrowserDialog({
  open,
  onClose,
  instances,
  filter,
  onPick,
  title,
  pickLabel,
}: {
  open: boolean;
  onClose: () => void;
  instances: PluginInstance[];
  filter: (i: PluginInstance) => boolean;
  onPick: (pick: SourcePick) => Promise<void> | void;
  title: string;
  pickLabel?: (item: PluginSearchItem) => string;
}) {
  const [pickError, setPickError] = useState<string | null>(null);

  if (!open) return null;
  return (
    <Modal open onClose={onClose} title={title} wide>
      <SourceSearchPane
        instances={instances}
        filter={filter}
        pickLabel={pickLabel}
        onPick={(p) => {
          setPickError(null);
          Promise.resolve(onPick(p))
            .catch((e) => setPickError(e instanceof ApiError ? e.message : String(e)))
            .finally(() => undefined);
        }}
      />
      {pickError && <div className="error">{pickError}</div>}
    </Modal>
  );
}

/** Load the plugin instance list once (shared by pickers). */
export function usePluginInstances() {
  const [instances, setInstances] = useState<PluginInstance[]>([]);
  useEffect(() => {
    api<PluginInstance[]>("/plugins")
      .then(setInstances)
      .catch(() => setInstances([]));
  }, []);
  return instances;
}