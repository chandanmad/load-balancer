-- Represents the different pricing tiers
CREATE TABLE Plans (
    plan_id BIGINT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,           -- 'Free', 'Pro'
    monthly_quota INTEGER NOT NULL,        -- Total requests allowed per month
    rps_limit INTEGER NOT NULL,            -- Rate limit (Requests Per Second)
    price_per_1k_req REAL NOT NULL         -- Usage-based rate after/within plan
);

-- The entity that owns the subscription and pays the bill
CREATE TABLE Accounts (
    account_id BIGINT PRIMARY KEY NOT NULL,
    email TEXT UNIQUE NOT NULL,
    plan_id BIGINT NOT NULL,
    billing_status TEXT NOT NULL,          -- 'active', 'past_due'
    FOREIGN KEY (plan_id) REFERENCES Plans(plan_id)
);

-- Multiple keys can belong to one account
CREATE TABLE APIKeys (
    key_id BIGINT PRIMARY KEY NOT NULL,
    account_id BIGINT NOT NULL,
    api_key_hash TEXT UNIQUE NOT NULL,     -- Store hashes, not raw keys
    is_active BOOLEAN NOT NULL DEFAULT 1,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (account_id) REFERENCES Accounts(account_id)
);
