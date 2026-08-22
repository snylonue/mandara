import { useCallback, useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useAuth } from "../auth";
import { Modal } from "../components/Modal";
import {
  SourceBrowserDialog,
  SourceSearchPane,
  usePluginInstances,
  type SourcePick,
} from "../components/SourceSearch";
import type {
  BookDetail,
  BookListEntry,
  PluginInstance,
  Visibility,
} from "../types";

function BookCard({ entry }: { entry: BookListEntry }) {
  const { t } = useTranslation();
  const { book, files } = entry;
  const authors = book.authors.length ? book.authors.join(" / ") : t("common.anonymous");
  return (
    <Link className="book-card" to={`/book/${book.id}`}>
      <div className="book-title">{book.title}</div>
      <div className="book-meta">{authors} · {files.reduce((n, f) => n + f.chapter_count, 0)} {t("common.chapters")}</div>
      <div className="book-tags">
        {files.map((f) => (
          <span key={f.id} className="tag">
            {f.format}
            {f.source !== "local" ? "·plugin" : ""}
            {f.visibility === "public" ? "·public" : "·private"}
          </span>
        ))}
      </div>
    </Link>
  );
}

type UploadMode = "auto" | "attach" | "manual";

const splitAuthors = (raw: string) =>
  raw
    .split(/[,，]/)
    .map((s) => s.trim())
    .filter(Boolean);

export function LibraryPage() {
  const { user } = useAuth();
  const { t } = useTranslation();
  const instances = usePluginInstances();
  const [entries, setEntries] = useState<BookListEntry[]>([]);
  const [q, setQ] = useState("");
  const [error, setError] = useState<string | null>(null);
  // Which toolbar dialog is open — at most one at a time.
  const [dialog, setDialog] = useState<"upload" | "source" | null>(null);

  const load = useCallback(async (query: string) => {
    try {
      const list = await api<BookListEntry[]>(`/books?q=${encodeURIComponent(query)}`);
      setEntries(list);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : t("common.failed"));
    }
  }, []);

  useEffect(() => {
    void load(q);
  }, [load, q]);

  return (
    <div>
      <div className="toolbar">
        <input
          className="search"
          placeholder={t("library.searchPlaceholder")}
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <button onClick={() => setDialog("source")}>
          {t("library.sourceBrowser")}
        </button>
        <button className="primary" onClick={() => setDialog("upload")}>
          {t("library.upload")}
        </button>
      </div>
      {error && <div className="error">{error}</div>}
      {user?.role === "admin" && <p className="hint">{t("library.adminHint")}</p>}
      <p className="hint">{t("library.metadataHint")}</p>
      <div className="book-grid">
        {entries.map((entry) => (
          <BookCard key={entry.book.id} entry={entry} />
        ))}
      </div>
      {!entries.length && !error && <p className="hint">{t("library.empty")}</p>}

      {/* Source browser: search a plugin source and materialize a book
          into the library (one book at a time, R4 lazy catalog access). */}
      <SourceBrowserDialog
        open={dialog === "source"}
        onClose={() => setDialog(null)}
        instances={instances}
        filter={(i) => i.enabled && i.capabilities.includes("lookup")}
        title={t("library.sourceBrowserTitle")}
        allowManualId
        onPick={async (pick: SourcePick) => {
          await api<BookDetail>(`/plugins/${pick.instance.id}/books`, {
            method: "POST",
            body: JSON.stringify({ book_id: pick.item.id }),
          });
          setDialog(null);
          setError(null);
          setQ("");
          void load("");
        }}
      />

      <UploadDialog
        open={dialog === "upload"}
        onClose={() => setDialog(null)}
        instances={instances}
        onUploaded={() => {
          setQ("");
          void load("");
        }}
      />
    </div>
  );
}

/**
 * Upload dialog with three metadata modes:
 * - auto:    parse metadata from the file; plugins may identify it first,
 * - attach:  attach the file to an existing metadata entry,
 * - manual:  hand-written fields, optionally prefilled from a plugin
 *            catalog (the upload then stays linked to that plugin source
 *            and its metadata can be refreshed from it later).
 */
function UploadDialog({
  open,
  onClose,
  instances,
  onUploaded,
}: {
  open: boolean;
  onClose: () => void;
  instances: PluginInstance[];
  onUploaded: () => void;
}) {
  const { t } = useTranslation();
  const [mode, setMode] = useState<UploadMode>("auto");
  const [uploading, setUploading] = useState(false);
  const [visibility, setVisibility] = useState<Visibility>("private");
  const [label, setLabel] = useState("");
  const [fileName, setFileName] = useState<string | null>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  // attach mode: books available to the caller
  const [bookOptions, setBookOptions] = useState<BookListEntry[]>([]);
  const [attachBookId, setAttachBookId] = useState("");

  // manual mode fields
  const [title, setTitle] = useState("");
  const [authors, setAuthors] = useState("");
  const [description, setDescription] = useState("");

  // manual mode: metadata from a plugin source (resolved via get-book)
  const [pluginRef, setPluginRef] = useState<{ source: string; id: string } | null>(null);
  const [pickerError, setPickerError] = useState<string | null>(null);

  const reset = () => {
    setMode("auto");
    setVisibility("private");
    setLabel("");
    setFileName(null);
    setDialogError(null);
    setAttachBookId("");
    setTitle("");
    setAuthors("");
    setDescription("");
    setPluginRef(null);
    setPickerError(null);
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

  function usePluginBook(pick: SourcePick) {
    setPluginRef({ source: pick.instance.id, id: pick.item.id });
    setTitle(pick.item.title);
    setAuthors(pick.item.authors.join("，"));
    setDescription(pick.item.description ?? "");
    setPickerError(null);
  }

  async function submit() {
    const file = fileRef.current?.files?.[0];
    if (!file) {
      setDialogError(t("library.pickFile"));
      return;
    }
    if (mode === "attach" && !attachBookId) {
      setDialogError(t("library.pickBook"));
      return;
    }
    if (mode === "manual" && !title.trim() && !pluginRef) {
      setDialogError(t("library.manualNeedsTitle"));
      return;
    }
    setUploading(true);
    setDialogError(null);
    try {
      const form = new FormData();
      form.append("file", file);
      form.append("visibility", visibility);
      if (label.trim()) form.append("label", label.trim());
      if (mode === "attach") {
        form.append("book_id", attachBookId);
      } else if (mode === "manual") {
        if (title.trim()) form.append("title", title.trim());
        if (authors.trim()) form.append("authors", JSON.stringify(splitAuthors(authors)));
        if (description.trim()) form.append("description", description.trim());
        if (pluginRef) {
          form.append("plugin_source", pluginRef.source);
          form.append("plugin_book_id", pluginRef.id);
        }
      }
      await api<BookDetail>("/books", { method: "POST", body: form });
      reset();
      onClose();
      onUploaded();
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
    <Modal open={open} onClose={close} title={t("library.upload")} wide>
      <section className="modal-section">
        <h3>{t("library.sectionMeta")}</h3>
        <div className="row">
          <label className="radio">
            <input
              type="radio"
              checked={mode === "auto"}
              onChange={() => setMode("auto")}
            />
            {t("library.modeAuto")}
          </label>
          <label className="radio">
            <input
              type="radio"
              checked={mode === "attach"}
              onChange={() => setMode("attach")}
            />
            {t("library.modeAttach")}
          </label>
          <label className="radio">
            <input
              type="radio"
              checked={mode === "manual"}
              onChange={() => setMode("manual")}
            />
            {t("library.modeManual")}
          </label>
        </div>

        {mode === "auto" && <p className="hint">{t("library.modeAutoHint")}</p>}

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
            <p className="hint">{t("library.modeManualHint")}</p>
            <div className="picker">
              <SourceSearchPane
                instances={instances}
                filter={(i) => i.enabled && i.capabilities.includes("search") && i.capabilities.includes("lookup")}
                onPick={(pick) => {
                  usePluginBook(pick);
                }}
              />
            </div>
            {pickerError && <p className="error">{pickerError}</p>}
            {pluginRef && (
              <p className="hint">
                {t("library.pluginSource", { plugin: pluginRef.source, id: pluginRef.id })}{" "}
                <button className="link-btn" onClick={() => setPluginRef(null)}>
                  {t("library.clearPlugin")}
                </button>
              </p>
            )}
          </div>
        )}
      </section>

      <section className="modal-section">
        <h3>{t("library.sectionFile")}</h3>
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
          {uploading ? t("library.uploading") : t("library.upload")}
        </button>
      </div>
    </Modal>
  );
}
