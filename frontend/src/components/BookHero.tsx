import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { BookCover } from "./BookCover";
import { ExtMetaTable, type ExtRecord } from "./ExtMetaForm";
import type { BookMeta } from "../types";

/** Identical book presentation for authenticated and public detail views. */
export function BookHero({ book, fileCount, series, actions }: {
  book: Pick<BookMeta, "id" | "title" | "authors" | "description" | "cover_url" | "ext">;
  fileCount: number;
  series?: ReactNode;
  actions?: ReactNode;
}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  return (
    <div className="detail-hero">
      <div className="detail-cover">
        <BookCover key={book.id} bookId={book.id} title={book.title} coverUrl={book.cover_url} />
      </div>
      <div className="detail-info">
        {series}
        <h1>{book.title}</h1>
        <div className="detail-meta">
          <span>{book.authors.join(" / ") || t("common.anonymous")}</span>
          <span>·</span>
          <span>{t("book.metaCount", { count: fileCount })}</span>
        </div>
        <ExtMetaTable kind="book" value={(book.ext ?? {}) as ExtRecord} />
        {book.description && <p className={`detail-desc ${expanded ? "expanded" : ""}`}>{book.description}</p>}
        {(book.description?.length ?? 0) > 120 && (
          <button className="btn link sm" onClick={() => setExpanded((value) => !value)}>
            {expanded ? t("book.descCollapse") : t("book.descExpand")} ▾
          </button>
        )}
        {actions && <div className="detail-actions">{actions}</div>}
      </div>
    </div>
  );
}
