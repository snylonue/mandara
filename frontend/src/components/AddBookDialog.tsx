// The unified "add a book" dialog (获取书籍 → 添加元数据).
//
// One acquisition flow for every content source:
//   step 1 (获取书籍): an uploaded file, or a book picked from a plugin
//     source (searchable or by manual id — see SourceSearchPane);
//   step 2 (添加元数据): a new entry (auto / manual overrides), or
//     attach to an existing metadata entry;
//   step 3: visibility + label, then submit.
//
// Both steps are orthogonal: any content source can be combined with any
// metadata mode. The submission goes to the unified endpoint
// POST /api/books (see the OpenAPI contract), which accepts either a
// `file` or `plugin_source`+`plugin_book_id` and a `book_id`/overrides.

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

type AcquireTab = "upload" | "plugin";
type MetaMode = "auto" | "attach" | "manual";

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

  // Plugin tab: the book whose content (and suggested metadata) we take.
  const [pluginAcq, setPluginAcq] = useState<SourcePick | null>(null);
  // Upload tab, manual mode: metadata linked to a plugin catalog entry.
  const [pluginMetaRef, setPluginMetaRef] = useState<SourcePick | null>(null);

  // attach mode: books available to the caller
  const [bookOptions, setBookOptions] = useState<BookListEntry[]>([]);
  const [attachBookId, setAttachBookId] = useState("");

  // manual mode fields
  const [title, setTitle] = useState("");
  const [authors, setAuthors] = useState("");
  const [description, setDescription] = useState("");
  // manual mode: extended metadata (ISBN/publisher/…)
  const [ext, setExt] = useState<ExtRecord>({});

  const suggestion = pluginAcq ?? pluginMetaRef;

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

  /** Adopt a picked source entry as the metadata suggestion (plugin tab). */
  function usePluginBook(pick: SourcePick) {
    setPluginAcq(pick);
    setMode((m) => (m === "auto" ? "auto" : "manual"));
    setTitle(pick.item.title);
    setAuthors(pick.item.authors.join("，"));
    setDescription(pick.item.description ?? "");
    setDialogError(null);
  }

  /** Link the metadata of an uploaded file to a plugin catalog entry. */
  function useUploadMetadata(pick: SourcePick) {
    setPluginMetaRef(pick);
    setTitle(pick.item.title);
    setAuthors(pick.item.authors.join("，"));
    setDescription(pick.item.description ?? "");
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
    if (mode === "manual" && !title.trim() && !suggestion) {
      setDialogError(t("library.manualNeedsTitle"));
      return;
    }

    setUploading(true);
    setDialogError(null);
    try {
      const form = new FormData();
      form.append("visibility", visibility);
      if (label.trim()) form.append("label", label.trim());

      // 获取书籍: the content source (file or plugin book).
      if (tab === "upload") {
        const file = fileRef.current?.files?.[0];
        if (file) form.append("file", file);
      } else if (pluginAcq) {
        form.append("plugin_source", pluginAcq.instance.id);
        form.append("plugin_book_id", pluginAcq.item.id);
      }

      // 添加元数据: where the metadata entry comes from.
      if (mode === "attach") {
        form.append("book_id", attachBookId);
      } else {
        // Uploaded file whose metadata is linked to a plugin catalog
        // entry (upload tab manual mode).
        if (tab === "upload" && pluginMetaRef) {
          form.append("plugin_source", pluginMetaRef.instance.id);
          form.append("plugin_book_id", pluginMetaRef.item.id);
        }
        if (mode === "manual") {
          if (title.trim()) form.append("title", title.trim());
          if (authors.trim()) form.append("authors", JSON.stringify(splitAuthors(authors)));
          if (description.trim()) form.append("description", description.trim());
          if (Object.keys(ext).length > 0) form.append("ext", JSON.stringify(ext));
        }
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

  function switchTab(next: AcquireTab) {
    if (next === tab) return;
    setTab(next);
    setDialogError(null);
    // Content and metadata refs belong to their tab; keep the metadata
    // suggestion of the newly active tab.
    setPluginAcq(next === "plugin" ? pluginAcq : null);
    setPluginMetaRef(next === "upload" ? pluginMetaRef : null);
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
      </section>

      <section className="modal-section">
        <h3>{t("library.sectionMeta")}</h3>
        <div className="row">
          <label className="radio">
            <input type="radio" checked={mode === "auto"} onChange={() => setMode("auto")} />
            {t("library.modeAuto")}
          </label>
          <label className="radio">
            <input type="radio" checked={mode === "attach"} onChange={() => setMode("attach")} />
            {t("library.modeAttach")}
          </label>
          <label className="radio">
            <input type="radio" checked={mode === "manual"} onChange={() => setMode("manual")} />
            {t("library.modeManual")}
          </label>
        </div>

        {mode === "auto" && (
          <p className="hint">
            {tab === "upload"
              ? t("library.modeAutoHint")
              : t("library.modeAutoPluginHint")}
          </p>
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
            {tab === "upload" && (
              <>
                <p className="hint">{t("library.modeManualHint")}</p>
                <div className="picker">
                  <SourceSearchPane
                    instances={instances}
                    filter={(i) =>
                      i.enabled && i.capabilities.includes("search") && i.capabilities.includes("lookup")
                    }
                    pickLabel={() => t("library.useAsMetadataSource")}
                    onPick={(pick) => {
                      useUploadMetadata(pick);
                    }}
                  />
                </div>
                {pluginMetaRef && (
                  <p className="hint">
                    {t("library.pluginSource", {
                      plugin: pluginMetaRef.instance.name,
                      id: pluginMetaRef.item.id,
                    })}{" "}
                    <button className="link-btn" onClick={() => setPluginMetaRef(null)}>
                      {t("library.clearPlugin")}
                    </button>
                  </p>
                )}
              </>
            )}
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