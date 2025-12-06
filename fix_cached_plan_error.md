# Fix: "cached plan must not change result type" Error

## Problem
PostgreSQL error occurs because:
- Added `wallet_address` column changed the query result structure
- PostgreSQL cached query plans from before the schema change
- Application queries now return different column structure than cached plans expect

## Solution Options

### Option 1: Restart Application (Recommended)
Simply restart your backend application to clear all cached query plans:

```bash
# If running with cargo
cargo run

# Or if running as a service
sudo systemctl restart your-ore-backend-service
```

### Option 2: Clear PostgreSQL Query Cache
Execute this SQL command to clear cached plans:

```sql
-- Clear all cached query plans
DISCARD ALL;

-- Or restart PostgreSQL (more aggressive)
-- sudo systemctl restart postgresql
```

### Option 3: Connection Pool Already Updated ✅
I've already updated your database connection pool with `test_before_acquire(true)` which will:
- Test connections before acquiring them
- Automatically detect and handle schema changes
- Prevent cached plan issues in the future

This should resolve the current error and prevent future occurrences.

## Verification
After applying any solution above, test the API:

```bash
curl http://localhost:3000/api/martingale/active/5cgUqstqvmM9KKiBLaitNwkHLbjYuUrsTzVFzuBzFSUP
```

Should return successful response without cached plan errors.

## Prevention
To avoid this in future schema changes:
1. Always restart applications after database schema changes
2. Use migration scripts that handle backward compatibility
3. Add version flags to schema changes for gradual rollout