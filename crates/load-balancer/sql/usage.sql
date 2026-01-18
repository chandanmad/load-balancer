CREATE TABLE Usage (
    account_id INTEGER NOT NULL,
    api_key CHAR(36) NOT NULL,
    plan_id INTEGER NOT NULL,
    date_time DATETIME NOT NULL,
    total_requests INTEGER,
    total_data_mb REAL,
    PRIMARY KEY (account_id, api_key, plan_id, date_time)
);
