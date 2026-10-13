// Attach matched editions to existing books; never import or replace their metadata.
import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useAuth } from "../auth";
import type { AcquireResult, BookDetail, SeriesBrief, User } from "../types";
import { Modal } from "./Modal";
import { SeriesContentMatch, type SeriesContentPlan } from "./SeriesContentMatch";
import { usePluginInstances } from "./SourceSearch";
import { useToast } from "./toast";

/** Attachment requires a visible book, not permission to edit its metadata. */
export function canAttachVolume(entry: BookDetail, user: User | null): boolean {
  return Boolean(user && (user.role === "admin" || entry.book.created_by === user.id || entry.files.length > 0));
}

/** Mounted once per draft, using a snapshot of the saved series order. */
export function SeriesBatchContentDialog({ series, books, onClose, onChanged }: {
  series: SeriesBrief;
  books: BookDetail[];
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const toast = useToast();
  const instances = usePluginInstances();
  const eligible = useMemo(() => new Set(books.filter((entry) => canAttachVolume(entry, user)).map((entry) => entry.book.id)), [books, user]);
  const missingIds = books.filter((entry) => eligible.has(entry.book.id) && entry.files.length === 0).map((entry) => entry.book.id);
  const [selectedIds, setSelectedIds] = useState<string[]>(missingIds);
  const selected = useMemo(() => selectedIds.filter((id) => eligible.has(id)), [selectedIds, eligible]);
  const targets = useMemo(() => books.map((entry, index) => {
    const volume = t("series.volumeLabel", { volume: entry.book.volume_no || index + 1 });
    return { id: entry.book.id, title: entry.book.title === series.title ? volume : volume + " · " + entry.book.title };
  }), [books, series.title, t]);
  const [plan, setPlan] = useState<SeriesContentPlan>({ ready: true, assignments: [] });
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [started, setStarted] = useState(false);
  const attempted = useRef(false);
  const [doneIds, setDoneIds] = useState<string[]>([]);
  const [failures, setFailures] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const locked = busy || started;
  const assignments = plan.assignments.filter((entry) => entry.content && eligible.has(entry.targetId));
  const remaining = assignments.filter((entry) => !doneIds.includes(entry.targetId));

  function finish() {
    onClose();
    if (attempted.current) {
      attempted.current = false;
      void onChanged().catch((err) => toast.push("error", err instanceof Error ? err.message : t("common.failed")));
    }
  }
  function close() { if (!busyRef.current) finish(); }
  async function submit() {
    if (busyRef.current || !plan.ready || remaining.length === 0) return;
    busyRef.current = true;
    setBusy(true);
    setStarted(true);
    setError(null);
    setFailures({});
    const failed: Record<string, string> = {};
    try {
      for (const entry of remaining) {
        const content = entry.content!;
        const form = new FormData();
        // Only the local book ID identifies the target. Provider IDs are content-only.
        form.append("book_id", entry.targetId);
        form.append("visibility", entry.visibility);
        if (entry.label.trim()) form.append("label", entry.label.trim());
        if (content.kind === "file") form.append("file", content.file);
        else {
          form.append("plugin_source", content.source);
          form.append("plugin_book_id", content.id);
        }
        attempted.current = true;
        try {
          const result = await api<AcquireResult>("/books", { method: "POST", body: form });
          if (!result.books.find((book) => book.book.id === entry.targetId)?.files.length) throw new Error(t("book.attachNoFile"));
          setDoneIds((prev) => [...prev, entry.targetId]);
        } catch (err) {
          failed[entry.targetId] = err instanceof Error ? err.message : t("common.failed");
        }
      }
      if (Object.keys(failed).length > 0) {
        setFailures(failed);
        setError(t("series.batchContentPartial", { failed: Object.keys(failed).length }));
      } else {
        toast.push("success", t("series.batchContentAdded", { count: assignments.length }));
        finish();
      }
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }
  return (
    <Modal open onClose={close} title={t("series.batchContentTitle")} wide>
      <p className="hint">{t("series.batchContentHint")}</p>
      <fieldset className="add-form" disabled={locked}>
        <div className="row">
          <button onClick={() => setSelectedIds(missingIds)}>{t("series.batchSelectMissing")}</button>
          <button onClick={() => setSelectedIds([...eligible])}>{t("library.selectAllVolumes")}</button>
          <button onClick={() => setSelectedIds([])}>{t("library.clearVolumes")}</button>
        </div>
        <ul className="source-results">
          {targets.map((target, index) => <li key={target.id} className="source-result">
            <label className="radio">
              <input type="checkbox" disabled={!eligible.has(target.id)} checked={selected.includes(target.id)}
                onChange={(event) => setSelectedIds((prev) => event.target.checked ? [...prev, target.id] : prev.filter((id) => id !== target.id))} />
              <span>{target.title}</span>
            </label>
            <span className="hint">{!eligible.has(target.id) ? t("series.batchNoPermission")
              : t("series.batchExistingFiles", { count: books[index].files.length })}</span>
          </li>)}
        </ul>
      </fieldset>
      <SeriesContentMatch targets={targets} selectedIds={selected} instances={instances} disabled={locked}
        completedIds={doneIds} failures={failures} onChange={setPlan}
        heading={t("series.batchContentSource")} skipLabel={t("series.batchSkipContent")} />
      {error && <p className="error" role="alert">{error}</p>}
      <div className="modal-actions">
        <button disabled={busy} onClick={close}>{t("common.close")}</button>
        <button className="btn primary" disabled={busy || !plan.ready || remaining.length === 0} onClick={() => void submit()}>
          {busy ? t("library.uploadingProgress", { done: doneIds.length, total: assignments.length })
            : started ? t("library.seriesRetryFailedContent") : t("series.batchAddContent", { count: remaining.length })}
        </button>
      </div>
    </Modal>
  );
}
