CREATE TABLE Usage (
    account_id INTEGER NOT NULL,
    key_id INTEGER NOT NULL,
    plan_id INTEGER NOT NULL,
    date_time DATETIME NOT NULL,
    total_requests INTEGER,
    total_data_mb REAL,
    PRIMARY KEY (account_id, key_id, plan_id, date_time)
);
