-- Freeze the explicitly selected content versions for each share.
CREATE TABLE share_files (
    token TEXT NOT NULL REFERENCES shares(token) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    PRIMARY KEY (token, file_id)
);
CREATE INDEX share_files_file_id ON share_files(file_id);

-- Existing book and session links retain access to exactly their original file.
INSERT INTO share_files (token, file_id) SELECT token, file_id FROM shares;

-- Keep the default version included for every insertion path, including older clients.
CREATE TRIGGER share_default_file AFTER INSERT ON shares BEGIN
    INSERT INTO share_files (token, file_id) VALUES (NEW.token, NEW.file_id);
END;
