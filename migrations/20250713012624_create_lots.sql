CREATE TABLE IF NOT EXISTS lots (
    uuid      TEXT     PRIMARY KEY,
    name      TEXT     NOT NULL UNIQUE
);
