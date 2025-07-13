CREATE TABLE IF NOT EXISTS lots (
    username  TEXT     NOT NULL,
    uuid      TEXT     PRIMARY KEY,
    label     TEXT,
    main      BOOLEAN  NOT NULL DEFAULT TRUE,
    -- TODO: make the store an listing of the record labels
    -- once we have a records table.
    store     BLOB,
    nonce     BLOB,
    FOREIGN KEY (username) REFERENCES users (username)
);
