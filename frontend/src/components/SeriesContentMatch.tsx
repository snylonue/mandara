// Match independent content candidates to explicitly selected metadata members.
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import type { BookMeta, PluginContentPreview, PluginInstance, Visibility } from "../types";
import { SourceSearchPane, type SeriesSourcePick } from "./SourceSearch";

export type SeriesContentTarget = Pick<BookMeta, "id" | "title">;

export type SeriesContentAssignment = {
  targetId: string;
  content: { kind: "file"; file: File } | { kind: "plugin"; source: string; id: string } | null;
  visibility: Visibility;
  label: string;
};
export type SeriesContentPlan = { ready: boolean; assignments: SeriesContentAssignment[] };
type Candidate = {
  key: string;
  title: string;
  file?: File;
  source?: string;
  candidateId: string;
  contentOption?: boolean;
  revision?: number;
  preview: PluginContentPreview | null;
  loading: boolean;
  error: string | null;
};
type Settings = { visibility: Visibility; label: string };
const defaults: Settings = { visibility: "private", label: "" };

/** Confirm declarations only; never acquire content while adjusting the draft. */
function CandidatePreview({ candidate, disabled, patch, load }: {
  candidate: Candidate;
  disabled: boolean;
  patch: (key: string, value: Partial<Candidate>, expected?: Candidate) => void;
  load: (source: string, id: string, signal: AbortSignal) => Promise<PluginContentPreview>;
}) {
  const { t } = useTranslation();
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    patch(candidate.key, { preview: null, loading: Boolean(candidate.candidateId.trim()), error: null }, candidate);
    if (!candidate.candidateId.trim()) return () => controller.abort();
    const timer = window.setTimeout(() => {
      load(candidate.source!, candidate.candidateId.trim(), controller.signal)
        .then((preview) => { if (!controller.signal.aborted) patch(candidate.key, { preview, loading: false }, candidate); })
        .catch((error) => {
          if (!controller.signal.aborted) patch(candidate.key, {
            loading: false,
            error: error instanceof Error && error.message ? error.message : t("common.failed"),
          }, candidate);
        });
    }, 250);
    return () => { window.clearTimeout(timer); controller.abort(); };
  }, [candidate.key, candidate.source, candidate.candidateId, candidate.revision, retry, patch, load, t]);
  return (
    <li className="series-match-row">
      <span className="strong">{candidate.title}</span>
      {!candidate.contentOption && <label className="field">
        <span>{t("library.contentId")}</span>
        <input aria-label={t("library.seriesContentIdFor", { title: candidate.title })} disabled={disabled} value={candidate.candidateId}
          onChange={(event) => patch(candidate.key, { candidateId: event.target.value, preview: null, loading: true, error: null })} />
      </label>}
      <div role="status" className="hint">
        {candidate.loading ? t("library.checkingSourceContent") : candidate.error
          ?? (candidate.preview ? candidate.preview.mode === "file" ? t("library.sourceContentFile")
            : t("library.sourceContentChapters", { count: candidate.preview.chapter_count }) : t("library.seriesUnconfirmedContent"))}
      </div>
      {candidate.error && <button className="btn link" disabled={disabled} onClick={() => setRetry((value) => value + 1)}>{t("library.retryContent")}</button>}
    </li>
  );
}

export function SeriesContentMatch({ targets, selectedIds, instances, disabled, completedIds, failures, onChange, heading, skipLabel }: {
  targets: SeriesContentTarget[];
  heading?: string;
  skipLabel?: string;
  selectedIds: string[];
  instances: PluginInstance[];
  disabled: boolean;
  completedIds: string[];
  failures: Record<string, string>;
  onChange: (plan: SeriesContentPlan) => void;
}) {
  const { t } = useTranslation();
  const [mode, setMode] = useState<"plugin" | "file">("plugin");
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [matches, setMatches] = useState<Record<string, string>>({});
  const [settings, setSettings] = useState<Record<string, Settings>>({});
  const [contentTitle, setContentTitle] = useState("");
  const nextFile = useRef(0);
  const nextPick = useRef(0);
  const [sourceLoading, setSourceLoading] = useState(false);
  const previewLanes = useRef<Promise<unknown>[]>(Array.from({ length: 4 }, () => Promise.resolve()));
  const nextLane = useRef(0);
  // Bound expensive plugin previews, and discard aborted work before dispatch.
  const load = useCallback((source: string, id: string, signal: AbortSignal) => {
    const lane = nextLane.current++ % previewLanes.current.length;
    const request = previewLanes.current[lane].then(() => {
      signal.throwIfAborted();
      const params = new URLSearchParams({ q: id });
      return api<PluginContentPreview>("/plugins/" + encodeURIComponent(source) + "/content?" + params, { signal });
    });
    previewLanes.current[lane] = request.then(() => undefined, () => undefined);
    return request;
  }, []);
  const patch = useCallback((key: string, value: Partial<Candidate>, expected?: Candidate) => {
    setCandidates((prev) => prev.map((candidate) => candidate.key === key
      && (!expected || (candidate.candidateId === expected.candidateId && candidate.revision === expected.revision))
      ? { ...candidate, ...value } : candidate));
  }, []);
  function matchInOrder(items: Candidate[]) {
    return Object.fromEntries(targets.map((book, index) => [book.id, items[index]?.key ?? ""]));
  }
  function chooseCandidates(items: Candidate[], title: string) {
    setCandidates(items);
    setMatches(matchInOrder(items));
    setContentTitle(title);
  }
  function clear() { chooseCandidates([], ""); }
  function chooseSeries(pick: SeriesSourcePick) {
    const revision = ++nextPick.current;
    const seen = new Set<string>();
    const items = pick.books.filter((book) => {
      if (seen.has(book.id)) return false;
      seen.add(book.id);
      return true;
    }).map((book) => ({
      key: pick.instance.id + ":" + book.id, title: book.title, source: pick.instance.id,
      candidateId: book.id, contentOption: pick.instance.capabilities.includes("content-list"), revision, preview: null, loading: true, error: null,
    }));
    chooseCandidates(items, pick.item.title);
  }
  function changeMatch(targetId: string, key: string) {
    setMatches((prev) => {
      const next = { ...prev, [targetId]: key };
      // Swap instead of silently binding the same candidate to two selected volumes.
      const other = key && selectedIds.find((id) => id !== targetId && prev[id] === key);
      if (other) next[other] = prev[targetId] ?? "";
      return next;
    });
  }
  const selected = targets.filter((book) => selectedIds.includes(book.id));
  const signatures = new Set<string>();
  let ready = !sourceLoading;
  let duplicate = false;
  const assignments = selected.map((book): SeriesContentAssignment => {
    const candidate = candidates.find((item) => item.key === matches[book.id]);
    let content: SeriesContentAssignment["content"] = null;
    if (candidate) {
      if (candidate.file) content = { kind: "file", file: candidate.file };
      else if (candidate.preview && !candidate.loading && !candidate.error) content = { kind: "plugin", source: candidate.source!, id: candidate.preview.id };
      else ready = false;
      const signature = candidate.file ? candidate.key : candidate.source + ":" + candidate.preview?.id;
      if (content && signatures.has(signature)) { duplicate = true; ready = false; }
      if (content) signatures.add(signature);
    }
    const value = settings[book.id] ?? defaults;
    return { targetId: book.id, content, ...value, visibility: content?.kind === "plugin" ? "public" : value.visibility };
  });
  // The parent owns submission; report the draft only when its inputs change.
  useLayoutEffect(() => {
    onChange({ ready, assignments });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [candidates, matches, settings, targets, selectedIds, sourceLoading, onChange]);
  return (
    <section className="modal-section">
      <h3>{heading ?? t("library.sectionSeriesContent")}</h3>
      <p className="hint">{t("library.seriesContentMatchHint")}</p>
      <fieldset className="add-form" disabled={disabled}>
        <div className="row">
          <label className="radio"><input type="radio" checked={mode === "plugin"} onChange={() => { setMode("plugin"); setSourceLoading(false); clear(); }} />{t("library.tabPlugin")}</label>
          <label className="radio"><input type="radio" checked={mode === "file"} onChange={() => { setMode("file"); setSourceLoading(false); clear(); }} />{t("library.tabUpload")}</label>
        </div>
        {mode === "plugin" ? <SourceSearchPane
          purpose="content" target="series" instances={instances} disabled={disabled} onLoadingChange={setSourceLoading}
          filter={(instance) => instance.enabled && instance.capabilities.includes("lookup")
            && (instance.capabilities.includes("content") || instance.capabilities.includes("book-file"))}
          pickLabel={() => t("library.useAsSeriesContentSource")} onPick={chooseSeries} onClear={clear}
        /> : <label className="file-picker">
          <span>{t("library.pickFile")}</span>
          <input type="file" accept=".epub,.txt,.text" multiple hidden aria-label={t("library.seriesContentFiles")}
            onChange={(event) => {
              const items = Array.from(event.target.files ?? []).map((file) => ({ key: "file:" + nextFile.current++, title: file.name,
                file, candidateId: "", preview: null, loading: false, error: null }));
              chooseCandidates(items, ""); event.target.value = "";
            }} />
        </label>}
        {contentTitle && <p className="strong">{t("library.seriesContentSelected", { title: contentTitle })}</p>}
        {contentTitle && candidates.length === 0 && <p className="hint">{t("library.seriesContentEmpty")}</p>}
        {candidates.length > 0 && <>
          <div className="row">
            <button onClick={() => setMatches(matchInOrder(candidates))}>{t("library.seriesMatchByOrder")}</button>
            <button className="btn link" onClick={clear}>{t("library.seriesClearContent")}</button>
          </div>
          {candidates.length !== targets.length && <p className="hint">{t("library.seriesContentCountMismatch", { metadata: targets.length, content: candidates.length })}</p>}
          <details className="additional-content">
            <summary>{t("library.seriesContentCandidates", { count: candidates.length })}</summary>
            <ul className="source-results">
              {candidates.map((candidate) => candidate.file ? <li key={candidate.key} className="series-match-row">{candidate.title}</li>
                : <CandidatePreview key={candidate.key} candidate={candidate} disabled={disabled} patch={patch} load={load} />)}
            </ul>
          </details>
        </>}
        {selected.length > 0 && <ul className="source-results series-content-matches">
          {selected.map((book) => {
            const candidate = candidates.find((item) => item.key === matches[book.id]);
            const value = settings[book.id] ?? defaults;
            return <li key={book.id} className="series-match-row">
              <label className="field">
                <span className="strong">{book.title}</span><span className="hint">{book.id}</span>
                <select aria-label={t("library.seriesMatchFor", { title: book.title })} value={matches[book.id] ?? ""} onChange={(event) => changeMatch(book.id, event.target.value)}>
                  <option value="">{skipLabel ?? t("library.seriesNoContent")}</option>
                  {candidates.map((item, index) => <option key={item.key} value={item.key}>{index + 1}. {item.title}</option>)}
                </select>
              </label>
              {candidate && <>
                <div className="hint">{candidate.file ? candidate.file.name : candidate.contentOption ? candidate.title : candidate.preview?.id ?? candidate.candidateId}</div>
                {!candidate.file && !candidate.preview && <p className="hint">{candidate.error ?? t(candidate.loading ? "library.checkingSourceContent" : "library.seriesUnconfirmedContent")}</p>}
                <div className="row item-settings">
                  {!candidate.file ? <span className="hint">{t("library.pluginPublicOnly")}</span> : <select aria-label={t("library.seriesVisibilityFor", { title: book.title })} value={value.visibility}
                    onChange={(event) => setSettings((prev) => ({ ...prev, [book.id]: { ...value, visibility: event.target.value as Visibility } }))}>
                    <option value="private">{t("library.visibilityPrivate")}</option><option value="public">{t("library.visibilityPublic")}</option>
                  </select>}
                  <input aria-label={t("library.seriesLabelFor", { title: book.title })} placeholder={t("library.labelPlaceholder")} value={value.label}
                    onChange={(event) => setSettings((prev) => ({ ...prev, [book.id]: { ...value, label: event.target.value } }))} />
                </div>
              </>}
              {completedIds.includes(book.id) && <p role="status" className="hint">{t("library.seriesContentDone")}</p>}
              {failures[book.id] && <p className="error">{failures[book.id]}</p>}
            </li>;
          })}
        </ul>}
        {duplicate && <p className="error">{t("library.seriesDuplicateContent")}</p>}
      </fieldset>
    </section>
  );
}
