CREATE TABLE IF NOT EXISTS user_lot_keys (
    username   TEXT  NOT NULL,
    lot_uuid   TEXT  NOT NULL,
    key_data   BLOB  NOT NULL,
    key_nonce  BLOB  NOT NULL,
    PRIMARY KEY (username, lot_uuid),
    FOREIGN KEY (username) REFERENCES users (username),
    FOREIGN KEY (lot_uuid) REFERENCES lots (uuid)
);
