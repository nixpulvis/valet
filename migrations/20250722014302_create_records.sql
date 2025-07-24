CREATE TABLE IF NOT EXISTS records (
    lot       TEXT  NOT NULL,
    uuid      TEXT  PRIMARY KEY,
    data      BLOB  NOT NULL,
    nonce     BLOB  NOT NULL,
    FOREIGN KEY (lot) REFERENCES lots (uuid)
);
