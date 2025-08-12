CREATE TABLE IF NOT EXISTS user_lots (
    username  TEXT  NOT NULL,
    lot       TEXT  NOT NULL,
    name      TEXT  NOT NULL,
    data      BLOB  NOT NULL,
    nonce     BLOB  NOT NULL,
    PRIMARY KEY (username, lot),
    UNIQUE (username, name),
    FOREIGN KEY (username) REFERENCES users (username),
    FOREIGN KEY (lot) REFERENCES lots (uuid)
);
