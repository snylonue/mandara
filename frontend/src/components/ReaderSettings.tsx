// Reader display settings (font size / line height / theme / font family /
// column width), persisted to localStorage. Purely client-side — no API.
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

export type ReaderTheme = "dark" | "sepia" | "light";
export type ReaderFont = "serif" | "sans";

export interface ReaderSettings {
  fontSize: number; // px
  lineHeight: number;
  theme: ReaderTheme;
  font: ReaderFont;
  widthEm: number; // reading column max-width, in em
}

const DEFAULTS: ReaderSettings = {
  fontSize: 17,
  lineHeight: 2,
  theme: "dark",
  font: "serif",
  widthEm: 42,
};

const KEY = "mandara_reader_settings";

function load(): ReaderSettings {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return DEFAULTS;
    return { ...DEFAULTS, ...(JSON.parse(raw) as Partial<ReaderSettings>) };
  } catch {
    return DEFAULTS;
  }
}

/** Persisted reader settings state. */
export function useReaderSettings() {
  const [settings, setSettings] = useState<ReaderSettings>(load);
  useEffect(() => {
    localStorage.setItem(KEY, JSON.stringify(settings));
  }, [settings]);
  return [settings, setSettings] as const;
}

const LINE_HEIGHTS = [1.6, 1.8, 2.0, 2.2];
const WIDTHS = [34, 38, 42, 48];

/** Popover panel with all reading display controls. */
export function ReaderSettingsPanel({
  settings,
  onChange,
  onClose,
}: {
  settings: ReaderSettings;
  onChange: (next: ReaderSettings) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const patch = (p: Partial<ReaderSettings>) => onChange({ ...settings, ...p });

  return (
    <div className="popover reader-settings" onClick={(e) => e.stopPropagation()}>
      <div className="reader-settings-head">
        <span>{t("reader.settings")}</span>
        <button className="btn ghost sm" onClick={onClose} aria-label={t("common.close")}>
          ✕
        </button>
      </div>

      <div className="reader-setting-row">
        <span className="reader-setting-label">{t("reader.fontSize")}</span>
        <div className="row">
          <button
            className="btn sm"
            onClick={() => patch({ fontSize: Math.max(13, settings.fontSize - 1) })}
          >
            A−
          </button>
          <span className="reader-setting-value">{settings.fontSize}px</span>
          <button
            className="btn sm"
            onClick={() => patch({ fontSize: Math.min(28, settings.fontSize + 1) })}
          >
            A+
          </button>
        </div>
      </div>

      <div className="reader-setting-row">
        <span className="reader-setting-label">{t("reader.lineHeight")}</span>
        <div className="seg solid">
          {LINE_HEIGHTS.map((lh) => (
            <button
              key={lh}
              className={`btn sm${settings.lineHeight === lh ? " active" : ""}`}
              onClick={() => patch({ lineHeight: lh })}
            >
              {lh.toFixed(1)}
            </button>
          ))}
        </div>
      </div>

      <div className="reader-setting-row">
        <span className="reader-setting-label">{t("reader.fontFamily")}</span>
        <div className="seg solid">
          <button
            className={`btn sm${settings.font === "serif" ? " active" : ""}`}
            onClick={() => patch({ font: "serif" })}
          >
            {t("reader.fontSerif")}
          </button>
          <button
            className={`btn sm${settings.font === "sans" ? " active" : ""}`}
            onClick={() => patch({ font: "sans" })}
          >
            {t("reader.fontSans")}
          </button>
        </div>
      </div>

      <div className="reader-setting-row">
        <span className="reader-setting-label">{t("reader.width")}</span>
        <div className="seg solid">
          {WIDTHS.map((w) => (
            <button
              key={w}
              className={`btn sm${settings.widthEm === w ? " active" : ""}`}
              onClick={() => patch({ widthEm: w })}
            >
              {w === WIDTHS[0] ? t("reader.widthNarrow") : w === WIDTHS[WIDTHS.length - 1] ? t("reader.widthWide") : String(w)}
            </button>
          ))}
        </div>
      </div>

      <div className="reader-setting-row">
        <span className="reader-setting-label">{t("reader.theme")}</span>
        <div className="seg solid">
          {(["dark", "sepia", "light"] as const).map((th) => (
            <button
              key={th}
              className={`btn sm${settings.theme === th ? " active" : ""}`}
              onClick={() => patch({ theme: th })}
            >
              {t(`reader.theme_${th}`)}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
