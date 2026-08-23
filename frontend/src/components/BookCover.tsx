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
    return <img src={coverUrl} alt="" loading="lazy" />;
  }
  return (
    <div className="book-cover-fallback">
      <span>{title.trim().charAt(0) || "书"}</span>
    </div>
  );
}
