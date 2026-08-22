// Reusable plugin source search pane (design v2, lazy catalog browsing).
//
// Renders an instance select + search box + paginated result list for
// instances matching the given filter. Used by:
//   - library "源浏览器" (materialize a book),
//   - upload dialog manual mode (pick metadata from a plugin),
//   - book detail "更换内容源" (rebind a file's content source).

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ApiError } from "../api";
import type { PluginInstance, PluginSearchItem, PluginSearchResponse } from "../types";

export const SEARCH_PAGE_SIZE = 10;

export type SourcePick = {
  instance: PluginInstance;
  item: PluginSearchItem;
};

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
  const seq = useRef(0);

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

  // Re-search when the instance or the query changes.
  useEffect(() => {
    if (!sel) {
      setResult(null);
      setError(null);
      return;
    }
    void load(sel, q, 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sel, q]);

  const total = result?.total ?? 0;
  const shown = result?.items.length ?? 0;

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
        {sel && (
          <input
            className="search"
            placeholder={t("plugin.searchPlaceholder")}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        )}
      </div>

      {error && <div className="error">{error}</div>}
      {loading && <p className="hint">{t("common.loading")}</p>}
      {!loading && !error && sel && result && result.items.length === 0 && (
        <p className="hint">{t("plugin.noResults")}</p>
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
              className="link-btn"
              disabled={picking !== null}
              onClick={() => {
                setPickError(null);
                setPicking(item.id);
                Promise.resolve(
                  onPick({ instance: options.find((i) => i.id === sel)!, item }),
                )
                  .catch((e) =>
                    setPickError(e instanceof ApiError ? e.message : String(e)),
                  )
                  .finally(() => setPicking(null));
              }}
            >
              {picking === item.id ? t("common.loading") : (pickLabel?.(item) ?? t("plugin.addToShelf"))}
            </button>
          </li>
        ))}
      </ul>
      {!loading && total > shown && (
        <div className="row">
          <button
            className="link-btn"
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
  allowManualId = false,
}: {
  open: boolean;
  onClose: () => void;
  instances: PluginInstance[];
  filter: (i: PluginInstance) => boolean;
  onPick: (pick: SourcePick, manualIdValue?: string) => Promise<void> | void;
  title: string;
  pickLabel?: (item: PluginSearchItem) => string;
  allowManualId?: boolean;
}) {
  const { t } = useTranslation();
  const [pickError, setPickError] = useState<string | null>(null);
  const [manual, setManual] = useState("");
  const [manualPickLoading, setManualPickLoading] = useState(false);

  if (!open) return null;
  const pick = (instance: PluginInstance, item: PluginSearchItem, manualId?: string) => {
    setPickError(null);
    if (manualId) setManualPickLoading(true);
    Promise.resolve(onPick({ instance, item }, manualId))
      .catch((e) => setPickError(e instanceof ApiError ? e.message : String(e)))
      .finally(() => setManualPickLoading(false));
  };
  return (
    <div className="card">
      <div className="row">
        <h3>{title}</h3>
        <button className="link-btn" onClick={onClose}>
          {t("library.cancel")}
        </button>
      </div>
      <SourceSearchPane
        instances={instances}
        filter={filter}
        pickLabel={pickLabel}
        onPick={(p) => pick(p.instance, p.item)}
      />
      {allowManualId && (
        <div className="field">
          <label className="hint">
            {t("plugin.manualId")}
            <input
              placeholder={t("plugin.manualIdPlaceholder")}
              value={manual}
              onChange={(e) => setManual(e.target.value)}
            />
          </label>
          {manual.trim() && instances.length > 0 && (
            <button
              className="link-btn"
              disabled={manualPickLoading}
              onClick={() => {
                const instance = instances.find(filter);
                if (!instance) return;
                pick(
                  instance,
                  {
                    id: manual.trim(),
                    title: manual.trim(),
                    authors: [],
                    description: null,
                    cover_url: null,
                    book_id: null,
                  },
                  manual.trim(),
                );
              }}
            >
              {manualPickLoading ? t("common.loading") : t("plugin.useManualId")}
            </button>
          )}
        </div>
      )}
      {pickError && <div className="error">{pickError}</div>}
    </div>
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