CREATE TABLE IF NOT EXISTS users (
    username        TEXT  PRIMARY KEY,
    password_salt   BLOB  NOT NULL
);
