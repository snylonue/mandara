// The unified "add a book" dialog (获取书籍 → 添加元数据).
//
// Content (step ①) and metadata (step ②) are orthogonal and may come
// from different plugins:
//   step 1 (获取书籍): an uploaded file, a book picked from a plugin
//     source, or none (仅添加元数据 — a bare metadata entry whose content
//     is attached later);
//   step 2 (添加元数据): auto (parsed / the plugin's own entry), from a
//     plugin catalog (从插件源 — possibly another plugin than the content
//     source), attached to an existing entry, or manual.
//
// The submission goes to the unified endpoint POST /api/books (see the
// OpenAPI contract), which accepts a `file`, a plugin book
// (`plugin_source` + `plugin_book_id`), or `metadata_only` for the
// content, plus the metadata fields (`book_id` / `meta_plugin_source` +
// `meta_plugin_book_id` / manual overrides).

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { ExtMetaForm, type ExtRecord } from "./ExtMetaForm";
import { Modal } from "./Modal";
import { SourceSearchPane, type SourcePick } from "./SourceSearch";
import { useToast } from "./toast";
import type {
  AcquireResult,
  BookListEntry,
  PluginInstance,
  Visibility,
} from "../types";

type AcquireTab = "upload" | "plugin" | "meta-only";
type MetaMode = "auto" | "plugin" | "attach" | "manual";

const splitAuthors = (raw: string) =>
  raw
    .split(/[,，]/)
    .map((s) => s.trim())
    .filter(Boolean);

export function AddBookDialog({
  open,
  onClose,
  instances,
  onAdded,
}: {
  open: boolean;
  onClose: () => void;
  instances: PluginInstance[];
  onAdded: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [tab, setTab] = useState<AcquireTab>("upload");
  const [mode, setMode] = useState<MetaMode>("auto");
  const [uploading, setUploading] = useState(false);
  const [visibility, setVisibility] = useState<Visibility>("private");
  const [label, setLabel] = useState("");
  const [fileName, setFileName] = useState<string | null>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  // Plugin content tab: the book whose content (and, in auto mode,
  // metadata) we take.
  const [pluginAcq, setPluginAcq] = useState<SourcePick | null>(null);
  // "从插件源" metadata mode: the catalog entry whose metadata we take.
  const [pluginMetaRef, setPluginMetaRef] = useState<SourcePick | null>(null);

  // attach mode: books available to the caller
  const [bookOptions, setBookOptions] = useState<BookListEntry[]>([]);
  const [attachBookId, setAttachBookId] = useState("");

  // manual (and plugin-refinement) mode fields
  const [title, setTitle] = useState("");
  const [authors, setAuthors] = useState("");
  const [description, setDescription] = useState("");
  const [ext, setExt] = useState<ExtRecord>({});

  const reset = () => {
    setTab("upload");
    setMode("auto");
    setVisibility("private");
    setLabel("");
    setFileName(null);
    setDialogError(null);
    setAttachBookId("");
    setTitle("");
    setAuthors("");
    setDescription("");
    setExt({});
    setPluginAcq(null);
    setPluginMetaRef(null);
  };

  // Snapshot the books the caller may attach to whenever the dialog opens.
  useEffect(() => {
    if (!open) return;
    setDialogError(null);
    let cancelled = false;
    api<BookListEntry[]>("/books")
      .then((list) => {
        if (!cancelled) setBookOptions(list);
      })
      .catch(() => {
        if (!cancelled) setBookOptions([]);
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  /** Adopt a picked source entry as the plugin content (获取书籍 step). */
  function usePluginBook(pick: SourcePick) {
    setPluginAcq(pick);
    // Seed the manual fields with the plugin's own metadata, so the user
    // can refine it without retyping.
    setTitle(pick.item.title);
    setAuthors(pick.item.authors.join("，"));
    setDescription(pick.item.description ?? "");
    setDialogError(null);
  }

  /** Adopt a picked source entry as the metadata source (从插件源 mode). */
  function usePluginMeta(pick: SourcePick) {
    setPluginMetaRef(pick);
    // Manual-id picks carry no metadata (the id is a lookup key, not a
    // title — see SourceSearch). Only prefill the refinement fields when
    // the pick actually has metadata; otherwise leave them empty so the
    // real entry is fetched by the backend (`get-book`) and no bogus
    // `title` override is submitted.
    setTitle(pick.item.title);
    setAuthors(pick.item.authors.join("，"));
    setDescription(pick.item.description ?? "");
    setDialogError(null);
  }

  /** Whether a metadata mode makes sense for the active content tab. */
  function modeAvailable(m: MetaMode): boolean {
    if (tab === "meta-only") return m === "plugin" || m === "manual";
    return true;
  }

  function switchTab(next: AcquireTab) {
    if (next === tab) return;
    setTab(next);
    setDialogError(null);
    // Content/metadata refs belong to their tab; keep the metadata
    // suggestion of the newly active tab.
    setPluginAcq(next === "plugin" ? pluginAcq : null);
    // A mode that is meaningless for the new tab falls back to a default.
    if (!modeAvailable(mode)) {
      setMode(next === "meta-only" ? "plugin" : "auto");
    }
  }

  function switchMode(next: MetaMode) {
    setMode(next);
    setDialogError(null);
  }

  async function submit() {
    // validation per tab + mode
    if (tab === "upload" && !fileRef.current?.files?.[0]) {
      setDialogError(t("library.pickFile"));
      return;
    }
    if (tab === "plugin" && !pluginAcq) {
      setDialogError(t("library.pluginTabNeedsPick"));
      return;
    }
    if (mode === "attach" && !attachBookId) {
      setDialogError(t("library.pickBook"));
      return;
    }
    if (mode === "plugin" && !pluginMetaRef) {
      setDialogError(t("library.pickMetaFromPlugin"));
      return;
    }
    if (mode === "manual" && !title.trim()) {
      setDialogError(t("library.manualNeedsTitle"));
      return;
    }

    setUploading(true);
    setDialogError(null);
    try {
      const form = new FormData();
      form.append("visibility", visibility);
      if (label.trim()) form.append("label", label.trim());

      // 获取书籍: the content source (file, plugin book, or none).
      if (tab === "upload") {
        const file = fileRef.current?.files?.[0];
        if (file) form.append("file", file);
      } else if (tab === "plugin") {
        // Non-null per the validation above.
        const acq = pluginAcq!;
        form.append("plugin_source", acq.instance.id);
        form.append("plugin_book_id", acq.item.id);
      } else {
        form.append("metadata_only", "true");
      }

      // 添加元数据: where the metadata entry comes from.
      if (mode === "attach") {
        form.append("book_id", attachBookId);
      } else if (mode === "plugin") {
        // Non-null per the validation above. Distinct field names: the
        // metadata source is independent of the content source, so a
        // cross-plugin combination (metadata A + content B) works.
        const metaRef = pluginMetaRef!;
        form.append("meta_plugin_source", metaRef.instance.id);
        form.append("meta_plugin_book_id", metaRef.item.id);
      }
      // Manual overrides refine both the manual and the plugin mode.
      if (mode === "manual" || mode === "plugin") {
        if (title.trim()) form.append("title", title.trim());
        if (authors.trim()) form.append("authors", JSON.stringify(splitAuthors(authors)));
        if (description.trim()) form.append("description", description.trim());
        if (Object.keys(ext).length > 0) form.append("ext", JSON.stringify(ext));
      }

      const result = await api<AcquireResult>("/books", { method: "POST", body: form });
      // Volume splits create a series + one book per 卷; surface that.
      if (result.series && result.books.length > 1) {
        toast.push(
          "success",
          t("library.seriesAdded", {
            title: result.series.title,
            count: result.books.length,
          }),
        );
      } else {
        toast.push("success", t("library.added", { title: result.books[0]?.book.title ?? "" }));
      }
      reset();
      onClose();
      onAdded();
    } catch (err) {
      setDialogError(err instanceof Error ? err.message : t("library.uploadFailed"));
    } finally {
      setUploading(false);
      if (fileRef.current) fileRef.current.value = "";
      setFileName(null);
    }
  }

  function close() {
    reset();
    onClose();
  }

  return (
    <Modal open={open} onClose={close} title={t("library.addBook")} wide>
      <section className="modal-section">
        <h3>{t("library.sectionAcquire")}</h3>
        <div className="row">
          <label className="radio">
            <input type="radio" checked={tab === "upload"} onChange={() => switchTab("upload")} />
            {t("library.tabUpload")}
          </label>
          <label className="radio">
            <input type="radio" checked={tab === "plugin"} onChange={() => switchTab("plugin")} />
            {t("library.tabPlugin")}
          </label>
          <label className="radio">
            <input
              type="radio"
              checked={tab === "meta-only"}
              onChange={() => switchTab("meta-only")}
            />
            {t("library.tabMetaOnly")}
          </label>
        </div>

        {tab === "upload" && (
          <div className="field">
            <label className="file-picker">
              <span>{fileName ?? t("library.pickFile")}</span>
              <input
                ref={fileRef}
                type="file"
                accept=".epub,.txt,.text"
                hidden
                onChange={(e) => setFileName(e.target.files?.[0]?.name ?? null)}
              />
            </label>
            <p className="hint">{t("library.tabUploadHint")}</p>
          </div>
        )}

        {tab === "plugin" && (
          <div className="field">
            <SourceSearchPane
              instances={instances}
              filter={(i) =>
                i.enabled && (i.capabilities.includes("search") || i.capabilities.includes("lookup"))
              }
              pickLabel={() => t("library.pickThisBook")}
              onPick={(pick) => {
                usePluginBook(pick);
              }}
            />
            {pluginAcq && (
              <p className="hint">
                {t("library.pluginBookPicked", {
                  title: pluginAcq.item.title,
                  plugin: pluginAcq.instance.name,
                  id: pluginAcq.item.id,
                })}{" "}
                <button className="link-btn" onClick={() => setPluginAcq(null)}>
                  {t("library.clearPlugin")}
                </button>
              </p>
            )}
          </div>
        )}

        {tab === "meta-only" && (
          <div className="field">
            <p className="hint">{t("library.tabMetaOnlyHint")}</p>
          </div>
        )}
      </section>

      <section className="modal-section">
        <h3>{t("library.sectionMeta")}</h3>
        <div className="row">
          {modeAvailable("auto") && (
            <label className="radio">
              <input type="radio" checked={mode === "auto"} onChange={() => switchMode("auto")} />
              {t("library.modeAuto")}
            </label>
          )}
          {modeAvailable("plugin") && (
            <label className="radio">
              <input
                type="radio"
                checked={mode === "plugin"}
                onChange={() => switchMode("plugin")}
              />
              {t("library.modePlugin")}
            </label>
          )}
          {modeAvailable("attach") && (
            <label className="radio">
              <input
                type="radio"
                checked={mode === "attach"}
                onChange={() => switchMode("attach")}
              />
              {t("library.modeAttach")}
            </label>
          )}
          <label className="radio">
            <input
              type="radio"
              checked={mode === "manual"}
              onChange={() => switchMode("manual")}
            />
            {t("library.modeManual")}
          </label>
        </div>

        {mode === "auto" && (
          <p className="hint">
            {tab === "upload" ? t("library.modeAutoHint") : t("library.modeAutoPluginHint")}
          </p>
        )}

        {mode === "plugin" && (
          <div className="field">
            <SourceSearchPane
              instances={instances}
              filter={(i) => i.enabled && i.capabilities.includes("lookup")}
              pickLabel={() => t("library.useAsMetadataSource")}
              onPick={(pick) => {
                usePluginMeta(pick);
              }}
            />
            {pluginMetaRef && (
              <p className="hint">
                {t("library.pluginMetaPicked", {
                  title: pluginMetaRef.item.title,
                  plugin: pluginMetaRef.instance.name,
                  id: pluginMetaRef.item.id,
                })}{" "}
                <button className="link-btn" onClick={() => setPluginMetaRef(null)}>
                  {t("library.clearPlugin")}
                </button>
              </p>
            )}
            <p className="hint">
              {tab === "plugin" && pluginAcq
                ? t("library.modePluginCrossHint", { plugin: pluginAcq.instance.name })
                : t("library.modePluginHint")}
            </p>
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
          </div>
        )}

        {mode === "attach" && (
          <div className="field">
            <select value={attachBookId} onChange={(e) => setAttachBookId(e.target.value)}>
              <option value="">{t("library.chooseBook")}</option>
              {bookOptions.map(({ book }) => (
                <option key={book.id} value={book.id}>
                  {book.title}
                </option>
              ))}
            </select>
            <p className="hint">{t("library.modeAttachHint")}</p>
          </div>
        )}

        {mode === "manual" && (
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
        <h3>{t("library.sectionFile")}</h3>
        <div className="row">
          <select
            value={visibility}
            onChange={(e) => setVisibility(e.target.value as Visibility)}
            aria-label={t("library.visibilityLabel")}
          >
            <option value="private">{t("library.visibilityPrivate")}</option>
            <option value="public">{t("library.visibilityPublic")}</option>
          </select>
          <input
            placeholder={t("library.labelPlaceholder")}
            value={label}
            onChange={(e) => setLabel(e.target.value)}
          />
        </div>
      </section>

      {dialogError && <div className="error">{dialogError}</div>}
      <div className="modal-actions">
        <button onClick={close}>{t("library.cancel")}</button>
        <button className="primary" onClick={() => void submit()} disabled={uploading}>
          {uploading ? t("library.uploading") : t("library.addBook")}
        </button>
      </div>
    </Modal>
  );
}
