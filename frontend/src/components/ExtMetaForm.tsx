// Extended metadata form/table shared by books and series
// (bangumi/douban-style fields, docs/metadata-ext-design.md).
//
// The value is a plain record (the API's `ext` object): every known key is
// optional, empty input removes the key (PATCH treats absent = keep /
// null = clear, so clearing sends explicit `null`s at the call site).
// Unknown keys are ignored by the form but preserved server-side.

import { useTranslation } from "react-i18next";

/** Known BookExt keys in display order. */
const BOOK_EXT_KEYS = [
  "subtitle",
  "original_title",
  "isbn",
  "publisher",
  "pub_date",
  "translators",
  "illustrators",
  "pages",
  "price",
  "binding",
  "language",
] as const;

/** Known SeriesExt keys in display order. */
export const SERIES_EXT_KEYS = [
  "original_title",
  "publisher",
  "pub_date",
  "language",
  "status",
  "total_volumes",
  "tags",
] as const;

export type ExtRecord = Record<string, unknown>;

const i18nKey = (key: string) => `ext.${key}`;

function toInput(value: unknown): string {
  if (value == null) return "";
  if (Array.isArray(value)) return value.join("，");
  return String(value);
}

function parseField(key: string, raw: string): unknown {
  const v = raw.trim();
  if (!v) return undefined;
  if (key === "translators" || key === "illustrators" || key === "tags") {
    const list = v.split(/[,，]/).map((s) => s.trim()).filter(Boolean);
    return list.length ? list : undefined;
  }
  if (key === "pages" || key === "total_volumes") {
    const n = Number(v);
    return Number.isFinite(n) && n >= 0 ? Math.floor(n) : undefined;
  }
  return v;
}

/** Read-only rendering of the non-empty known keys. */
export function ExtMetaTable({ kind, value }: { kind: "book" | "series"; value: ExtRecord }) {
  const { t } = useTranslation();
  const keys = kind === "book" ? BOOK_EXT_KEYS : SERIES_EXT_KEYS;
  const rows = keys.filter((k) => {
    const v = value[k];
    return v != null && !(Array.isArray(v) && v.length === 0) && v !== "";
  });
  if (rows.length === 0) return null;
  return (
    <table className="ext-table">
      <tbody>
        {rows.map((k) => (
          <tr key={k}>
            <th>{t(i18nKey(k))}</th>
            <td>
              {Array.isArray(value[k])
                ? (value[k] as unknown[]).map((x, i) => (
                    <span className="tag" key={String(x) + "-" + i}>
                      {String(x)}
                    </span>
                  ))
                : toInput(value[k])}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/**
 * Editable inputs for the known extended-metadata keys. `onChange`
 * receives the full next object; cleared fields are removed from it (the
 * caller decides how "cleared" reaches the API).
 */
export function ExtMetaForm({
  kind,
  value,
  onChange,
}: {
  kind: "book" | "series";
  value: ExtRecord;
  onChange: (next: ExtRecord) => void;
}) {
  const { t } = useTranslation();
  const keys = kind === "book" ? BOOK_EXT_KEYS : SERIES_EXT_KEYS;
  function set(key: string, raw: string) {
    const next = { ...value };
    const parsed = parseField(key, raw);
    if (parsed === undefined) delete next[key];
    else next[key] = parsed;
    onChange(next);
  }
  return (
    <div className="field">
      <div className="ext-form-grid">
        {keys.map((k) => (
          <input
            key={k}
            aria-label={t(i18nKey(k))}
            placeholder={t(i18nKey(k))}
            type={k === "pages" || k === "total_volumes" ? "number" : "text"}
            min={0}
            value={toInput(value[k])}
            onChange={(e) => set(k, e.target.value)}
          />
        ))}
      </div>
      <p className="hint">{t(`ext.formHint`)}</p>
    </div>
  );
}

/**
 * Build a PATCH merge-patch body from a form result against the previous
 * value: keys present in the form are sent as-is (set), keys that existed
 * before but were cleared are sent as explicit `null`s, untouched keys
 * stay absent (= keep).
 */
export function extMergePatch(
  before: ExtRecord,
  after: ExtRecord,
): ExtRecord | undefined {
  const patch: ExtRecord = {};
  let changed = false;
  for (const [k, v] of Object.entries(after)) {
    if (JSON.stringify(v) !== JSON.stringify(before[k])) {
      patch[k] = v;
      changed = true;
    }
  }
  for (const k of Object.keys(before)) {
    if (!(k in after)) {
      patch[k] = null;
      changed = true;
    }
  }
  return changed ? patch : undefined;
}
