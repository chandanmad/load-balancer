CREATE TABLE Usage (
    account_id INTEGER NOT NULL,
    api_key_id CHAR(36) PRIMARY KEY,
    plan_id INTEGER NOT NULL,
    date_time DATETIME NOT NULL,
    total_requests INTEGER,
    total_data_mb REAL,
    PRIMARY KEY (account_id, api_key_id, plan_id, date_time)
);
