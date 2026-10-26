import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ReaderImage } from "../readerImage";
import { Modal } from "./Modal";

const extensions: Record<string, string> = {
  "image/jpeg": "jpg", "image/png": "png", "image/gif": "gif", "image/webp": "webp", "image/svg+xml": "svg",
};

export function ReaderImageDialog({ image, onClose }: { image: ReaderImage; onClose: () => void }) {
  const { t } = useTranslation();
  const [file, setFile] = useState<File | null>(null);
  const [sharing, setSharing] = useState(false);
  const pending = useRef(false);
  const [error, setError] = useState("");
  const canShare = window.isSecureContext && !!navigator.share && !!navigator.canShare;

  useEffect(() => {
    if (!canShare) return;
    const controller = new AbortController();
    let cancelled = false;
    setFile(null);
    // Prepare the original bytes before the click to retain Safari user activation.
    void fetch(image.src, { signal: controller.signal }).then(async response => {
      if (!response.ok) throw new Error("image-load");
      const blob = await response.blob();
      const extension = extensions[blob.type];
      if (!extension || cancelled) return;
      const original = new File([blob], t("readerImage.filename") + "." + extension, { type: blob.type });
      if (navigator.canShare({ files: [original] })) setFile(original);
    }).catch(() => { /* Native long-press and the original link remain available. */ });
    return () => { cancelled = true; controller.abort(); };
  }, [image.src, canShare, t]);

  async function share() {
    if (!file || pending.current) return;
    pending.current = true;
    setSharing(true);
    setError("");
    try {
      await navigator.share({ files: [file] });
    } catch (err) {
      if (!(err && typeof err === "object" && "name" in err && err.name === "AbortError")) {
        setError(t("readerImage.shareFailed"));
      }
    } finally { pending.current = false; setSharing(false); }
  }

  return <Modal open wide title={t("readerImage.title")} onClose={() => { if (!pending.current) onClose(); }}>
    <p className="hint">{t("readerImage.hint")}</p>
    <div className="reader-image-preview">
      <img src={image.src} alt={image.alt || t("readerImage.title")} />
    </div>
    {error && <p className="error" role="alert">{error}</p>}
    <div className="detail-actions">
      {canShare && <button className="btn primary" disabled={!file || sharing} onClick={() => void share()}>
        {t(sharing ? "readerImage.sharing" : "readerImage.share")}
      </button>}
      <a className="btn" href={image.src} target="_blank" rel="noopener noreferrer">{t("readerImage.openOriginal")}</a>
    </div>
    {!file && <p className="hint">{t("readerImage.shareFallback")}</p>}
  </Modal>;
}
