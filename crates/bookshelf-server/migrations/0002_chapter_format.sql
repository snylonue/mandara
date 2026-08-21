-- Chapter content format: "html" (sanitized EPUB XHTML) or "text" (plain).
ALTER TABLE chapters ADD COLUMN format TEXT NOT NULL DEFAULT 'text';