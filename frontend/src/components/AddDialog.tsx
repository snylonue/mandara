// Add either one book with independently selected content, or an explicit series.
// Plugin series selection previews members; only checked books are imported,
// as metadata without files. Manual series creation remains metadata-only.

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { ExtMetaForm, type ExtRecord } from "./ExtMetaForm";
import { Modal } from "./Modal";
import { PluginContentChoice } from "./PluginContentChoice";
import { SourceSearchPane, type SourcePick, type SeriesSourcePick } from "./SourceSearch";
import { useToast } from "./toast";
import type {
  AcquireResult,
  PluginInstance,
  SeriesBrief,
  Visibility,
} from "../types";

export type AddMode = "book" | "series";
type MetaSource = "file" | "manual" | "plugin";

/** One content item of the book-mode ② section: an uploaded file or a
 *  plugin book, each with its own file settings (visibility + label). */
type ContentEntry = {
  kind: "file" | "plugin";
  file?: File;
  pick?: SourcePick;
  visibility: Visibility;
  label: string;
};

/** Keep the component identity stable while editing per-version settings. */
function ItemSettings({ entry, onChange }: {
  entry: ContentEntry;
  onChange: (patch: Partial<ContentEntry>) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="row item-settings">
      <select
        value={entry.visibility}
        onChange={(e) => onChange({ visibility: e.target.value as Visibility })}
        aria-label={t("library.visibilityLabel")}
      >
        <option value="private">{t("library.visibilityPrivate")}</option>
        <option value="public">{t("library.visibilityPublic")}</option>
      </select>
      <input
        placeholder={t("library.labelPlaceholder")}
        value={entry.label}
        onChange={(e) => onChange({ label: e.target.value })}
      />
    </div>
  );
}

const splitAuthors = (raw: string) =>
  raw
    .split(/[,，]/)
    .map((s) => s.trim())
    .filter(Boolean);

export function AddDialog({
  open,
  onClose,
  instances,
  onAdded,
  onSeriesCreated,
  initialMode = "book",
}: {
  open: boolean;
  onClose: () => void;
  instances: PluginInstance[];
  onAdded: () => void;
  /** Optional: navigate to a freshly created series (like the old
   *  CreateSeriesDialog did). */
  onSeriesCreated?: (s: SeriesBrief) => void;
  /** Which entry kind the dialog opens with (添加书籍 vs 新建系列). */
  initialMode?: AddMode;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [mode, setMode] = useState<AddMode>(initialMode);
  // Book mode: metadata source (①) and content (②).
  const [meta, setMeta] = useState<MetaSource>("file");
  const [metadataFile, setMetadataFile] = useState<File | null>(null);
  // Series mode: metadata source.
  const [seriesMeta, setSeriesMeta] = useState<Exclude<MetaSource, "file">>("manual");

  const [uploading, setUploading] = useState(false);
  // Progress of a multi-content submission: `uploadDone`/`uploadTotal`.
  const [uploadDone, setUploadDone] = useState(0);
  const [uploadTotal, setUploadTotal] = useState(0);
  // Selected content items (uploaded files + plugin books), each with its
  // own visibility/label.
  const [contents, setContents] = useState<ContentEntry[]>([]);
  const [dialogError, setDialogError] = useState<string | null>(null);

  // Plugin pick for the metadata source (①).
  const [pluginMetaRef, setPluginMetaRef] = useState<SourcePick | null>(null);
  const [pluginSeriesRef, setPluginSeriesRef] = useState<SeriesSourcePick | null>(null);
  const [selectedVolumes, setSelectedVolumes] = useState<string[]>([]);

  // Manual metadata fields.
  const [title, setTitle] = useState("");
  const [authors, setAuthors] = useState("");
  const [description, setDescription] = useState("");
  const [ext, setExt] = useState<ExtRecord>({});
  // Manual series fields.
  const [seriesTitle, setSeriesTitle] = useState("");
  const [seriesAuthors, setSeriesAuthors] = useState("");
  const [seriesDescription, setSeriesDescription] = useState("");
  const [seriesExt, setSeriesExt] = useState<ExtRecord>({});

  const reset = () => {
    setMode(initialMode);
    setMeta("file");
    setMetadataFile(null);
    setSeriesMeta("manual");
    setContents([]);
    setDialogError(null);
    setPluginMetaRef(null);
    setPluginSeriesRef(null);
    setSelectedVolumes([]);
    setTitle("");
    setAuthors("");
    setDescription("");
    setExt({});
    setSeriesTitle("");
    setSeriesAuthors("");
    setSeriesDescription("");
    setSeriesExt({});
    setUploadDone(0);
    setUploadTotal(0);
  };

  // Reset whenever the dialog opens (fresh form each time).
  useEffect(() => {
    if (open) reset();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  /** Adopt a picked source entry as the metadata source (①). */
  function usePluginMeta(pick: SourcePick) {
    setPluginMetaRef(pick);
    setDialogError(null);
  }

  function clearPluginSeries() {
    setPluginSeriesRef(null);
    setSelectedVolumes([]);
    setDialogError(null);
  }

  function usePluginSeries(pick: SeriesSourcePick) {
    setPluginSeriesRef(pick);
    setSelectedVolumes([]);
    setDialogError(null);
  }

  /** Add a picked source entry as one more content item (②). */
  function usePluginBook(pick: SourcePick) {
    setContents((prev) => {
      const dup = prev.some(
        (c) =>
          c.kind === "plugin" &&
          c.pick!.instance.id === pick.instance.id &&
          c.pick!.item.id === pick.item.id,
      );
      return dup
        ? prev
        : [...prev, { kind: "plugin", pick, visibility: "private", label: "" }];
    });
    setDialogError(null);
  }

  /** Local uploads supply both metadata and content through the same entry list. */
  function addFiles(files: File[], asMetadata = false) {
    if (asMetadata && files[0]) setMetadataFile(files[0]);
    setContents((prev) => {
      // Replacing the local source replaces its automatically selected content.
      const retained = asMetadata && files[0] && metadataFile
        ? prev.filter((entry) => entry.file !== metadataFile)
        : prev;
      return [...retained, ...files.filter((file) => !retained.some((entry) => entry.file === file)).map((file) => ({
        kind: "file" as const, file, visibility: "private" as Visibility, label: "",
      }))];
    });
    setDialogError(null);
  }

  function removeContent(index: number) {
    if (contents[index].file === metadataFile) setMetadataFile(null);
    setContents((prev) => prev.filter((_, i) => i !== index));
  }

  /** Update one content item's settings. */
  function patchContent(i: number, patch: Partial<ContentEntry>) {
    setContents((prev) => prev.map((c, j) => (j === i ? { ...c, ...patch } : c)));
  }

  /** Build the metadata form fields for the *creating* submission. */
  function appendMetadataFields(form: FormData) {
    if (meta === "plugin") {
      form.append("meta_plugin_source", pluginMetaRef!.instance.id);
      form.append("meta_plugin_book_id", pluginMetaRef!.item.id);
    } else if (meta === "manual") {
      if (title.trim()) form.append("title", title.trim());
      if (authors.trim()) form.append("authors", JSON.stringify(splitAuthors(authors)));
      if (description.trim()) form.append("description", description.trim());
      if (Object.keys(ext).length > 0) form.append("ext", JSON.stringify(ext));
    }
  }

  /** One `POST /api/books` with one content item's settings + content. */
  async function acquire(entry: ContentEntry | null, extra: (form: FormData) => void): Promise<AcquireResult> {
    const form = new FormData();
    if (entry) {
      form.append("visibility", entry.visibility);
      if (entry.label.trim()) form.append("label", entry.label.trim());
    }
    extra(form);
    return api<AcquireResult>("/books", { method: "POST", body: form });
  }

  async function submitBook() {
    // Validation.
    if (meta === "plugin" && !pluginMetaRef) {
      setDialogError(t("library.pickMetaFromPlugin"));
      return;
    }
    if (meta === "file" && !metadataFile) {
      setDialogError(t("library.pickMetadataFile"));
      return;
    }
    // The chosen local metadata file must create the entry, even after reordering.
    const submittedContents = meta === "file"
      ? [...contents.filter((entry) => entry.file === metadataFile), ...contents.filter((entry) => entry.file !== metadataFile)]
      : contents;
    const contentCount = submittedContents.length;
    if (contentCount === 0 && meta === "manual" && !title.trim()) {
      setDialogError(t("library.manualNeedsTitle"));
      return;
    }

    setUploading(true);
    setDialogError(null);
    setUploadTotal(contentCount);
    setUploadDone(0);
    try {
      // Creating submission (must succeed — the entry must exist). With
      // no content at all this is a metadata-only entry (`metadata_only`).
      const firstItem = submittedContents[0] ?? null;
      let bookId: string | null = null;
      let firstTitle = "";
      const first = await acquire(firstItem, (form) => {
        appendMetadataFields(form);
        if (firstItem) {
          if (firstItem.kind === "file") form.append("file", firstItem.file!);
          else {
            form.append("plugin_source", firstItem.pick!.instance.id);
            form.append("plugin_book_id", firstItem.pick!.item.id);
          }
        } else {
          form.append("metadata_only", "true");
        }
      });
      setUploadDone(firstItem ? 1 : 0);
      // A multi-volume plugin book splits into a series + one book per
      // 卷 — a complete acquisition; further content cannot be attached
      // to a meaningful single entry, so close.
      if (first.series && first.books.length > 1) {
        toast.push(
          "success",
          t("library.seriesAdded", {
            title: first.series.title,
            count: first.books.length,
          }),
        );
        onSeriesCreated?.(first.series);
        reset();
        onClose();
        return;
      }
      bookId = first.books[0]?.book.id ?? null;
      firstTitle = first.books[0]?.book.title ?? "";
      if (!bookId) throw new Error(t("library.uploadFailed"));

      // Attach the remaining content; failures are collected and
      // reported, the rest still lands.
      let okCount = contentCount;
      for (let i = 1; i < submittedContents.length; i++) {
        const entry = submittedContents[i];
        try {
          await acquire(entry, (form) => {
            form.append("book_id", bookId!);
            if (entry.kind === "file") form.append("file", entry.file!);
            else {
              form.append("plugin_source", entry.pick!.instance.id);
              form.append("plugin_book_id", entry.pick!.item.id);
            }
          });
          setUploadDone(i + 1);
        } catch {
          okCount--;
        }
      }

      const failed = contentCount - okCount;
      if (failed > 0) {
        toast.push(
          "warning",
          t("library.addedPartial", { ok: okCount, failed }),
        );
      } else if (contentCount > 0) {
        toast.push(
          "success",
          t("library.addedBatch", {
            title: firstTitle,
            count: contentCount,
          }),
        );
      } else {
        toast.push("success", t("library.added", { title: firstTitle }));
      }
      reset();
      onClose();
      onAdded();
    } catch (err) {
      setDialogError(err instanceof Error ? err.message : t("library.uploadFailed"));
    } finally {
      setUploading(false);
    }
  }

  async function submitSeries() {
    if (seriesMeta === "manual" && !seriesTitle.trim()) {
      setDialogError(t("library.manualNeedsTitle"));
      return;
    }
    if (seriesMeta === "plugin" && !pluginSeriesRef) {
      setDialogError(t("library.pickMetaFromSeries"));
      return;
    }
    setUploading(true);
    setDialogError(null);
    try {
      let series: SeriesBrief;
      if (seriesMeta === "plugin") {
        // Import the series itself and only explicitly checked book metadata.
        series = await api<SeriesBrief>(`/plugins/${pluginSeriesRef!.instance.id}/series`, {
          method: "POST",
          body: JSON.stringify({ series_id: pluginSeriesRef!.item.id, book_ids: selectedVolumes }),
        });
        toast.push("success", t("series.created", { title: series.title }));
        onSeriesCreated?.(series);
      } else {
        const body: Record<string, unknown> = { title: seriesTitle.trim() };
        if (seriesAuthors.trim()) body.authors = splitAuthors(seriesAuthors);
        if (seriesDescription.trim()) body.description = seriesDescription.trim();
        if (Object.keys(seriesExt).length > 0) body.ext = seriesExt;
        series = await api<SeriesBrief>("/series", {
          method: "POST",
          body: JSON.stringify(body),
        });
        toast.push("success", t("series.created", { title: series.title }));
        onSeriesCreated?.(series);
      }
      reset();
      onClose();
      onAdded();
    } catch (err) {
      setDialogError(err instanceof Error ? err.message : t("common.failed_verb"));
    } finally {
      setUploading(false);
    }
  }

  function close() {
    if (uploading) return;
    reset();
    onClose();
  }

  const inBookMode = mode === "book";
  const contentCount = contents.length;
  const fileCount = contents.filter((c) => c.kind === "file").length;
  const metadataPane = (
    <SourceSearchPane
      key="book-metadata"
      instances={instances}
      filter={(i) => i.enabled && i.capabilities.includes("lookup")}
      disabled={uploading}
      pickLabel={() => t("library.useAsMetadataSource")}
      onPick={usePluginMeta}
      onClear={() => setPluginMetaRef(null)}
    />
  );
  const seriesPane = (
    <SourceSearchPane
      key="series-metadata"
      target="series"
      disabled={uploading}
      instances={instances}
      filter={(i) => i.enabled && i.capabilities.includes("lookup")}
      pickLabel={() => t("library.useAsSeriesSource")}
      onPick={usePluginSeries}
      onClear={clearPluginSeries}
    />
  );

  return (
    <Modal
      open={open}
      onClose={close}
      title={inBookMode ? t("library.addBook") : t("series.create")}
      wide
    >
      <fieldset className="add-form" disabled={uploading}>
        {/* Top switch: what are we adding? */}
        <section className="modal-section">
          <h3>{t("library.sectionWhat")}</h3>
          <div className="row">
            <label className="radio">
              <input type="radio" checked={inBookMode} onChange={() => { setMode("book"); setDialogError(null); }} />
              {t("library.whatBook")}
            </label>
            <label className="radio">
              <input type="radio" checked={!inBookMode} onChange={() => { setMode("series"); setDialogError(null); }} />
              {t("library.whatSeries")}
            </label>
          </div>
        </section>

        {inBookMode ? (
          <>
            <section className="modal-section">
              <h3>{t("library.sectionMeta")}</h3>
              <div className="row">
                <label className="radio">
                  <input type="radio" checked={meta === "file"} onChange={() => { setMeta("file"); setDialogError(null); }} />
                  {t("library.modeFile")}
                </label>
                <label className="radio">
                  <input
                    type="radio"
                    checked={meta === "manual"}
                    onChange={() => setMeta("manual")}
                  />
                  {t("library.modeManual")}
                </label>
                <label className="radio">
                  <input
                    type="radio"
                    checked={meta === "plugin"}
                    onChange={() => setMeta("plugin")}
                  />
                  {t("library.modePlugin")}
                </label>
              </div>
              {meta === "file" && <div className="field">
                <label className="file-picker">
                  <span>{metadataFile?.name ?? t("library.pickFile")}</span>
                  <input
                    type="file" accept=".epub,.txt,.text" hidden
                    aria-label={t("library.pickMetadataFile")}
                    onChange={(event) => {
                      addFiles(Array.from(event.target.files ?? []), true);
                      event.target.value = "";
                    }}
                  />
                </label>
                <p className="hint">{t("library.modeFileHint")}</p>
              </div>}
              {meta === "plugin" && <div className="field">
                {metadataPane}
                {pluginMetaRef && <div className="source-selection">
                  <p className="strong">{t("library.selectedBook", { title: pluginMetaRef.item.title })}</p>
                  <p className="hint">{pluginMetaRef.instance.name} · {pluginMetaRef.item.authors.join(" / ")}</p>
                  {pluginMetaRef.item.description && <p className="hint">{pluginMetaRef.item.description}</p>}
                  {(pluginMetaRef.instance.capabilities.includes("content") || pluginMetaRef.instance.capabilities.includes("book-file"))
                    ? <PluginContentChoice
                        key={pluginMetaRef.instance.id + ":" + pluginMetaRef.item.id}
                        pick={pluginMetaRef}
                        disabled={uploading}
                        selectedIds={contents.filter((entry) => entry.pick?.instance.id === pluginMetaRef.instance.id).map((entry) => entry.pick!.item.id)}
                        onPick={usePluginBook}
                      />
                    : <p className="hint">{t("library.sourceMetadataOnly")}</p>}
                </div>}
              </div>}
              {meta === "manual" && (
                <div className="field">
                  <input
                    placeholder={t("library.bookPlaceholder")}
                    value={title}
                    onChange={(e) => setTitle(e.target.value)}
                  />
                  <input
                    placeholder={t("library.authorsPlaceholder")}
                    value={authors}
                    onChange={(e) => setAuthors(e.target.value)}
                  />
                  <textarea
                    placeholder={t("library.descriptionPlaceholder")}
                    value={description}
                    onChange={(e) => setDescription(e.target.value)}
                  />
                  <ExtMetaForm kind="book" value={ext} onChange={setExt} />
                  <p className="hint">{t("library.modeManualHint")}</p>
                </div>
              )}
            </section>

            <section className="modal-section">
              <h3>{t("library.sectionAcquire")}</h3>

              {/* Per-item list with individual file settings */}
              {contents.length > 0 && (
                <div className="field">
                  <ul className="picker-list file-list">
                    {contents.map((entry, i) => (
                      <li key={entry.kind === "file" ? `f-${i}` : `p-${entry.pick!.instance.id}-${entry.pick!.item.id}`}>
                        <div className="file-item-head">
                          <span className="strong">
                            {entry.kind === "file"
                              ? entry.file!.name
                              : entry.pick!.item.title || entry.pick!.item.id}
                          </span>
                          <span className="hint">
                            {entry.kind === "plugin"
                              ? `${entry.pick!.instance.name} · ${entry.pick!.item.id}`
                              : ""}
                            {meta === "file" && entry.file === metadataFile ? ` · ${t("library.fileMetadataSource")}` : ""}
                          </span>
                          <button
                            className="btn link danger"
                            onClick={() => removeContent(i)}
                          >
                            {t("library.removeFile")}
                          </button>
                        </div>
                        <ItemSettings entry={entry} onChange={(patch) => patchContent(i, patch)} />
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              <details className="additional-content">
                <summary>{t("library.otherContents")}</summary>
                {/* Additional uploads share the same selected metadata. */}
                <div className="field">
                  <label className="file-picker">
                    <span>
                      {fileCount > 0
                        ? t("library.filesSelected", { count: fileCount })
                        : t("library.pickFile")}
                    </span>
                    <input
                      type="file"
                      accept=".epub,.txt,.text"
                      multiple
                      hidden
                      aria-label={t("library.tabUpload")}
                      onChange={(e) => {
                        const picked = Array.from(e.target.files ?? []);
                        addFiles(picked, meta === "file" && !metadataFile);
                        e.target.value = "";
                      }}
                    />
                  </label>
                </div>

                {/* Other content providers must advertise content capabilities. */}
                <div className="field">
                  <p className="strong">{t("library.tabPlugin")}</p>
                  <SourceSearchPane
                    instances={instances}
                    filter={(i) =>
                      i.enabled &&
                      (i.capabilities.includes("content") || i.capabilities.includes("book-file")) &&
                      (i.capabilities.includes("search") || i.capabilities.includes("lookup"))
                    }
                    pickLabel={() => t("library.pickThisBook")}
                    disabled={uploading}
                    onPick={(pick) => usePluginBook(pick)}
                  />
                </div>
              </details>
              <p className="hint">{t("library.contentBothHint")}</p>
              {contentCount === 0 && <p className="hint">{t("library.tabMetaOnlyHint")}</p>}
            </section>
          </>
        ) : (
          <section className="modal-section">
            <h3>{t("library.sectionSeriesMeta")}</h3>
            <div className="row">
              <label className="radio">
                <input
                  type="radio"
                  checked={seriesMeta === "manual"}
                  onChange={() => setSeriesMeta("manual")}
                />
                {t("library.modeManual")}
              </label>
              <label className="radio">
                <input
                  type="radio"
                  checked={seriesMeta === "plugin"}
                  onChange={() => setSeriesMeta("plugin")}
                />
                {t("library.modePlugin")}
              </label>
            </div>
            {seriesMeta === "manual" && (
              <div className="field">
                <input
                  placeholder={t("book.seriesTitlePlaceholder")}
                  value={seriesTitle}
                  onChange={(e) => setSeriesTitle(e.target.value)}
                />
                <input
                  placeholder={t("library.authorsPlaceholder")}
                  value={seriesAuthors}
                  onChange={(e) => setSeriesAuthors(e.target.value)}
                />
                <textarea
                  placeholder={t("library.descriptionPlaceholder")}
                  value={seriesDescription}
                  onChange={(e) => setSeriesDescription(e.target.value)}
                />
                <ExtMetaForm kind="series" value={seriesExt} onChange={setSeriesExt} />
                <p className="hint">{t("series.createHint")}</p>
              </div>
            )}
            {seriesMeta === "plugin" && <div className="field">{seriesPane}</div>}
            {seriesMeta === "plugin" && pluginSeriesRef && (
              <div className="field">
                <p className="strong">{t("library.selectedSeries", { title: pluginSeriesRef.item.title })}</p>
                <p className="hint">{pluginSeriesRef.item.authors.join(" / ")}</p>
                {pluginSeriesRef.item.description && <p className="hint">{pluginSeriesRef.item.description}</p>}
                <h3>{t("library.seriesMemberSelection", { count: selectedVolumes.length })}</h3>
                <p className="hint">{t("library.seriesMetadataOnlyHint")}</p>
                {pluginSeriesRef.books.length > 0 && <div className="row">
                  <button disabled={uploading} onClick={() => setSelectedVolumes(pluginSeriesRef.books.map((book) => book.id))}>{t("library.selectAllVolumes")}</button>
                  <button disabled={uploading} onClick={() => setSelectedVolumes([])}>{t("library.clearVolumes")}</button>
                </div>}
                <ul className="source-results">
                  {pluginSeriesRef.books.map((book) => (
                    <li key={book.id} className="source-result">
                      <label className="radio">
                        <input
                          type="checkbox"
                          disabled={uploading}
                          checked={selectedVolumes.includes(book.id)}
                          onChange={(e) => {
                            const checked = e.target.checked;
                            setSelectedVolumes((prev) => checked ? [...prev, book.id] : prev.filter((id) => id !== book.id));
                          }}
                        />
                        <span>{book.title}</span>
                      </label>
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </section>
        )}

        {dialogError && <div className="error">{dialogError}</div>}
        <div className="modal-actions">
          <button onClick={close}>{t("library.cancel")}</button>
          {inBookMode ? (
            <button className="btn primary" onClick={() => void submitBook()} disabled={uploading}>
              {uploading
                ? uploadTotal > 0
                  ? t("library.uploadingProgress", { done: uploadDone, total: uploadTotal })
                  : t("library.uploading")
                : contentCount > 1
                  ? t("library.addContents", { count: contentCount })
                  : t(contentCount === 0 && meta !== "file" ? "library.addMetadataOnly" : "library.addBook")}
            </button>
          ) : (
            <button className="btn primary" onClick={() => void submitSeries()} disabled={uploading || (seriesMeta === "plugin" && !pluginSeriesRef)}>
              {uploading ? t("library.uploading") : t("series.create")}
            </button>
          )}
        </div>
      </fieldset>
    </Modal>
  );
}
