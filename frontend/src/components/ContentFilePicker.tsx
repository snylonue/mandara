// Enumerate source-owned content choices; users never construct content IDs.
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ApiError } from "../api";
import type { PluginContentOption, PluginInstance } from "../types";

export function ContentFilePicker({ instance, initialReference = "", disabled = false, selectedIds = [], multiple = false, onPick, onClear, onLoadingChange }: {
  instance: PluginInstance;
  initialReference?: string;
  disabled?: boolean;
  selectedIds?: string[];
  multiple?: boolean;
  onPick: (items: PluginContentOption[], reference: string) => void | Promise<void>;
  onClear?: () => void;
  onLoadingChange?: (loading: boolean) => void;
}) {
  const { t } = useTranslation();
  const [reference, setReference] = useState(initialReference);
  const [items, setItems] = useState<PluginContentOption[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const request = useRef<AbortController | null>(null);
  const sequence = useRef(0);
  useEffect(() => { onLoadingChange?.(loading || picking); }, [loading, picking, onLoadingChange]);

  async function load(value: string) {
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    setLoading(true); setError(null); setItems(null);
    try {
      const params = new URLSearchParams({ q: value.trim() });
      const options = await api<PluginContentOption[]>("/plugins/" + encodeURIComponent(instance.id) + "/content-options?" + params, { signal: controller.signal });
      if (!controller.signal.aborted) setItems(options);
    } catch (err) {
      if (!controller.signal.aborted) setError(err instanceof ApiError && err.status === 404
        ? t("plugin.contentBookUnavailable") : err instanceof Error ? err.message : t("common.failed"));
    } finally {
      if (!controller.signal.aborted) setLoading(false);
    }
  }
  useEffect(() => {
    if (initialReference.trim()) void load(initialReference);
    return () => { request.current?.abort(); ++sequence.current; onLoadingChange?.(false); };
    // The parent keys this picker by source and initial book reference.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function choose(options: PluginContentOption[]) {
    const current = ++sequence.current;
    setPicking(true); setError(null);
    try { await onPick(options, reference.trim()); }
    catch (err) { if (current === sequence.current) setError(err instanceof Error ? err.message : t("common.failed")); }
    finally { if (current === sequence.current) setPicking(false); }
  }
  const available = (items ?? []).filter((item) => item.available);
  return <div className="content-file-picker">
    <p className="hint">{t("plugin.contentBookHint")}</p>
    <div className="row">
      <input aria-label={t("plugin.contentBookReference")} placeholder={t("plugin.contentBookReference")}
        value={reference} disabled={disabled || picking} onChange={(event) => {
          request.current?.abort(); setLoading(false); setItems(null); setError(null);
          setReference(event.target.value); onClear?.();
        }} />
      <button disabled={disabled || picking || loading || !reference.trim()} onClick={() => void load(reference)}>{t("plugin.listContentFiles")}</button>
    </div>
    {loading && <p role="status" className="hint">{t("common.loading")}</p>}
    {error && <p role="alert" className="error">{error}</p>}
    {items?.length === 0 && <p className="hint">{t("plugin.noContentFiles")}</p>}
    {multiple && available.length > 0 && <button disabled={disabled || picking} onClick={() => void choose(available)}>{t("plugin.useContentFiles")}</button>}
    <ul className="source-results">
      {(items ?? []).map((item) => <li key={item.id} className="source-result">
        <div><span className="strong">{item.title}</span>
          {!item.available && <p className="hint">{t("plugin.contentFileUnavailable")}</p>}
        </div>
        {!multiple && <button disabled={disabled || picking || !item.available || selectedIds.includes(item.id)}
          onClick={() => void choose([item])}>
          {t(selectedIds.includes(item.id) ? "library.sourceContentSelected" : "plugin.chooseContentFile")}
        </button>}
      </li>)}
    </ul>
  </div>;
}
