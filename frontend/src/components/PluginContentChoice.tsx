// Confirm candidate content independently of metadata before offering acquisition.
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import type { PluginContentPreview } from "../types";
import type { SourcePick } from "./SourceSearch";
import { ContentFilePicker } from "./ContentFilePicker";

type Props = {
  pick: SourcePick;
  disabled: boolean;
  selectedIds: string[];
  onPick: (pick: SourcePick) => void;
};

export function PluginContentChoice(props: Props) {
  const { pick, disabled, selectedIds, onPick } = props;
  if (pick.instance.capabilities.includes("content-list")) return <ContentFilePicker
    key={pick.instance.id + ":" + pick.item.id} instance={pick.instance} initialReference={pick.item.id}
    disabled={disabled} selectedIds={selectedIds}
    onPick={([option]) => onPick({ ...pick, item: { ...pick.item, id: option.id, title: option.title } })}
  />;
  return <DeclaredContentChoice {...props} />;
}

function DeclaredContentChoice({ pick, disabled, selectedIds, onPick }: {
  pick: SourcePick;
  disabled: boolean;
  selectedIds: string[];
  onPick: (pick: SourcePick) => void;
}) {
  const { t } = useTranslation();
  const [candidate, setCandidate] = useState(pick.item.id);
  const [preview, setPreview] = useState<PluginContentPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    setPreview(null);
    setError(null);
    if (!candidate.trim()) { setLoading(false); return; }
    setLoading(true);
    // A metadata ID is only a candidate; never infer content from capabilities.
    const timer = window.setTimeout(() => {
      const params = new URLSearchParams({ q: candidate.trim() });
      api<PluginContentPreview>(
        "/plugins/" + encodeURIComponent(pick.instance.id) + "/content?" + params,
        { signal: controller.signal },
      ).then((value) => { if (!controller.signal.aborted) setPreview(value); })
        .catch((err) => {
          if (!controller.signal.aborted) setError(err instanceof Error && err.message ? err.message : t("common.failed"));
        })
        .finally(() => { if (!controller.signal.aborted) setLoading(false); });
    }, 250);
    return () => { window.clearTimeout(timer); controller.abort(); };
  }, [pick.instance.id, candidate, retry, t]);

  const added = preview !== null && selectedIds.includes(preview.id);
  return (
    <div className="field">
      <p className="hint">{t("library.sourceContentHint")}</p>
      <details>
        <summary>{t("library.changeContentId")}</summary>
        <input
          aria-label={t("library.contentId")}
          value={candidate}
          disabled={disabled}
          onChange={(event) => { setPreview(null); setCandidate(event.target.value); }}
        />
      </details>
      <div role="status" aria-live="polite">
        {loading && <p className="hint">{t("library.checkingSourceContent")}</p>}
        {error && <div className="hint">{error} <button className="btn link" disabled={disabled || loading} onClick={() => setRetry((value) => value + 1)}>{t("library.retryContent")}</button></div>}
        {preview && <p className="hint">{preview.mode === "file" ? t("library.sourceContentFile") : t("library.sourceContentChapters", { count: preview.chapter_count })}</p>}
      </div>
      <button
        disabled={disabled || !preview || loading || added}
        onClick={() => {
          if (preview) onPick({ ...pick, item: { ...pick.item, id: preview.id } });
        }}
      >
        {t(added ? "library.sourceContentSelected" : "library.addSourceContent")}
      </button>
    </div>
  );
}
