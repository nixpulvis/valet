CREATE TABLE IF NOT EXISTS records (
    uuid   TEXT  PRIMARY KEY NOT NULL,
    lot    TEXT  NOT NULL,
    data   BLOB  NOT NULL,
    nonce  BLOB  NOT NULL,
    FOREIGN KEY (lot) REFERENCES lots (uuid)
);
