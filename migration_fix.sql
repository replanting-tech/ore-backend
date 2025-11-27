-- Migration to fix column types for martingale_strategies table
-- Run this SQL to alter existing columns from DECIMAL to DOUBLE PRECISION

ALTER TABLE martingale_strategies 
ALTER COLUMN base_amount_sol TYPE DOUBLE PRECISION USING base_amount_sol::DOUBLE PRECISION;

ALTER TABLE martingale_strategies 
ALTER COLUMN loss_multiplier TYPE DOUBLE PRECISION USING loss_multiplier::DOUBLE PRECISION;

ALTER TABLE martingale_strategies 
ALTER COLUMN max_loss_sol TYPE DOUBLE PRECISION USING max_loss_sol::DOUBLE PRECISION;

ALTER TABLE martingale_strategies 
ALTER COLUMN current_amount_sol TYPE DOUBLE PRECISION USING current_amount_sol::DOUBLE PRECISION;

ALTER TABLE martingale_strategies 
ALTER COLUMN total_deployed_sol TYPE DOUBLE PRECISION USING total_deployed_sol::DOUBLE PRECISION;

ALTER TABLE martingale_strategies 
ALTER COLUMN total_rewards_sol TYPE DOUBLE PRECISION USING total_rewards_sol::DOUBLE PRECISION;

ALTER TABLE martingale_strategies 
ALTER COLUMN total_loss_sol TYPE DOUBLE PRECISION USING total_loss_sol::DOUBLE PRECISION;