// Shared book-cover component: stored cover bytes first, then the remote
// `cover_url` recorded in the metadata, then a title-derived gradient tile
// (same title → same hue, so a coverless shelf still reads as distinct
// books instead of empty boxes). Each source fails on its own, so a dead
// remote URL still lands on the tile.
import { useState, type CSSProperties } from "react";

/** Stable hue (0-359) for a title: the shelf's coverless placeholder. */
export function placeholderHue(title: string): number {
  return [...title].reduce((h, c) => (h * 31 + c.codePointAt(0)!) % 360, 7);
}

type Stage = "stored" | "remote" | "tile";

export function BookCover({
  bookId,
  title,
  coverUrl,
}: {
  bookId: string;
  title: string;
  coverUrl?: string | null;
}) {
  const [stage, setStage] = useState<Stage>("stored");
  const next = () => setStage(stage === "stored" ? (coverUrl ? "remote" : "tile") : "tile");

  if (stage === "stored") {
    return (
      <img
        src={`/api/books/${bookId}/cover`}
        alt=""
        loading="lazy"
        decoding="async"
        onError={next}
      />
    );
  }
  if (stage === "remote" && coverUrl) {
    return (
      <img
        src={coverUrl}
        alt=""
        loading="lazy"
        decoding="async"
        // never leak the Referer to third-party CDNs
        referrerPolicy="no-referrer"
        onError={next}
      />
    );
  }
  return (
    <div
      className="book-cover-fallback"
      style={{ "--ph-h": placeholderHue(title) } as CSSProperties}
    >
      <span>{title.trim().charAt(0) || "书"}</span>
    </div>
  );
}
