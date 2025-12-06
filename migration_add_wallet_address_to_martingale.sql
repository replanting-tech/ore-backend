-- Migration: Add wallet_address column to martingale_strategies table
-- This change allows direct wallet-based queries without joining with users table

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

-- Verify the migration
SELECT id, user_id, wallet_address, status, created_at 
FROM martingale_strategies 
LIMIT 5;