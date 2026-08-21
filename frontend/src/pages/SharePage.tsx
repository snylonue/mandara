import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { Reader } from "../components/Reader";
import type { Chapter, ShareBookResponse, ShareView } from "../types";

/// Public share view: no login required, no progress saving.
export function SharePage() {
  const { token = "" } = useParams();
  const { t } = useTranslation();
  const [view, setView] = useState<ShareView | null>(null);
  const [book, setBook] = useState<ShareBookResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        const [v, b] = await Promise.all([
          api<ShareView>(`/shares/${token}`),
          api<ShareBookResponse>(`/shares/${token}/book`),
        ]);
        setView(v);
        setBook(b);
      } catch (err) {
        setError(err instanceof Error ? err.message : t("share.invalid"));
      }
    })();
  }, [token]);

  if (error) {
    return (
      <div className="card">
        <h1>{t("share.invalidTitle")}</h1>
        <p>{error}</p>
      </div>
    );
  }
  if (!view || !book) return <div className="page-loading">{t("common.loading")}</div>;

  return (
    <div>
      {view.session && (
        <div className="card share-snapshot">
          <h2>
            {t("share.progressOf", {
              owner: view.session.owner_username,
              label: view.session.label,
            })}
          </h2>
          <p>
            <strong>{book.book.title}</strong> · {t("share.readTo")}{" "}
            <span className="strong">{view.session.percent}%</span>
            <span className="hint">
              {t("share.updatedAt", {
                time: new Date(view.session.updated_at).toLocaleString("zh-CN"),
              })}
            </span>
          </p>
        </div>
      )}
      <Reader
        title={book.book.title}
        chapters={book.chapters}
        loadChapter={async (idx) => {
          const c = await api<Chapter>(`/shares/${token}/chapters/${idx}`);
          return c;
        }}
        readOnly
      />
      <p className="hint center">
        {t("share.from")}
        <Link to="/">{t("share.deploy")}</Link>
      </p>
    </div>
  );
}