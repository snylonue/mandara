import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { renderReaderScreenshot, type ScreenshotOptions, type ScreenshotStyle, type ScreenshotSelection } from "../readerScreenshot";
import { Modal } from "./Modal";
import { ScreenshotStepError } from "../screenshotTask";

export interface ReaderExcerpt extends ScreenshotSelection {
  title: string;
  chapter: string;
  chapterIdx: number;
  style: ScreenshotStyle;
}

export function ReaderScreenshotDialog({ excerpt, shareLink, createShareLink, onClose }: {
  excerpt: ReaderExcerpt;
  shareLink?: string;
  createShareLink?: () => Promise<string>;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [showTitle, setShowTitle] = useState(true);
  const [showChapter, setShowChapter] = useState(true);
  const [showLink, setShowLink] = useState(false);
  const [link, setLink] = useState(shareLink);
  const [sharing, setSharing] = useState(false);
  const submitting = useRef(false);
  const [shareError, setShareError] = useState("");
  const [renderError, setRenderError] = useState("");
  const [renderDetail, setRenderDetail] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [actionError, setActionError] = useState("");
  const [status, setStatus] = useState("");
  const [copying, setCopying] = useState(false);
  const [rendered, setRendered] = useState<{ options: ScreenshotOptions; blob: Blob; url: string } | null>(null);
  const canCopy = window.isSecureContext && !!navigator.clipboard?.write && typeof ClipboardItem !== "undefined";
  const options = useMemo<ScreenshotOptions>(() => {
    const url = showLink && link ? new URL(link, location.origin) : null;
    url?.searchParams.set("chapter", String(excerpt.chapterIdx));
    return {
      text: excerpt.text, document: excerpt.document, viewport: excerpt.viewport, style: excerpt.style,
      title: showTitle ? excerpt.title : undefined,
      chapter: showChapter ? excerpt.chapter : undefined,
      link: url?.href,
    };
  }, [excerpt, showTitle, showChapter, showLink, link]);
  // Matching by identity disables export immediately, before the next effect runs.
  const image = rendered?.options === options && (!showLink || link) ? rendered : null;
  useEffect(() => {
    let cancelled = false;
    const controller = new AbortController();
    let url: string | undefined;
    setRenderError("");
    setRenderDetail("");
    setStatus("");
    setActionError("");
    if (!showLink || link) {
      void renderReaderScreenshot(options, controller.signal).then((blob) => {
        if (cancelled) return;
        url = URL.createObjectURL(blob);
        setRendered({ options, blob, url });
      }).catch((err: unknown) => {
        if (cancelled) return;
        console.error("Reader screenshot rendering failed", err);
        const code = err && typeof err === "object" && "message" in err ? String(err.message) : String(err);
        setRenderError(t(code === "selection-size" ? "screenshot.tooLong" : code === "screenshot-timeout" ? "screenshot.timedOut" : "screenshot.renderFailed"));
        if (err instanceof ScreenshotStepError) {
          setRenderDetail(t("screenshot.errorDetail", { step: t("screenshot.steps." + err.step), detail: err.detail === "screenshot-timeout" ? t("screenshot.timeoutDetail") : err.detail }));
        } else if (code && code !== "selection-size" && code !== "screenshot-timeout") {
          setRenderDetail(t("screenshot.errorDetail", { step: t("screenshot.steps.unknown"), detail: code }));
        }
      });
    }
    return () => { cancelled = true; controller.abort(); if (url) URL.revokeObjectURL(url); };
  }, [options, showLink, link, t, attempt]);

  async function generateLink() {
    if (!createShareLink || submitting.current) return;
    submitting.current = true;
    setSharing(true);
    setShareError("");
    try { setLink(await createShareLink()); }
    catch (err) { setShareError(err instanceof Error ? err.message : t("screenshot.shareFailed")); }
    finally { submitting.current = false; setSharing(false); }
  }

  async function copy() {
    if (!image || copying || !canCopy) return;
    setCopying(true);
    setActionError("");
    setStatus("");
    try {
      // The PNG is already ready; write during the button's user activation.
      await navigator.clipboard.write([new ClipboardItem({ "image/png": image.blob })]);
      setStatus(t("screenshot.copied"));
    } catch { setActionError(t("screenshot.copyFailed")); }
    finally { setCopying(false); }
  }

  function save() {
    if (!image) return;
    const anchor = document.createElement("a");
    anchor.href = image.url;
    anchor.download = t("screenshot.filename");
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
  }

  return <Modal open wide title={t("screenshot.title")} onClose={() => { if (!submitting.current) onClose(); }}>
    <p className="hint">{t("screenshot.description")}</p>
    <fieldset className="screenshot-options" disabled={sharing || copying}>
      <legend>{t("screenshot.include")}</legend>
      <label><input type="checkbox" checked={showTitle} onChange={e => setShowTitle(e.target.checked)} />{t("screenshot.bookTitle")}</label>
      <label><input type="checkbox" checked={showChapter} onChange={e => setShowChapter(e.target.checked)} />{t("screenshot.chapter")}</label>
      <label><input type="checkbox" checked={showLink} disabled={!link && !createShareLink} onChange={e => setShowLink(e.target.checked)} />{t("screenshot.shareLink")}</label>
    </fieldset>
    {showLink && !link && <div className="screenshot-share">
      <p className="hint">{t("screenshot.shareWarning")}</p>
      <button className="btn" disabled={sharing} onClick={() => void generateLink()}>{t(sharing ? "share.creating" : "screenshot.generateLink")}</button>
      {shareError && <p className="error" role="alert">{shareError}</p>}
    </div>}
    {showLink && link && <p className="hint screenshot-link">{options.link}</p>}
    {renderError && <div><p className="error" role="alert">{renderError}{renderDetail && <><br />{renderDetail}</>}</p>
      <button className="btn" onClick={() => setAttempt(value => value + 1)}>{t("screenshot.retry")}</button></div>}
    {image ? <div className="screenshot-preview"><img src={image.url} alt={t("screenshot.preview")} /></div> : !renderError && (!showLink || link) && <p role="status">{t("screenshot.rendering")}</p>}
    {!canCopy && <p className="hint">{t("screenshot.clipboardUnavailable")}</p>}
    {actionError && <p className="error" role="alert">{actionError}</p>}
    {status && <p role="status">{status}</p>}
    <div className="detail-actions">
      <button className="btn" disabled={!image || !canCopy || copying} onClick={() => void copy()}>{t(copying ? "screenshot.copying" : "screenshot.copy")}</button>
      <button className="btn primary" disabled={!image} onClick={save}>{t("screenshot.save")}</button>
    </div>
  </Modal>;
}
