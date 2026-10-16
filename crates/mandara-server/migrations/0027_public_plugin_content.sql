-- Plugin content is shared publicly; only local uploads have private visibility.
UPDATE book_files SET visibility = 'public' WHERE source <> 'local' AND visibility <> 'public';

CREATE TRIGGER book_files_plugin_public_insert
BEFORE INSERT ON book_files
WHEN NEW.source <> 'local' AND NEW.visibility <> 'public'
BEGIN
    SELECT RAISE(ABORT, 'plugin content must be public');
END;

CREATE TRIGGER book_files_plugin_public_update
BEFORE UPDATE OF source, visibility ON book_files
WHEN NEW.source <> 'local' AND NEW.visibility <> 'public'
BEGIN
    SELECT RAISE(ABORT, 'plugin content must be public');
END;
