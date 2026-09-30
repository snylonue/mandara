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
//   - book detail "更换内容源" (rebind a file's content source).

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ApiError } from "../api";
import { Modal } from "./Modal";
import type { PluginInstance, PluginSearchItem, PluginSearchResponse } from "../types";

export const SEARCH_PAGE_SIZE = 10;

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

export function SourceSearchPane({
  instances,
  filter,
  onPick,
  pickLabel,
}: {
  instances: PluginInstance[];
  filter: (i: PluginInstance) => boolean;
  onPick: (pick: SourcePick) => Promise<void> | void;
  /** Optional custom label for the pick button ("加入书架" by default). */
  pickLabel?: (item: PluginSearchItem) => string;
}) {
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
  const seq = useRef(0);

  const selected = options.find((i) => i.id === sel);
  const interaction = selected ? sourceInteraction(selected) : null;
  // Manual-id entry needs the source's `lookup` capability and is shown
  // whenever the source has it — alongside the search box when the
  // source also supports search (bangumi: search *and* direct id).
  const showSearch = interaction?.canSearch ?? false;
  const showManualId = interaction?.canManualId ?? false;

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
      if (seq.current === mySeq) setResult(r);
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
    if (!sel || !showSearch) {
      setResult(null);
      setError(null);
      return;
    }
    setResult(null); // drop the previous page while the new search loads
    void load(sel, q, 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sel, q, showSearch]);

  const total = result?.total ?? 0;
  const shown = result?.items.length ?? 0;

  function pick(item: PluginSearchItem) {
    if (!selected) return;
    setPickError(null);
    setPicking(item.id);
    Promise.resolve(onPick({ instance: selected, item }))
      .catch((e) => setPickError(e instanceof ApiError ? e.message : String(e)))
      .finally(() => setPicking(null));
  }

  return (
    <div className="source-search">
      <div className="row">
        <select value={sel} onChange={(e) => setSel(e.target.value)}>
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
            placeholder={searchPlaceholder(selected, t("plugin.searchPlaceholder"))}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        )}
      </div>

      {error && <div className="error">{error}</div>}
      {loading && <p className="hint">{t("common.loading")}</p>}
      {!loading && !error && showSearch && sel && result && result.items.length === 0 && (
        <p className="hint">{t("plugin.noResults")}</p>
      )}
      {showManualId && sel && selected && (
        <div className="field">
          {!showSearch && <p className="hint">{t("plugin.noSearch")}</p>}
          {showSearch && <p className="hint">{t("plugin.manualIdAlso")}</p>}
          <div className="row">
            <input
              placeholder={idHint(selected, t("plugin.manualIdPlaceholder"))}
              value={manual}
              onChange={(e) => setManual(e.target.value)}
              aria-label={t("plugin.manualIdPlaceholder")}
            />
            {manual.trim() && (
              <button
                className="btn link"
                disabled={picking !== null}
                onClick={() =>
                  pick({
                    // A manual id is a *lookup key*, not metadata: the
                    // entry's title/authors/description are unknown until
                    // the source resolves the id (via `get-book` on the
                    // backend). Filling `title` with the id would be sent
                    // as a metadata override and clobber the real title
                    // (e.g. the id "50538" would overwrite 游戏人生).
                    id: manual.trim(),
                    title: "",
                    authors: [],
                    description: null,
                    cover_url: null,
                    book_id: null,
                  })
                }
              >
                {picking !== null ? t("common.loading") : t("plugin.useManualId")}
              </button>
            )}
          </div>
        </div>
      )}
      {!loading && !error && interaction?.kind === "browse" && sel && (
        <p className="hint">{t("plugin.browseSynced")}</p>
      )}
      <ul className="source-results">
        {result?.items.map((item) => (
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
              disabled={picking !== null}
              onClick={() => pick(item)}
            >
              {picking === item.id ? t("common.loading") : (pickLabel?.(item) ?? t("plugin.addToShelf"))}
            </button>
          </li>
        ))}
      </ul>
      {!loading && total > shown && (
        <div className="row">
          <button
            className="btn link"
            disabled={sel === ""}
            onClick={() => void load(sel, q, shown)}
          >
            {t("plugin.more", { total: String(total - shown) })}
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