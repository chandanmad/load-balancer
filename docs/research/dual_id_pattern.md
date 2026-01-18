# Dual ID Pattern for APIKeys Table

## Problem Statement

The `ChangeLog` table needs to reference records from multiple tables:

| Table | Primary Key | Type |
|-------|-------------|------|
| Plans | `plan_id` | `INTEGER` |
| Accounts | `account_id` | `INTEGER` |
| APIKeys | `api_key_id` | `CHAR(36)` (UUID) |

Having mixed primary key types creates issues:
1. `ChangeLog.record_id` must be `TEXT` to accommodate UUIDs, losing type consistency
2. Larger index sizes for UUID-based foreign keys
3. More complex Rust code to handle different ID types

---

## Solution: Dual ID Pattern

Add an internal `INTEGER` primary key while keeping the UUID as a unique external identifier:

```sql
CREATE TABLE APIKeys (
    key_id INTEGER PRIMARY KEY AUTOINCREMENT,  -- Internal ID
    api_key_id CHAR(36) UNIQUE NOT NULL,       -- External UUID
    account_id INTEGER NOT NULL,
    api_key_hash TEXT UNIQUE NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT 1,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (account_id) REFERENCES Accounts(account_id)
);
```

---

## Benefits

| Aspect | Benefit |
|--------|---------|
| **Consistent ChangeLog** | All tables use `INTEGER` primary keys → `record_id INTEGER` works uniformly |
| **Smaller indexes** | INTEGER (8 bytes) vs CHAR(36) (36 bytes) - faster joins and lookups |
| **Simpler Rust code** | `key_id: i64` everywhere, parse UUID only when needed |
| **Decoupled identifiers** | Internal ID (`key_id`) vs External ID (`api_key_id`) - good for security |
| **Future flexibility** | Can change UUID format without changing internal references |

---

## Trade-offs

| Concern | Mitigation |
|---------|------------|
| Two IDs to manage | Clear naming: `key_id` (internal), `api_key_id` (external/user-facing) |
| Extra column storage | Negligible - INTEGER is only 8 bytes |
| Which ID to use where? | **Rule:** Use `key_id` for internal references, `api_key_id` in API responses |

---

## Updated Schema Design

### APIKeys Table

```sql
CREATE TABLE APIKeys (
    api_key_id INTEGER PRIMARY KEY AUTOINCREMENT,
    api_key CHAR(36) UNIQUE NOT NULL,         -- UUID for external use
    account_id INTEGER NOT NULL,
    api_key_hash TEXT UNIQUE NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT 1,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (account_id) REFERENCES Accounts(account_id)
);
```

### ChangeLog Table

```sql
-- ChangeLog can now use INTEGER consistently for all tables
CREATE TABLE ChangeLog (
    change_id INTEGER PRIMARY KEY AUTOINCREMENT,
    table_name TEXT NOT NULL,
    record_id INTEGER NOT NULL,               -- Works uniformly!
    operation TEXT NOT NULL,
    occurred_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

### Updated Triggers

```sql
-- Triggers use api_key_id (INTEGER) for ChangeLog
CREATE TRIGGER trg_apikeys_insert AFTER INSERT ON APIKeys BEGIN
    INSERT INTO ChangeLog (table_name, record_id, operation) 
    VALUES ('APIKeys', NEW.api_key_id, 'INSERT');
END;

CREATE TRIGGER trg_apikeys_update AFTER UPDATE ON APIKeys BEGIN
    INSERT INTO ChangeLog (table_name, record_id, operation) 
    VALUES ('APIKeys', NEW.api_key_id, 'UPDATE');
END;

CREATE TRIGGER trg_apikeys_delete AFTER DELETE ON APIKeys BEGIN
    INSERT INTO ChangeLog (table_name, record_id, operation) 
    VALUES ('APIKeys', OLD.api_key_id, 'DELETE');
END;
```

---

## Usage Pattern

| Context | Use This ID |
|---------|-------------|
| ChangeLog `record_id` | `api_key_id` (INTEGER) |
| Foreign keys from other tables | `api_key_id` (INTEGER) |
| API responses to users | `api_key` (UUID) |
| Token format | `api_key` (UUID) embedded in token |
| Database queries/joins | `api_key_id` (faster) |
| Lookup by token | Query by `api_key` or `api_key_hash` |

---

## Rust Struct Updates

### Before

```rust
pub struct ApiKey {
    pub api_key_id: Uuid,        // Was primary key (UUID)
    pub account_id: i64,
    pub api_key_hash: String,
    pub is_active: bool,
}
```

### After

```rust
pub struct ApiKey {
    pub api_key_id: i64,         // Internal primary key (INTEGER)
    pub api_key: Uuid,           // External UUID identifier
    pub account_id: i64,
    pub api_key_hash: String,
    pub is_active: bool,
}
```

---

## Migration Considerations

If migrating from UUID-only primary key:

1. Add `key_id` column with `AUTOINCREMENT`
2. Keep `api_key_id` as `UNIQUE NOT NULL`
3. Update triggers to use `key_id` for ChangeLog
4. Update Rust code to use `key_id` for internal lookups
5. Usage tracking can continue to use `api_key_id` if preferred for external reporting

---

## Conclusion

The dual ID pattern:
- ✅ Maintains UUID for external/API use
- ✅ Uses INTEGER for internal consistency
- ✅ Simplifies ChangeLog design
- ✅ Improves query performance
- ✅ Follows production best practices

**Recommendation:** Implement this pattern for the APIKeys table.
