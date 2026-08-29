// Shared book-cover component: stored cover bytes first, then the remote
// `cover_url` recorded in the metadata, then a letter fallback tile.
import { useState } from "react";

export function BookCover({
  bookId,
  title,
  coverUrl,
}: {
  bookId: string;
  title: string;
  coverUrl?: string | null;
}) {
  const [failed, setFailed] = useState(false);
  if (!failed) {
    return (
      <img
        src={`/api/books/${bookId}/cover`}
        alt=""
        loading="lazy"
        onError={() => setFailed(true)}
      />
    );
  }
  if (coverUrl) {
    // Remote cover: never leak the Referer to third-party CDNs, and
    // degrade to the letter tile when it fails to load.
    return (
      <img
        src={coverUrl}
        alt=""
        loading="lazy"
        referrerPolicy="no-referrer"
        onError={() => setFailed(true)}
      />
    );
  }
  return (
    <div className="book-cover-fallback">
      <span>{title.trim().charAt(0) || "书"}</span>
    </div>
  );
}
