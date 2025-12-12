# ORE Mining API - Deployment Fixes Documentation

## Overview

This document outlines the fixes applied to the ORE Mining API deployment system to resolve issues and match the successful CLI deployment pattern.

## Critical Fix: Blockhash Not Found Error Resolution

### The Problem
```
{
    "error": "Checkpoint transaction failed: RPC response error -32002: Transaction simulation failed: Blockhash not found; "
}
```

**Root Cause**: Solana blockhashes expire after ~60-90 seconds. When the API performs a checkpoint transaction, it:
1. Gets a blockhash
2. Creates and signs the transaction
3. But by the time it tries to send it, the blockhash has expired

### The Solution: Auto-Checkpoint with Blockhash Refresh

**NEW: `auto_checkpoint_miner()` function:**
- ✅ Gets fresh blockhash right before sending each transaction
- ✅ Implements retry logic (up to 3 attempts) when blockhash expires
- ✅ Automatically skips checkpoint if already done for current round
- ✅ Handles missing miner accounts gracefully

**NEW: `deploy_ore_with_auto_retry()` function:**
- ✅ Gets fresh blockhash right before sending each deployment transaction
- ✅ Implements retry logic (up to 3 attempts) when blockhash expires
- ✅ Maintains priority fee = 0 to match CLI behavior

### How It Works Now

1. **Auto-Checkpoint** (before deploy):
   - Checks if miner needs checkpointing
   - Gets fresh blockhash
   - Sends checkpoint transaction
   - Retries up to 3 times if blockhash expires

2. **Auto-Retry Deploy**:
   - Gets fresh blockhash
   - Sends deployment transaction
   - Retries up to 3 times if blockhash expires

**Result**: No more "Blockhash not found" errors! 🎉

## Issues Fixed

### 1. **Auto-Checkpoint Implementation**
- **Problem**: "Blockhash not found" errors during checkpoint transactions due to expired blockhashes
- **Fix**: Implemented `auto_checkpoint_miner()` function with automatic blockhash refresh and retry logic
- **Impact**: Prevents checkpoint transaction failures and ensures reliable auto-checkpointing

### 2. **Auto-Retry Deployment System**
- **Problem**: "Blockhash not found" errors during deployment transactions due to expired blockhashes
- **Fix**: Implemented `deploy_ore_with_auto_retry()` function with automatic blockhash refresh and retry logic
- **Impact**: Prevents deployment transaction failures with automatic retry on blockhash expiration

### 3. **Miner PDA Calculation Error**
- **Problem**: The API was incorrectly parsing the keypair path as a wallet pubkey when checking miner accounts
- **Fix**: Now correctly parses the wallet address from the request to calculate the miner PDA
- **Impact**: Prevents checkpoint failures due to wrong wallet references

### 4. **Priority Fee Support**
- **Problem**: API didn't support priority fees like the CLI does
- **Fix**: Added `deploy_ore_with_priority_fee` function with priority fee parameter
- **Impact**: Now matches CLI behavior with `PRIORITY_FEE=0` support

### 5. **Square Validation Enhancement**
- **Problem**: Square ID validation was insufficient
- **Fix**: Added proper validation for square IDs (0-24) and ensured at least one square is selected
- **Impact**: Prevents invalid square selections that could cause transaction failures

### 6. **Enhanced Logging and Error Handling**
- **Problem**: Limited logging made debugging deployment issues difficult
- **Fix**: Added comprehensive logging throughout deployment process
- **Impact**: Better visibility into deployment flow and easier troubleshooting

## Key Changes

### New Function: `auto_checkpoint_miner`
```rust
pub async fn auto_checkpoint_miner(
    rpc: &RpcClient,
    keypair_path: &str,
    authority: Option<Pubkey>,
) -> Result<String, ApiError>
```
- Automatically handles blockhash refresh for checkpoint transactions
- Implements retry logic (up to 3 attempts) for "Blockhash not found" errors
- Prevents checkpoint failures due to expired blockhashes

### New Function: `deploy_ore_with_auto_retry`
```rust
pub async fn deploy_ore_with_auto_retry(
    rpc: &RpcClient,
    keypair_path: &str,
    amount: u64,
    square_ids: Option<Vec<u64>>,
    priority_fee_microlamports: u64,
) -> Result<String, ApiError>
```
- Automatically handles blockhash refresh for deployment transactions
- Implements retry logic (up to 3 attempts) for "Blockhash not found" errors
- Prevents deployment failures due to expired blockhashes

### Updated Deploy Function
- Now uses `auto_checkpoint_miner()` for automatic checkpointing
- Now uses `deploy_ore_with_auto_retry()` for deployment with retry logic
- Fixed miner PDA calculation using correct wallet address
- Enhanced error handling and logging

### Updated Martingale Deployment
- Fixed checkpoint authority to use correct wallet
- Uses improved auto-checkpoint and auto-retry deployment functions
- Better error handling and logging

## CLI Compatibility

The API now matches the CLI deployment pattern:

### CLI Pattern (Working):
```bash
# 1. Checkpoint
KEYPAIR=~/.config/solana/id.json RPC=https://api.mainnet-beta.solana.com COMMAND=checkpoint cargo run --release --bin ore-cli

# 2. Deploy
PRIORITY_FEE=0 KEYPAIR=~/.config/solana/id.json RPC=https://api.mainnet-beta.solana.com COMMAND=deploy AMOUNT=10000 SQUARE=0,1,2,4,7,9,10,11,12,13,14,15,17,20,22,23,24 cargo run --release --bin ore-cli
```

### API Pattern (Now Fixed with Auto-Checkpoint):
```javascript
// Deploy endpoint now automatically handles:
// 1. Auto-checkpoint with blockhash refresh and retry
// 2. Auto-retry deployment with blockhash refresh
// 3. Priority fee = 0 (matching CLI)

POST /api/deploy
{
  "wallet_address": "...",
  "amount": 10000,
  "square_ids": [0,1,2,4,7,9,10,11,12,13,14,15,17,20,22,23,24]
}

// Manual checkpoint still available (with auto-retry)
POST /api/checkpoint
```

**What happens automatically:**
1. **Auto-Checkpoint**: Checks if miner needs checkpoint, performs it with retry logic
2. **Blockhash Refresh**: Gets fresh blockhash before each transaction attempt
3. **Auto-Retry**: Retries up to 3 times if blockhash expires
4. **Deploy**: Performs deployment with retry logic

## Testing the Fixes

### 1. Test Individual Endpoints

**Checkpoint Test:**
```bash
curl -X POST http://localhost:3000/api/checkpoint \
  -H "Content-Type: application/json"
```

**Deploy Test:**
```bash
curl -X POST http://localhost:3000/api/deploy \
  -H "Content-Type: application/json" \
  -d '{
    "wallet_address": "YourWalletAddress",
    "amount": 10000,
    "square_ids": [0,1,2,4,7,9,10,11,12,13,14,15,17,20,22,23,24]
  }'
```

### 2. Test Square Validation
Try invalid square IDs to ensure proper validation:
```bash
curl -X POST http://localhost:3000/api/deploy \
  -H "Content-Type: application/json" \
  -d '{
    "wallet_address": "YourWalletAddress",
    "amount": 10000,
    "square_ids": [25, 30]  # Should fail - invalid IDs
  }'
```

### 3. Check Logs
Monitor application logs for deployment flow:
```bash
# Check for deployment logs showing:
# - Correct miner PDA calculation
# - Priority fee usage (0 microlamports)
# - Square validation results
# - Transaction success confirmations
```

## Environment Variables

Ensure these are set correctly:
```bash
RPC_URL=https://api.mainnet-beta.solana.com
KEYPAIR_PATH=~/.config/solana/id.json
DATABASE_URL=postgresql://...
REDIS_URL=redis://...
```

## Monitoring

Watch for these log messages to confirm fixes are working:

1. **Auto-Checkpoint**: `"Auto-checkpointing miner before deploy for wallet: [address]"` or `"Auto-checkpoint completed: [signature]"`
2. **Blockhash Retry**: `"Blockhash expired on attempt X, retrying with fresh blockhash"`
3. **Auto-Retry Deploy**: `"Auto-retry deploying [amount] lamports to squares: [list]"`
4. **Transaction Success**: `"Auto-retry deploy transaction confirmed on attempt X: [signature]"`
5. **Already Checkpointed**: `"Miner already checkpointed for round X, skipping auto-checkpoint"`

## Rollback Plan

If issues arise, the previous deployment functions (`deploy_ore_multiple`, original checkpoint logic) are still available and can be reverted by:
1. Updating the deploy function to call `deploy_ore_multiple` instead of `deploy_ore_with_priority_fee`
2. Reverting checkpoint logic to use `None` authority parameter

## Next Steps

1. ✅ **Auto-Checkpoint Implemented**: Now automatically handles blockhash refresh for checkpoint transactions
2. ✅ **Auto-Retry Deployment**: Now automatically retries deployment on blockhash expiration
3. Test on mainnet with small amounts
4. Monitor transaction success rates and retry counts
5. Consider adding deployment queue for high-volume scenarios
6. Consider implementing priority fee optimization based on network conditions
