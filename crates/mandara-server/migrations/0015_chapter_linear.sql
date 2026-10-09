-- EPUB auxiliary documents are addressable without joining default reading order.
-- Existing imports and plugin/text chapters remain linear for compatibility.
ALTER TABLE chapters ADD COLUMN linear INTEGER NOT NULL DEFAULT 1 CHECK (linear IN (0, 1));
