// Shelf cards: one book, one collapsed series (stacked cover + volume
// count), and the list-view rows. Presentational only — grouping, sorting
// and progress are resolved by the page into `ShelfEntry.progress`.
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { BookCover } from "./BookCover";
import { IconPlugin, IconPrivate, IconPublic } from "./icons";
import type { BookMeta, FileMeta, SeriesBrief } from "../types";
import { hasStarted, percentLabel, type WithProgress } from "../progress";

/** Minimal shape every card needs: a book, its visible files, progress. */
export type ShelfItem = WithProgress<{ book: BookMeta; files: FileMeta[] }>;

/** Author line: names joined, or the anonymous placeholder. */
function authorsOf(authors: string[], anonymous: string): string {
  return authors.length ? authors.join(" / ") : anonymous;
}

/** Visibility / plugin badges of a book, aggregated over its files. */
function CoverBadges({ entry }: { entry: ShelfItem }) {
  const { t } = useTranslation();
  const anyPublic = entry.files.some((f) => f.visibility === "public");
  const isPlugin = entry.files.some((f) => f.source !== "local");
  return (
    <div className="book-badges">
      <span
        className={`badge-icon ${anyPublic ? "badge-public" : "badge-private"}`}
        title={anyPublic ? t("book.publicTag") : t("book.privateTag")}
      >
        {anyPublic ? <IconPublic size={12} /> : <IconPrivate size={12} />}
      </span>
      {isPlugin && (
        <span className="badge-icon badge-plugin" title={t("book.pluginTag")}>
          <IconPlugin size={12} />
        </span>
      )}
    </div>
  );
}

/** Cover of a card, plus the overlays that belong on top of it. */
function CardCover({
  bookId,
  title,
  coverUrl,
  entry,
  volumeCount,
  volumeNo,
  percent,
}: {
  bookId: string;
  title: string;
  coverUrl: string | null;
  entry: ShelfItem;
  volumeCount?: number;
  volumeNo?: number;
  percent?: number;
}) {
  const { t } = useTranslation();
  return (
    <div className="book-cover">
      <BookCover bookId={bookId} title={title} coverUrl={coverUrl} />
      <CoverBadges entry={entry} />
      {volumeNo !== undefined && volumeNo > 0 && (
        <span className="cover-badge cover-volume">
          {t("series.volumeLabel", { volume: volumeNo })}
        </span>
      )}
      {volumeCount !== undefined && volumeCount > 1 && (
        <span className="cover-badge cover-count">×{volumeCount}</span>
      )}
      {percent !== undefined && percent > 0 && (
        <div className="cover-progress" aria-hidden>
          <i style={{ width: `${percent}%` }} />
        </div>
      )}
    </div>
  );
}

/** Body of a card: title + one line of metadata. */
function CardBody({ title, meta }: { title: string; meta: string }) {
  return (
    <div className="book-card-body">
      <div className="book-title" title={title}>
        {title}
      </div>
      <div className="book-meta">{meta}</div>
    </div>
  );
}

/**
 * One book. `to: null` renders a non-link card (a volume the caller cannot
 * read); `actions` is rendered outside the link (manage buttons).
 */
export function ShelfBookCard({
  entry,
  to,
  meta,
  title,
  volumeNo,
  finished = false,
  muted = false,
  actions,
}: {
  entry: ShelfItem;
  to?: string | null;
  /** Overrides the default metadata line. */
  meta?: string;
  /** Overrides the default book title (the series page shows 第 N 卷). */
  title?: string;
  volumeNo?: number;
  /** Read volumes dim their cover. */
  finished?: boolean;
  /** Volumes without readable content dim their whole card. */
  muted?: boolean;
  actions?: React.ReactNode;
}) {
  const { t } = useTranslation();
  const { book, files } = entry;
  const percent = percentLabel(entry.progress);
  const chapterCount = files.reduce((n, f) => n + f.chapter_count, 0);
  const defaultMeta = `${authorsOf(book.authors, t("common.anonymous"))} · ${chapterCount} ${t(
    "common.chapters",
  )}`;
  const inner = (
    <>
      <CardCover
        bookId={book.id}
        title={book.title}
        coverUrl={book.cover_url}
        entry={entry}
        volumeNo={volumeNo}
        percent={percent}
      />
      <CardBody
        title={title ?? book.title}
        meta={meta ?? (percent > 0 ? t("library.readPercent", { percent }) : defaultMeta)}
      />
    </>
  );
  const href = to === undefined ? `/book/${book.id}` : to;
  const cls = `book-card${finished ? " book-card--finished" : ""}${muted ? " book-card-muted" : ""}`;
  return (
    <div className={cls}>
      {href === null ? inner : (
        <Link className="book-card-link" to={href}>
          {inner}
        </Link>
      )}
      {actions}
    </div>
  );
}

/** A series with ≥2 visible volumes, collapsed to one stacked card. */
export function ShelfSeriesCard({
  series,
  volumes,
}: {
  series: SeriesBrief;
  volumes: ShelfItem[];
}) {
  const { t } = useTranslation();
  const lead = volumes.find((v) => v.book.cover_url) ?? volumes[0];
  const readCount = volumes.filter((v) => hasStarted(v.progress)).length;
  const percent = Math.round((readCount / volumes.length) * 100);
  return (
    <Link className="book-card book-card--series" to={`/series/${series.id}`}>
      <CardCover
        bookId={lead.book.id}
        title={lead.book.title}
        coverUrl={lead.book.cover_url}
        entry={lead}
        volumeCount={volumes.length}
        percent={percent}
      />
      <CardBody
        title={series.title}
        meta={
          readCount > 0
            ? t("library.seriesRead", { read: readCount, total: volumes.length })
            : authorsOf(series.authors, t("common.anonymous"))
        }
      />
    </Link>
  );
}

/** List row: compact one-line entry for the list view. */
export function ShelfListRow({
  entry,
  to,
  meta,
  volumeNo,
}: {
  entry: ShelfItem;
  to?: string;
  meta?: string;
  volumeNo?: number;
}) {
  const { t } = useTranslation();
  const { book, files } = entry;
  const percent = percentLabel(entry.progress);
  const chapterCount = files.reduce((n, f) => n + f.chapter_count, 0);
  const anyPublic = files.some((f) => f.visibility === "public");
  const isPlugin = files.some((f) => f.source !== "local");
  const defaultMeta = `${authorsOf(book.authors, t("common.anonymous"))} · ${chapterCount} ${t(
    "common.chapters",
  )}`;
  return (
    <Link className="shelf-row" to={to ?? `/book/${book.id}`}>
      <div className="shelf-row-cover">
        <BookCover bookId={book.id} title={book.title} coverUrl={book.cover_url} />
      </div>
      <div className="shelf-row-main">
        <div className="book-title" title={book.title}>
          {book.title}
        </div>
        <div className="book-meta">
          {meta ?? defaultMeta}
          {percent > 0 && ` · ${t("library.readPercent", { percent })}`}
        </div>
      </div>
      <div className="shelf-row-tags">
        {volumeNo !== undefined && volumeNo > 0 && (
          <span className="tag">
            {t("series.volumeLabel", { volume: volumeNo })}
          </span>
        )}
        <span
          className={`badge-icon ${anyPublic ? "badge-public" : "badge-private"}`}
          title={anyPublic ? t("book.publicTag") : t("book.privateTag")}
        >
          {anyPublic ? <IconPublic size={12} /> : <IconPrivate size={12} />}
        </span>
        {isPlugin && (
          <span className="badge-icon badge-plugin" title={t("book.pluginTag")}>
            <IconPlugin size={12} />
          </span>
        )}
      </div>
      {percent > 0 && (
        <span className="shelf-row-progress" aria-hidden>
          <i style={{ width: `${percent}%` }} />
        </span>
      )}
    </Link>
  );
}

/** List row for a collapsed series. */
export function ShelfSeriesRow({
  series,
  volumes,
}: {
  series: SeriesBrief;
  volumes: ShelfItem[];
}) {
  const { t } = useTranslation();
  const lead = volumes.find((v) => v.book.cover_url) ?? volumes[0];
  const readCount = volumes.filter((v) => hasStarted(v.progress)).length;
  const percent = Math.round((readCount / volumes.length) * 100);
  return (
    <Link className="shelf-row" to={`/series/${series.id}`}>
      <div className="shelf-row-cover">
        <BookCover bookId={lead.book.id} title={lead.book.title} coverUrl={lead.book.cover_url} />
      </div>
      <div className="shelf-row-main">
        <div className="book-title" title={series.title}>
          {series.title}
        </div>
        <div className="book-meta">
          {t("series.volumes", { count: volumes.length })}
          {readCount > 0 && ` · ${t("library.seriesRead", { read: readCount, total: volumes.length })}`}
        </div>
      </div>
      <div className="shelf-row-tags">
        <span className="tag">×{volumes.length}</span>
      </div>
      {percent > 0 && (
        <span className="shelf-row-progress" aria-hidden>
          <i style={{ width: `${percent}%` }} />
        </span>
      )}
    </Link>
  );
}

/** Skeleton card shown while the library list loads. */
export function ShelfCardSkeleton() {
  return (
    <div className="book-card book-card--skeleton" aria-hidden>
      <div className="book-cover skeleton" />
      <div className="book-card-body">
        <div className="skeleton skeleton-line w70" />
        <div className="skeleton skeleton-line w45" />
      </div>
    </div>
  );
}
