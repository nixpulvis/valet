CREATE TABLE IF NOT EXISTS lots (
    uuid      TEXT  PRIMARY KEY NOT NULL,
    name      TEXT  NOT NULL UNIQUE,
    key_data  BLOB  NOT NULL,
    key_nonce BLOB  NOT NULL
);
