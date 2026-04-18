ALTER TABLE records RENAME COLUMN nonce TO data_nonce;
ALTER TABLE records ADD COLUMN label       BLOB NOT NULL;
ALTER TABLE records ADD COLUMN label_nonce BLOB NOT NULL;
