CREATE TABLE IF NOT EXISTS lots (
    username  TEXT     NOT NULL,
    uuid      TEXT     PRIMARY KEY,
    FOREIGN KEY (username) REFERENCES users (username)
);
