// Admin page: plugin instance management.
//
// Lists registered instances (enable/disable, delete, re-sync), registers
// new instances from the compiled wasm files, and renders the
// schema-driven configuration form per instance.

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ApiError } from "../api";
import { useAuth } from "../auth";
import type {
  ConfigError,
  ConfigField,
  ConfigSchema,
  PluginInstance,
  ValidationErrorResponse,
} from "../types";

function defaultsFor(fields: ConfigField[]): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of fields) {
    if (f.default !== null && f.default !== undefined) {
      try {
        // Defaults are serialized JSON (a bare zh-CN label for text fields
        // is not parseable and falls back to the raw string below).
        out[f.key] = JSON.parse(f.default);
      } catch {
        if (f.kind === "string") out[f.key] = f.default;
      }
      if ((f.kind === "string" || f.kind === "number") && !(f.key in out)) {
        out[f.key] = f.kind === "string" ? "" : 0;
      }
    }
    if (!(f.key in out)) {
      if (f.kind === "boolean") out[f.key] = false;
      else if (f.kind === "enum") out[f.key] = 0;
      else if (f.kind === "list-of-string") out[f.key] = [];
      else if (f.kind === "number") out[f.key] = 0;
      else out[f.key] = "";
    }
  }
  return out;
}

/** Current values of an instance's config, or schema defaults. */
function currentValues(schema: ConfigSchema): Record<string, unknown> {
  return { ...defaultsFor(schema.fields), ...(schema.config as Record<string, unknown>) };
}

function ConfigForm({
  instance,
  onSaved,
  setError,
}: {
  instance: PluginInstance;
  onSaved: () => void;
  setError: (msg: string | null) => void;
}) {
  const { t } = useTranslation();
  const [schema, setSchema] = useState<ConfigSchema | null>(null);
  const [values, setValues] = useState<Record<string, unknown> | null>(null);
  const [saving, setSaving] = useState(false);
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [listTexts, setListTexts] = useState<Record<string, string>>({});

  const load = useCallback(async () => {
    try {
      const s = await api<ConfigSchema>(`/plugins/${instance.id}/config-schema`);
      setSchema(s);
      const v = currentValues(s);
      setValues(v);
      // list-of-string fields edit as newline/comma separated text
      const texts: Record<string, string> = {};
      for (const f of s.fields) {
        if (f.kind === "list-of-string") {
          texts[f.key] = ((v[f.key] as string[]) ?? []).join("\n");
        }
      }
      setListTexts(texts);
      setFieldErrors({});
    } catch (e) {
      setError(e instanceof Error ? e.message : t("common.failed"));
    }
  }, [instance.id, setError, t]);

  useEffect(() => {
    void load();
  }, [load]);

  if (!schema || !values) return <p className="hint">{t("common.loading")}</p>;

  function setValue(key: string, value: unknown) {
    setValues((v) => (v ? { ...v, [key]: value } : v));
  }

  function buildPayload(): Record<string, unknown> {
    const payload: Record<string, unknown> = {};
    for (const f of schema?.fields ?? []) {
      if (f.kind === "list-of-string") {
        const raw = listTexts[f.key] ?? "";
        payload[f.key] = raw
          .split(/[\n,，]/)
          .map((s) => s.trim())
          .filter(Boolean);
      } else if (values) {
        payload[f.key] = values[f.key];
      }
    }
    return payload;
  }

  async function save() {
    if (!schema || !values) return;
    setSaving(true);
    setFieldErrors({});
    setError(null);
    try {
      await api<PluginInstance>(`/plugins/${instance.id}/config`, {
        method: "PUT",
        body: JSON.stringify({ config: buildPayload() }),
      });
      onSaved();
    } catch (e) {
      if (e instanceof ApiError && e.status === 400) {
        const errors = (e.details as ValidationErrorResponse | undefined)?.errors ?? [];
        setFieldErrors(
          Object.fromEntries(errors.map((err: ConfigError) => [err.field, err.message])),
        );
      } else {
        setError(e instanceof Error ? e.message : t("common.failed_verb"));
      }
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="config-form">
      {schema.fields.map((f) => (
        <div key={f.key} className="field">
          <label className="hint">
            {f.label}
            {f.required ? " *" : ""}
            {f.hint ? `（${f.hint}）` : ""}
          </label>
          {f.kind === "string" && (
            <input
              value={(values[f.key] as string) ?? ""}
              onChange={(e) => setValue(f.key, e.target.value)}
            />
          )}
          {f.kind === "number" && (
            <input
              type="number"
              value={(values[f.key] as number) ?? 0}
              onChange={(e) => setValue(f.key, Number(e.target.value))}
            />
          )}
          {f.kind === "boolean" && (
            <input
              type="checkbox"
              checked={Boolean(values[f.key])}
              onChange={(e) => setValue(f.key, e.target.checked)}
            />
          )}
          {f.kind === "enum" && (
            <select
              value={(values[f.key] as number) ?? 0}
              onChange={(e) => setValue(f.key, Number(e.target.value))}
            >
              {(f.options ?? []).map((opt, i) => (
                <option key={opt} value={i}>
                  {opt}
                </option>
              ))}
            </select>
          )}
          {f.kind === "list-of-string" && (
            <textarea
              rows={2}
              placeholder={t("plugin.listPlaceholder")}
              value={listTexts[f.key] ?? ""}
              onChange={(e) =>
                setListTexts((t) => ({ ...t, [f.key]: e.target.value }))
              }
            />
          )}
          {fieldErrors[f.key] && <div className="error">{fieldErrors[f.key]}</div>}
        </div>
      ))}
      <button className="primary" disabled={saving} onClick={() => void save()}>
        {saving ? t("common.loading") : t("plugin.saveConfig")}
      </button>
    </div>
  );
}

export function PluginsPage() {
  const { user } = useAuth();
  const { t } = useTranslation();
  const [instances, setInstances] = useState<PluginInstance[]>([]);
  const [wasmFiles, setWasmFiles] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [configFor, setConfigFor] = useState<string | null>(null);
  // register form
  const [regFile, setRegFile] = useState("");
  const [regId, setRegId] = useState("");
  const [regBusy, setRegBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [list, files] = await Promise.all([
        api<PluginInstance[]>("/plugins"),
        api<string[]>("/plugins/wasm-files"),
      ]);
      setInstances(list);
      setWasmFiles(files);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : t("common.failed"));
    }
  }, [t]);

  useEffect(() => {
    void load();
  }, [load]);

  async function register() {
    setError(null);
    setNotice(null);
    if (!regFile) {
      setError(t("plugin.pickWasmFile"));
      return;
    }
    setRegBusy(true);
    try {
      await api<PluginInstance>("/plugins/instances", {
        method: "POST",
        body: JSON.stringify({
          id: regId.trim() || undefined,
          wasm_file: regFile,
        }),
      });
      setRegId("");
      setNotice(t("plugin.registered"));
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : t("common.failed_verb"));
    } finally {
      setRegBusy(false);
    }
  }

  async function setEnabled(instance: PluginInstance, enabled: boolean) {
    setNotice(null);
    setError(null);
    try {
      await api<PluginInstance>(`/plugins/instances/${instance.id}/enabled`, {
        method: "PUT",
        body: JSON.stringify({ enabled }),
      });
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : t("common.failed_verb"));
    }
  }

  async function syncInstance(_instance: PluginInstance) {
    setNotice(null);
    setError(null);
    try {
      const r = await api<{ synced: number }>("/plugins/sync", { method: "POST" });
      setNotice(t("plugin.synced", { n: String(r.synced) }));
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : t("common.failed_verb"));
    }
  }

  async function deleteInstance(instance: PluginInstance) {
    if (!window.confirm(t("plugin.confirmDelete", { id: instance.id }))) return;
    setError(null);
    setNotice(null);
    try {
      await api(`/plugins/instances/${instance.id}`, { method: "DELETE" });
      if (configFor === instance.id) setConfigFor(null);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : t("common.failed_verb"));
    }
  }

  const caps = (c: string[]) =>
    c.length ? c.join(" · ") : t("plugin.noCapabilities");

  return (
    <div>
      <div className="toolbar">
        <h2>{t("plugin.manageTitle")}</h2>
        <span className="hint">{user?.username}</span>
      </div>
      {error && <div className="error">{error}</div>}
      {notice && <p className="hint">{notice}</p>}

      <section className="card">
        <h3>{t("plugin.registerTitle")}</h3>
        <p className="hint">{t("plugin.registerHint")}</p>
        <div className="inline-form">
          <select value={regFile} onChange={(e) => setRegFile(e.target.value)}>
            <option value="">{t("plugin.pickWasmFile")}</option>
            {wasmFiles.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
          <input
            placeholder={t("plugin.idPlaceholder")}
            value={regId}
            onChange={(e) => setRegId(e.target.value)}
          />
          <button className="primary" disabled={regBusy} onClick={() => void register()}>
            {regBusy ? t("common.loading") : t("plugin.register")}
          </button>
        </div>
      </section>

      <section className="card">
        <h3>{t("plugin.instancesTitle")}</h3>
        {instances.length === 0 && <p className="hint">{t("plugin.noInstances")}</p>}
        {instances.map((instance) => (
          <div key={instance.id} className="plugin-instance">
            <div className="file-head">
              <div>
                <span className="strong">{instance.name}</span>
                <span className="tag">{instance.id}</span>
                <span className="tag">{instance.wasm_file}</span>
                <span className={`tag ${instance.enabled ? "tag-public" : ""}`}>
                  {instance.enabled ? t("plugin.enabled") : t("plugin.disabled")}
                </span>
              </div>
              <div className="row-actions">
                <button
                  className="link-btn"
                  onClick={() => void setEnabled(instance, !instance.enabled)}
                >
                  {instance.enabled ? t("plugin.disable") : t("plugin.enable")}
                </button>
                <button className="link-btn" onClick={() => void syncInstance(instance)}>
                  {t("plugin.resync")}
                </button>
                <button className="link-btn" onClick={() => setConfigFor(instance.id)}>
                  {t("plugin.configure")}
                </button>
                <button className="link-btn danger" onClick={() => void deleteInstance(instance)}>
                  {t("book.delete")}
                </button>
              </div>
            </div>
            <div className="file-meta hint">
              {t("plugin.capabilities")}: {caps(instance.capabilities)}
            </div>
            {configFor === instance.id && (
              <ConfigForm
                instance={instance}
                setError={setError}
                onSaved={() => {
                  setNotice(t("plugin.configSaved"));
                  setConfigFor(null);
                  void load();
                }}
              />
            )}
          </div>
        ))}
      </section>
    </div>
  );
}