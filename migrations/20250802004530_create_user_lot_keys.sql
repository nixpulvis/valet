CREATE TABLE IF NOT EXISTS user_lot_keys (
    username  TEXT  NOT NULL,
    lot       TEXT  NOT NULL,
    data      BLOB  NOT NULL,
    nonce     BLOB  NOT NULL,
    PRIMARY KEY (username, lot),
    FOREIGN KEY (username) REFERENCES users (username),
    FOREIGN KEY (lot) REFERENCES lots (uuid)
);
