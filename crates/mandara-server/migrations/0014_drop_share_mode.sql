-- `shares.mode` is a pure function of `kind` (`read` for book shares,
-- `progress` for session shares). Storing the derived value let the two
-- drift (any read path could pass `row.mode` through unchecked). Drop the
-- column; the model derives it from `ShareKind` instead.
ALTER TABLE shares DROP COLUMN mode;
