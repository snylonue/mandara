// The unified "add" dialog (添加书籍 / 新建系列 → 一个对话框，两个预设入口).
//
// Two top-level modes, switched with the radio at the top:
//   - book mode (添加书籍): ① 选择元数据来源（手动 / 从插件源）→
//     ② 添加若干内容（上传文件 / 从插件源 — 两者可同时选择，且都能选
//     多份）；**每个内容项有自己的文件设置**（可见性 / 备注）。一次提交
//     把全部内容挂到同一书目条目下 —— 第一项内容创建元数据条目
//     （`POST /api/books`，手动字段或 `meta_plugin_*`），其余内容依次带
//     `book_id` 挂上（一个元数据 ⇔ 多文件）。一项失败不影响其余。
//     都不选则仅添加元数据（可在详情页稍后补充内容）。
//   - series mode (新建系列): ① 选择元数据来源（手动 / 从插件源）。
//     手动 → `POST /api/series`；插件 → 以内容方式获取该插件书，后端按卷
//     拆分自动生成系列（bangumi 系列条目即此路径）。
//
// The metadata source and the content source stay orthogonal, exactly like
// the backend `POST /api/books` contract (meta_plugin_* vs plugin_source).

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { ExtMetaForm, type ExtRecord } from "./ExtMetaForm";
import { Modal } from "./Modal";
import { SourceSearchPane, type SourcePick } from "./SourceSearch";
import { useToast } from "./toast";
import type {
  AcquireResult,
  PluginInstance,
  SeriesBrief,
  Visibility,
} from "../types";

export type AddMode = "book" | "series";
type MetaSource = "manual" | "plugin";

/** One content item of the book-mode ② section: an uploaded file or a
 *  plugin book, each with its own file settings (visibility + label). */
type ContentEntry = {
  kind: "file" | "plugin";
  file?: File;
  pick?: SourcePick;
  visibility: Visibility;
  label: string;
};

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
  const [meta, setMeta] = useState<MetaSource>("manual");
  // Series mode: metadata source.
  const [seriesMeta, setSeriesMeta] = useState<MetaSource>("manual");

  const [uploading, setUploading] = useState(false);
  // Progress of a multi-content submission: `uploadDone`/`uploadTotal`.
  const [uploadDone, setUploadDone] = useState(0);
  const [uploadTotal, setUploadTotal] = useState(0);
  // Selected content items (uploaded files + plugin books), each with its
  // own visibility/label.
  const [contents, setContents] = useState<ContentEntry[]>([]);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  // Plugin pick for the metadata source (①).
  const [pluginMetaRef, setPluginMetaRef] = useState<SourcePick | null>(null);

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
    setMeta("manual");
    setSeriesMeta("manual");
    setContents([]);
    setDialogError(null);
    setPluginMetaRef(null);
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
    // Manual-id picks carry no metadata (the id is a lookup key); only
    // prefill the refinement fields when the pick has metadata.
    setTitle(pick.item.title);
    setAuthors(pick.item.authors.join("，"));
    setDescription(pick.item.description ?? "");
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
    const contentCount = contents.length;
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
      const firstItem = contents[0] ?? null;
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
      for (let i = 1; i < contents.length; i++) {
        const entry = contents[i];
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
    if (seriesMeta === "plugin" && !pluginMetaRef) {
      setDialogError(t("library.pickMetaFromSeries"));
      return;
    }
    setUploading(true);
    setDialogError(null);
    try {
      let series: SeriesBrief;
      if (seriesMeta === "plugin") {
        // 从插件源: acquire the plugin book as content. A multi-volume
        // source (e.g. bangumi 系列条目) splits into a series + one book
        // per 卷; a single-volume one creates a bare metadata entry whose
        // content can be attached later.
        const form = new FormData();
        form.append("visibility", "public");
        form.append("plugin_source", pluginMetaRef!.instance.id);
        form.append("plugin_book_id", pluginMetaRef!.item.id);
        const result = await api<AcquireResult>("/books", {
          method: "POST",
          body: form,
        });
        if (result.series) {
          series = result.series;
        } else {
          // Single-volume: create a series for it and move the book in.
          const s = await api<SeriesBrief>("/series", {
            method: "POST",
            body: JSON.stringify({
              title: result.books[0]?.book.title ?? pluginMetaRef!.item.title,
              authors: result.books[0]?.book.authors ?? [],
            }),
          });
          series = s;
          const bookId = result.books[0]?.book.id;
          if (bookId) {
            await api(`/books/${bookId}`, {
              method: "PATCH",
              body: JSON.stringify({ series_id: s.id }),
            });
          }
        }
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
    reset();
    onClose();
  }

  const inBookMode = mode === "book";
  const contentCount = contents.length;
  const fileCount = contents.filter((c) => c.kind === "file").length;
  const seriesMetaPane = (
    <SourceSearchPane
      instances={instances}
      filter={(i) => i.enabled && i.capabilities.includes("lookup")}
      pickLabel={() =>
        inBookMode ? t("library.useAsMetadataSource") : t("library.useAsSeriesSource")
      }
      onPick={(pick) => usePluginMeta(pick)}
    />
  );

  /** Per-item file settings row (visibility + label). */
  function ItemSettings({ entry, index }: { entry: ContentEntry; index: number }) {
    return (
      <div className="row item-settings">
        <select
          value={entry.visibility}
          onChange={(e) => patchContent(index, { visibility: e.target.value as Visibility })}
          aria-label={t("library.visibilityLabel")}
        >
          <option value="private">{t("library.visibilityPrivate")}</option>
          <option value="public">{t("library.visibilityPublic")}</option>
        </select>
        <input
          placeholder={t("library.labelPlaceholder")}
          value={entry.label}
          onChange={(e) => patchContent(index, { label: e.target.value })}
        />
      </div>
    );
  }

  return (
    <Modal
      open={open}
      onClose={close}
      title={inBookMode ? t("library.addBook") : t("series.create")}
      wide
    >
      {/* Top switch: what are we adding? */}
      <section className="modal-section">
        <h3>{t("library.sectionWhat")}</h3>
        <div className="row">
          <label className="radio">
            <input type="radio" checked={inBookMode} onChange={() => setMode("book")} />
            {t("library.whatBook")}
          </label>
          <label className="radio">
            <input type="radio" checked={!inBookMode} onChange={() => setMode("series")} />
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
            {meta === "plugin" && <div className="field">{seriesMetaPane}</div>}
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

            {/* 上传文件 (multiple, independent of the plugin source) */}
            <div className="field">
              <p className="strong">{t("library.tabUpload")}</p>
              <label className="file-picker">
                <span>
                  {fileCount > 0
                    ? t("library.filesSelected", { count: fileCount })
                    : t("library.pickFile")}
                </span>
                <input
                  ref={fileRef}
                  type="file"
                  accept=".epub,.txt,.text"
                  multiple
                  hidden
                  onChange={(e) => {
                    const picked = Array.from(e.target.files ?? []);
                    if (picked.length) {
                      setContents((prev) => [
                        ...prev,
                        ...picked.map((file) => ({
                          kind: "file" as const,
                          file,
                          visibility: "private" as Visibility,
                          label: "",
                        })),
                      ]);
                    }
                    e.target.value = "";
                  }}
                />
              </label>
            </div>

            {/* 从插件源 (multiple books, independent of the upload) */}
            <div className="field">
              <p className="strong">{t("library.tabPlugin")}</p>
              <SourceSearchPane
                instances={instances}
                filter={(i) =>
                  i.enabled &&
                  (i.capabilities.includes("search") || i.capabilities.includes("lookup"))
                }
                pickLabel={() => t("library.pickThisBook")}
                onPick={(pick) => usePluginBook(pick)}
              />
            </div>

            {/* Per-item list with individual file settings */}
            {contents.length > 0 && (
              <div className="field">
                <ul className="picker-list file-list">
                  {contents.map((entry, i) => (
                    <li key={entry.kind === "file" ? `f-${i}` : `p-${entry.pick!.item.id}`}>
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
                          {i === 0 ? ` · ${t("library.firstCreatesEntry")}` : ` · ${t("library.attachToEntry")}`}
                        </span>
                        <button
                          className="btn link danger"
                          onClick={() => setContents((prev) => prev.filter((_, j) => j !== i))}
                        >
                          {t("library.removeFile")}
                        </button>
                      </div>
                      <ItemSettings entry={entry} index={i} />
                    </li>
                  ))}
                </ul>
              </div>
            )}

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
          {seriesMeta === "plugin" && <div className="field">{seriesMetaPane}</div>}
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
                : t("library.addBook")}
          </button>
        ) : (
          <button className="btn primary" onClick={() => void submitSeries()} disabled={uploading}>
            {uploading ? t("library.uploading") : t("series.create")}
          </button>
        )}
      </div>
    </Modal>
  );
}
