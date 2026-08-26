-- Intrinsic image dimensions, sniffed from the encoded bytes at
-- store time. Emitted as width/height attributes on chapter <img> tags so
-- browsers reserve layout space (aspect-ratio) before the bytes load;
-- NULL = unknown, the attribute is simply omitted.
ALTER TABLE images ADD COLUMN width INTEGER;
ALTER TABLE images ADD COLUMN height INTEGER;
