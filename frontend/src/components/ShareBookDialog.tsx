import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { Modal } from "./Modal";
import type { CreateShare, FileMeta, ShareInfo } from "../types";

export function ShareBookDialog({ files, initialFileId, onClose, onCreated }: {
  files: FileMeta[];
  initialFileId?: string;
  onClose: () => void;
  onCreated: (share: ShareInfo) => void;
}) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<string[]>(initialFileId ? [initialFileId] : []);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const active = useRef(true);
  const submitting = useRef(false);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  async function create() {
    if (submitting.current || selected.length === 0) return;
    submitting.current = true;
    setBusy(true);
    setError(null);
    try {
      const body: CreateShare = { kind: "book", file_ids: selected };
      const share = await api<ShareInfo>(`/files/${encodeURIComponent(selected[0])}/shares`, {
        method: "POST", body: JSON.stringify(body),
      });
      if (active.current) onCreated(share);
    } catch (err) {
      if (active.current) setError(err instanceof Error ? err.message : t("common.failed"));
    } finally {
      submitting.current = false;
      if (active.current) setBusy(false);
    }
  }
  return (
    <Modal open title={t("share.chooseContents")} onClose={() => { if (!submitting.current) onClose(); }}>
      <p className="hint">{t("share.selectionHint")}</p>
      <fieldset className="add-form" disabled={busy}>
        {files.map((file) => (
          <label className="share-file-option" key={file.id}>
            <input type="checkbox" checked={selected.includes(file.id)} onChange={(event) => {
              setSelected((ids) => event.target.checked ? [...ids, file.id] : ids.filter((id) => id !== file.id));
            }} />
            <span>
              <strong>{file.label || (file.source === "local" ? file.format : file.source)}</strong>
              <span className="hint">{file.source} · {file.format} · {t("common.chaptersTotal", { count: file.chapter_count })}</span>
            </span>
          </label>
        ))}
      </fieldset>
      {error && <p className="error" role="alert">{error}</p>}
      <div className="detail-actions">
        <button className="btn" disabled={busy} onClick={onClose}>{t("library.cancel")}</button>
        <button className="btn primary" disabled={busy || selected.length === 0 || selected.length > 100} onClick={() => void create()}>
          {busy ? t("share.creating") : t("share.createLink")}
        </button>
      </div>
    </Modal>
  );
}
