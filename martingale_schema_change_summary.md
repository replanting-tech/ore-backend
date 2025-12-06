# Martingale Schema Change Summary

## Overview
Changed the martingale_strategies table to store wallet_address directly instead of using user_id for all operations, as requested by the user.

## Database Migration

### File: `migration_add_wallet_address_to_martingale.sql`
- Added `wallet_address VARCHAR(44)` column to martingale_strategies table
- Created index `idx_martingale_wallet_address` for better query performance
- Updated existing records by joining with users table
- Made the column NOT NULL after populating data

### SQL Migration Script:
```sql
-- Add the new column
ALTER TABLE martingale_strategies ADD COLUMN wallet_address VARCHAR(44);

-- Create index for better query performance
CREATE INDEX idx_martingale_wallet_address ON martingale_strategies(wallet_address);

-- Update existing records by joining with users table
UPDATE martingale_strategies 
SET wallet_address = users.wallet_address
FROM users 
WHERE martingale_strategies.user_id = users.id;

-- Make the column NOT NULL after populating data
ALTER TABLE martingale_strategies ALTER COLUMN wallet_address SET NOT NULL;
```

## Backend Code Changes

### 1. Updated MartingaleStrategy Struct
**Location:** `src/main.rs` lines 498-516
- Added `wallet_address: String` field to the struct

### 2. Updated Database Schema Setup
**Location:** `src/main.rs` lines 1347-1370
- Added `wallet_address VARCHAR(44) NOT NULL` to table creation
- Added `CREATE INDEX idx_martingale_wallet_address ON martingale_strategies(wallet_address)`

### 3. Updated API Functions

#### `get_active_martingale` function
**Location:** `src/main.rs` lines 1670-1680
- **Before:** Used JOIN with users table
- **After:** Direct query using wallet_address
```sql
SELECT * FROM martingale_strategies
WHERE wallet_address = $1 AND status = 'active'
ORDER BY created_at DESC
LIMIT 1
```

#### `get_martingale_progress` function
**Location:** `src/main.rs` lines 1767-1777
- **Before:** Used JOIN with users table
- **After:** Direct query using wallet_address
```sql
SELECT * FROM martingale_strategies
WHERE wallet_address = $1 AND status = 'active'
ORDER BY created_at DESC
LIMIT 1
```

#### `start_martingale` function
**Location:** `src/main.rs` lines 1913-1941
- **Before:** Checked for existing strategy using user_id
- **After:** Checked using wallet_address
- **Before:** INSERT without wallet_address
- **After:** INSERT with wallet_address field

## API Endpoints (Unchanged)

All existing API endpoints continue to work exactly as before:
- `GET /api/martingale/active/{wallet_address}` ✅
- `GET /api/martingale/progress/{wallet_address}` ✅
- `POST /api/martingale/start` ✅

## Benefits of This Change

1. **Simpler Queries**: No more JOINs needed for basic martingale operations
2. **Better Performance**: Direct wallet_address queries with dedicated index
3. **Direct Database Access**: Users can query martingale_strategies table directly using wallet addresses
4. **Consistency**: wallet_address is now the primary identifier for martingale strategies

## Migration Steps Required

To deploy these changes:

1. **Run the migration script**:
   ```bash
   psql -d your_database -f migration_add_wallet_address_to_martingale.sql
   ```

2. **Restart the backend application** to pick up the new schema

3. **Verify the changes**:
   ```sql
   SELECT id, wallet_address, status, created_at 
   FROM martingale_strategies 
   LIMIT 5;
   ```

## Important Notes

- The user_id column remains in the table for backward compatibility
- Existing martingale strategies will be automatically migrated by the SQL script
- All API endpoints remain unchanged - this is a backend optimization
- The wallet_address column has a NOT NULL constraint and dedicated index

## Testing

After deployment, test these endpoints:
```bash
# Test getting active martingale strategy
curl http://localhost:3000/api/martingale/active/5cgUqstqvmM9KKiBLaitNwkHLbjYuUrsTzVFzuBzFSUP

# Test getting martingale progress  
curl http://localhost:3000/api/martingale/progress/5cgUqstqvmM9KKiBLaitNwkHLbjYuUrsTzVFzuBzFSUP
```

Both should work exactly as before, but now the backend uses direct wallet_address queries instead of user_id lookups.