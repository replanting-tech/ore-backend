# Martingale Strategy Documentation

## Overview
The Martingale strategy is an automated ORE mining system that increases deployment amounts after losses to potentially recover losses and achieve profitability.

## How It Works

### Basic Strategy
1. **Start**: Deploy base amount (e.g., 0.1 SOL)
2. **If Loss**: Next round deploys `base_amount × loss_multiplier` (e.g., 0.2 SOL)
3. **If Loss Again**: Next round deploys `0.2 × 2.0 = 0.4 SOL`
4. **If Win**: Next round resets to `base_amount` (0.1 SOL)
5. **Stop**: When max loss reached or all rounds completed

### Example Flow
```
Round 1: Deploy 0.1 SOL → LOSS
Round 2: Deploy 0.2 SOL → WIN  
Round 3: Deploy 0.1 SOL → LOSS
Round 4: Deploy 0.2 SOL → LOSS
Round 5: Deploy 0.4 SOL → WIN
```

## API Endpoints

### 1. Start Martingale Strategy

**Endpoint:** `POST /api/martingale/start`

**Request Body:**
```json
{
  "wallet_address": "Your44CharWalletAddress",
  "rounds": 5,
  "base_amount_sol": 0.1,
  "loss_multiplier": 2.0,
  "max_loss_sol": 2.0
}
```

**Parameters:**
- `rounds`: Number of rounds to play (1-100)
- `base_amount_sol`: Starting amount in SOL (0.001-10.0)
- `loss_multiplier`: Amount to multiply after loss (1.1-10.0)
- `max_loss_sol`: Maximum total loss allowed (0.1-100.0)

**Response:**
```json
{
  "success": true,
  "strategy_id": "uuid-here",
  "message": "Martingale strategy started"
}
```

### 2. Get Active Strategy

**Endpoint:** `GET /api/martingale/active/{wallet_address}`

**Response:**
```json
{
  "success": true,
  "strategy": {
    "id": "uuid",
    "rounds": 5,
    "base_amount_sol": 0.1,
    "loss_multiplier": 2.0,
    "status": "active",
    "current_round": 2,
    "current_amount_sol": 0.2,
    "total_deployed_sol": 0.3,
    "total_rewards_sol": 0.25,
    "total_loss_sol": 0.05
  }
}
```

### 3. Get Detailed Progress

**Endpoint:** `GET /api/martingale/progress/{wallet_address}`

**Response:**
```json
{
  "success": true,
  "strategy_progress": {
    "current_round": 2,
    "total_rounds": 5,
    "remaining_rounds": 3,
    "progress_percentage": 40.0,
    "next_deployment": {
      "amount_sol": 0.4,
      "round_id": 12346
    },
    "totals": {
      "deployed_sol": 0.3,
      "rewards_sol": 0.25,
      "profit_loss_sol": -0.05,
      "overall_roi_percentage": -16.67
    },
    "risk_management": {
      "max_loss_limit_sol": 2.0,
      "current_loss_sol": 0.05,
      "remaining_loss_budget_sol": 1.95,
      "risk_level": "LOW"
    },
    "performance": {
      "wins": 0,
      "losses": 2,
      "win_rate_percentage": 0.0
    },
    "round_history": [...]
  }
}
```

### 4. Get Active Mining Sessions

**Endpoint:** `GET /api/mining/active/{wallet_address}`

**Response:**
```json
{
  "success": true,
  "active_sessions": [
    {
      "round_id": 12345,
      "deployed_amount_sol": 0.1,
      "rewards_sol": 0.15,
      "profit_loss_sol": 0.05,
      "profitability": "win",
      "status": "active"
    }
  ],
  "total_active": 1
}
```

## Frontend Integration

### Starting a Strategy
```javascript
async function startMartingaleStrategy(walletAddress, rounds, baseAmount, multiplier, maxLoss) {
  const response = await fetch('/api/martingale/start', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      wallet_address: walletAddress,
      rounds: rounds,
      base_amount_sol: baseAmount,
      loss_multiplier: multiplier,
      max_loss_sol: maxLoss
    })
  });
  
  return await response.json();
}
```

### Monitoring Progress
```javascript
async function getStrategyProgress(walletAddress) {
  const response = await fetch(`/api/martingale/progress/${walletAddress}`);
  return await response.json();
}

// Poll every 30 seconds
setInterval(() => {
  getStrategyProgress(walletAddress).then(data => {
    if (data.success) {
      updateUI(data.strategy_progress);
    }
  });
}, 30000);
```

### UI Update Function
```javascript
function updateUI(progress) {
  // Progress bar
  document.getElementById('progress').style.width = `${progress.progress_percentage}%`;
  
  // Current round
  document.getElementById('current-round').textContent = `${progress.current_round}/${progress.total_rounds}`;
  
  // Next deployment
  document.getElementById('next-amount').textContent = `${progress.next_deployment.amount_sol} SOL`;
  
  // Profit/Loss
  const pnl = progress.totals.profit_loss_sol;
  document.getElementById('pnl').textContent = `${pnl > 0 ? '+' : ''}${pnl.toFixed(4)} SOL`;
  document.getElementById('pnl').className = pnl >= 0 ? 'profit' : 'loss';
  
  // Risk level
  const riskElement = document.getElementById('risk-level');
  riskElement.textContent = progress.risk_management.risk_level;
  riskElement.className = `risk-${progress.risk_management.risk_level.toLowerCase()}`;
  
  // Win rate
  document.getElementById('win-rate').textContent = `${progress.performance.win_rate_percentage.toFixed(1)}%`;
}
```

## Strategy Parameters

### Conservative (Low Risk)
- Rounds: 5
- Base Amount: 0.1 SOL
- Loss Multiplier: 2.0
- Max Loss: 2.0 SOL

### Moderate (Medium Risk)
- Rounds: 10
- Base Amount: 0.05 SOL
- Loss Multiplier: 2.5
- Max Loss: 1.0 SOL

### Aggressive (High Risk)
- Rounds: 20
- Base Amount: 0.01 SOL
- Loss Multiplier: 3.0
- Max Loss: 5.0 SOL

## Important Notes

1. **One Strategy Per User**: Each wallet can only have one active strategy
2. **Automatic Execution**: Strategy runs automatically once started
3. **Risk Management**: Stops automatically when max loss reached
4. **Blockchain Dependent**: Follows ORE blockchain round timing
5. **No Manual Intervention**: Cannot modify strategy after starting

## Error Handling

```javascript
try {
  const result = await startMartingaleStrategy(params);
  if (result.success) {
    showSuccess('Strategy started successfully');
  } else {
    showError(result.error || 'Failed to start strategy');
  }
} catch (error) {
  showError('Network error: ' + error.message);
}
```

## Status Types

- **active**: Strategy currently running
- **completed**: All rounds finished successfully
- **stopped**: Strategy stopped due to loss limit or error

This documentation should help frontend developers integrate the Martingale strategy functionality into their applications.