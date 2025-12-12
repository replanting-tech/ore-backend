# ORE Mining API - Deployment Fixes Documentation

## Overview

This document outlines the fixes applied to the ORE Mining API deployment system to resolve issues and match the successful CLI deployment pattern.

## Issues Fixed

### 1. **Miner PDA Calculation Error**
- **Problem**: The API was incorrectly parsing the keypair path as a wallet pubkey when checking miner accounts
- **Fix**: Now correctly parses the wallet address from the request to calculate the miner PDA
- **Impact**: Prevents checkpoint failures due to wrong wallet references

### 2. **Priority Fee Support**
- **Problem**: API didn't support priority fees like the CLI does
- **Fix**: Added `deploy_ore_with_priority_fee` function with priority fee parameter
- **Impact**: Now matches CLI behavior with `PRIORITY_FEE=0` support

### 3. **Square Validation Enhancement**
- **Problem**: Square ID validation was insufficient
- **Fix**: Added proper validation for square IDs (0-24) and ensured at least one square is selected
- **Impact**: Prevents invalid square selections that could cause transaction failures

### 4. **Improved Checkpoint Logic**
- **Problem**: Checkpoint function had poor error handling and didn't check if checkpoint was needed
- **Fix**: 
  - Added check to skip checkpoint if already done for current round
  - Better error handling for missing miner accounts
  - Proper authority parameter usage
- **Impact**: More efficient deployment flow matching CLI behavior

### 5. **Enhanced Logging and Error Handling**
- **Problem**: Limited logging made debugging deployment issues difficult
- **Fix**: Added comprehensive logging throughout deployment process
- **Impact**: Better visibility into deployment flow and easier troubleshooting

## Key Changes

### New Function: `deploy_ore_with_priority_fee`
```rust
pub async fn deploy_ore_with_priority_fee(
    rpc: &RpcClient,
    keypair_path: &str,
    amount: u64,
    square_ids: Option<Vec<u64>>,
    priority_fee_microlamports: u64,
) -> Result<String, ApiError>
```

### Updated Deploy Function
- Now uses the new priority fee function with `priority_fee=0` (matching CLI)
- Fixed miner PDA calculation using correct wallet address
- Enhanced error handling and logging

### Updated Martingale Deployment
- Fixed checkpoint authority to use correct wallet
- Uses improved deployment function
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

### API Pattern (Now Fixed):
```javascript
// 1. Checkpoint (automatic in deploy flow)
POST /api/checkpoint

// 2. Deploy (includes automatic checkpoint + priority fee = 0)
POST /api/deploy
{
  "wallet_address": "...",
  "amount": 10000,
  "square_ids": [0,1,2,4,7,9,10,11,12,13,14,15,17,20,22,23,24]
}
```

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

1. **Miner Account Check**: `"Miner account not found, skipping checkpoint"` or `"Checkpointing miner before deploy for wallet: [address]"`
2. **Priority Fee**: `"Sending transaction with priority fee: 0 microlamports"`
3. **Square Selection**: `"Deploying [amount] lamports to squares: [list]"`
4. **Transaction Success**: `"Deploy transaction confirmed: [signature]"`

## Rollback Plan

If issues arise, the previous deployment functions (`deploy_ore_multiple`, original checkpoint logic) are still available and can be reverted by:
1. Updating the deploy function to call `deploy_ore_multiple` instead of `deploy_ore_with_priority_fee`
2. Reverting checkpoint logic to use `None` authority parameter

## Next Steps

1. Test on mainnet with small amounts
2. Monitor transaction success rates
3. Consider adding deployment queue for high-volume scenarios
4. Implement transaction retry logic for failed deployments
