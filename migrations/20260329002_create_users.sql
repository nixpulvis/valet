CREATE TABLE IF NOT EXISTS users (
    username          TEXT  PRIMARY KEY NOT NULL,
    salt              BLOB  NOT NULL,
    validation_data   BLOB  NOT NULL,
    validation_nonce  BLOB  NOT NULL
);
