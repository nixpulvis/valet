CREATE TABLE IF NOT EXISTS lots (
    username  TEXT     NOT NULL,
    uuid      TEXT     PRIMARY KEY,
    main      BOOLEAN  NOT NULL DEFAULT TRUE,
    -- TODO: make the data a listing of the record labels
    -- once we have a records table.
    data      BLOB,
    nonce     BLOB,
    FOREIGN KEY (username) REFERENCES users (username)
);
