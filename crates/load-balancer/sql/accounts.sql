-- Represents the different pricing tiers
CREATE TABLE Plans (
    plan_id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    monthly_quota INTEGER NOT NULL,
    rps_limit INTEGER NOT NULL,
    price_per_1k_req REAL NOT NULL,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- The entity that owns the subscription
CREATE TABLE Accounts (
    account_id INTEGER PRIMARY KEY AUTOINCREMENT,
    email TEXT UNIQUE NOT NULL,
    plan_id INTEGER NOT NULL,
    billing_status TEXT NOT NULL,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (plan_id) REFERENCES Plans(plan_id)
);

-- Multiple keys per account
CREATE TABLE APIKeys (
    key_id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL,
    api_key_hash TEXT UNIQUE NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT 1,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (account_id) REFERENCES Accounts(account_id)
);

-- The Delta Tracker (The "Change Log")
-- This records exactly which record in which table changed.
CREATE TABLE ChangeLog (
    change_id INTEGER PRIMARY KEY AUTOINCREMENT,
    table_name TEXT NOT NULL,
    record_id INTEGER NOT NULL,
    operation TEXT NOT NULL, -- 'INSERT', 'UPDATE', 'DELETE'
    occurred_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- --- PLANS TRIGGERS ---
CREATE TRIGGER trg_plans_insert AFTER INSERT ON Plans BEGIN
    INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Plans', NEW.plan_id, 'INSERT');
END;

CREATE TRIGGER trg_plans_update AFTER UPDATE ON Plans BEGIN
    UPDATE Plans SET updated_at = CURRENT_TIMESTAMP WHERE plan_id = NEW.plan_id;
    INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Plans', NEW.plan_id, 'UPDATE');
END;

CREATE TRIGGER trg_plans_delete AFTER DELETE ON Plans BEGIN
    INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Plans', OLD.plan_id, 'DELETE');
END;

-- --- ACCOUNTS TRIGGERS ---
CREATE TRIGGER trg_accounts_insert AFTER INSERT ON Accounts BEGIN
    INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Accounts', NEW.account_id, 'INSERT');
END;

CREATE TRIGGER trg_accounts_update AFTER UPDATE ON Accounts BEGIN
    UPDATE Accounts SET updated_at = CURRENT_TIMESTAMP WHERE account_id = NEW.account_id;
    INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Accounts', NEW.account_id, 'UPDATE');
END;

CREATE TRIGGER trg_accounts_delete AFTER DELETE ON Accounts BEGIN
    INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Accounts', OLD.account_id, 'DELETE');
END;

-- --- APIKEYS TRIGGERS ---
CREATE TRIGGER trg_apikeys_insert AFTER INSERT ON APIKeys BEGIN
    INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('APIKeys', NEW.key_id, 'INSERT');
END;

CREATE TRIGGER trg_apikeys_update AFTER UPDATE ON APIKeys BEGIN
    UPDATE APIKeys SET updated_at = CURRENT_TIMESTAMP WHERE key_id = NEW.key_id;
    INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('APIKeys', NEW.key_id, 'UPDATE');
END;

CREATE TRIGGER trg_apikeys_delete AFTER DELETE ON APIKeys BEGIN
    INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('APIKeys', OLD.key_id, 'DELETE');
END;
