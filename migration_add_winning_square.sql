-- Migration to add winning_square column to mining_sessions table
-- Run this SQL on your PostgreSQL database

ALTER TABLE mining_sessions ADD COLUMN winning_square INTEGER;

-- Optional: Add comment for documentation
COMMENT ON COLUMN mining_sessions.winning_square IS 'The winning square for this round (0-24)';