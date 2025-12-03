# ORE Mining Backend API Documentation

Base URL: `http://localhost:3000`

## REST API Endpoints

### GET /
**Description:** Root endpoint

**Response:** Plain text

**Curl:**
```bash
curl http://localhost:3000/
```

---

### GET /health
**Description:** Health check endpoint

**Response:** JSON
```json
{
  "status": "healthy",
  "timestamp": "2025-12-01T18:12:55.101Z"
}
```

**Curl:**
```bash
curl http://localhost:3000/health
```

---

### GET /api/board
**Description:** Get current board information

**Response:** JSON (BoardInfo)
```json
{
  "round_id": 12345,
  "start_slot": 1000,
  "end_slot": 1150,
  "current_slot": 1100,
  "time_remaining_sec": 30.5
}
```

**Curl:**
```bash
curl http://localhost:3000/api/board
```

---

### GET /api/treasury
**Description:** Get treasury information

**Response:** JSON (TreasuryInfo)
```json
{
  "address": "TreasuryPubkey...",
  "balance_sol": 10000.0,
  "motherlode_ore": 50000.0,
  "total_staked": 100000.0,
  "total_unclaimed": 5000.0,
  "total_refined": 20000.0
}
```

**Curl:**
```bash
curl http://localhost:3000/api/treasury
```

---

### GET /api/miner/{wallet}
**Description:** Get miner statistics for wallet

**Parameters:**
- `wallet`: Wallet address (44 characters)

**Response:** JSON (MinerStats)
```json
{
  "address": "MinerPDA...",
  "authority": "WalletAddress...",
  "rewards_sol": 5.0,
  "rewards_ore": 100.0,
  "refined_ore": 50.0,
  "round_id": 12345,
  "checkpoint_id": 12344,
  "lifetime_rewards_sol": 25.0,
  "lifetime_rewards_ore": 500.0,
  "deployed": [1000, 1000, ...],
  "cumulative": [5000, 5000, ...]
}
```

**Curl:**
```bash
curl http://localhost:3000/api/miner/{wallet_address}
```

---

### GET /api/miner/{wallet}/sessions
**Description:** Get mining sessions for wallet

**Parameters:**
- `wallet`: Wallet address (44 characters)

**Response:** JSON Array (MiningSession[])
```json
[
  {
    "id": "uuid",
    "user_id": "uuid",
    "round_id": 12345,
    "deployed_amount": 1000000000,
    "squares": [0,1,2,...],
    "status": "active",
    "rewards_sol": 2000000000,
    "rewards_ore": 50,
    "claimed": false,
    "profitability": "win",
    "created_at": "2025-12-01T18:00:00Z",
    "updated_at": "2025-12-01T18:00:00Z"
  }
]
```

**Curl:**
```bash
curl http://localhost:3000/api/miner/{wallet_address}/sessions
```

---

### GET /api/miner/{wallet}/history
**Description:** Get deployment history for wallet

**Parameters:**
- `wallet`: Wallet address (44 characters)

**Response:** JSON
```json
{
  "wallet_address": "WalletAddress...",
  "total_deploys": 10,
  "stats": {
    "wins": 6,
    "losses": 3,
    "breakevens": 1,
    "pending": 0,
    "win_rate_percentage": 60.0
  },
  "totals": {
    "deployed_sol": 10.0,
    "rewards_sol": 12.0
  },
  "sessions": [...]
}
```

**Curl:**
```bash
curl http://localhost:3000/api/miner/{wallet_address}/history
```

---

### POST /api/user/register
**Description:** Register or get user by wallet

**Request:** JSON
```json
{
  "wallet_address": "44-character-wallet-address"
}
```

**Response:** JSON
```json
{
  "success": true,
  "user_id": "uuid",
  "wallet_address": "WalletAddress...",
  "created_at": "2025-12-01T18:00:00Z"
}
```

**Curl:**
```bash
curl -X POST http://localhost:3000/api/user/register \
  -H "Content-Type: application/json" \
  -d '{"wallet_address":"44-character-wallet-address"}'
```

---

### POST /api/deploy
**Description:** Deploy ORE

**Request:** JSON (DeployRequest)
```json
{
  "wallet_address": "44-character-wallet-address",
  "amount": 1.0,
  "square_id": null
}
```

**Response:** JSON
```json
{
  "success": true,
  "signature": "TransactionSignature...",
  "amount": 1.0,
  "square": null,
  "user_id": "uuid"
}
```

**Curl:**
```bash
curl -X POST http://localhost:3000/api/deploy \
  -H "Content-Type: application/json" \
  -d '{"wallet_address":"44-character-wallet-address","amount":1.0,"square_id":null}'
```

---

### POST /api/claim
**Description:** Claim rewards

**Request:** None

**Response:** JSON (ClaimResponse)
```json
{
  "signature": "TransactionSignature...",
  "sol_claimed": 2.5,
  "ore_claimed": 50.0
}
```

**Curl:**
```bash
curl -X POST http://localhost:3000/api/claim
```

---

### POST /api/checkpoint
**Description:** Checkpoint miner

**Request:** None

**Response:** JSON
```json
{
  "success": true,
  "signature": "TransactionSignature..."
}
```

**Curl:**
```bash
curl -X POST http://localhost:3000/api/checkpoint
```

---

### POST /api/martingale/start
**Description:** Start martingale strategy

**Request:** JSON (StartMartingaleRequest)
```json
{
  "wallet_address": "44-character-wallet-address",
  "rounds": 10,
  "base_amount_sol": 1.0,
  "loss_multiplier": 2.0,
  "max_loss_sol": 10.0
}
```

**Response:** JSON
```json
{
  "success": true,
  "strategy_id": "uuid",
  "message": "Martingale strategy started"
}
```

**Curl:**
```bash
curl -X POST http://localhost:3000/api/martingale/start \
  -H "Content-Type: application/json" \
  -d '{"wallet_address":"44-character-wallet-address","rounds":10,"base_amount_sol":1.0,"loss_multiplier":2.0,"max_loss_sol":10.0}'
```

---

### GET /api/martingale/active/{wallet}
**Description:** Get active martingale strategy

**Parameters:**
- `wallet`: Wallet address (44 characters)

**Response:** JSON
```json
{
  "success": true,
  "strategy": {
    "id": "uuid",
    "rounds": 10,
    "base_amount_sol": 1.0,
    "loss_multiplier": 2.0,
    "max_loss_sol": 10.0,
    "status": "active",
    "current_round": 3,
    "current_amount_sol": 2.0,
    "total_deployed_sol": 3.0,
    "total_rewards_sol": 4.0,
    "total_loss_sol": 0.0,
    "last_round_id": 12347,
    "created_at": "2025-12-01T18:00:00Z",
    "updated_at": "2025-12-01T18:00:00Z"
  }
}
```

**Curl:**
```bash
curl http://localhost:3000/api/martingale/active/{wallet_address}
```

---

### GET /api/martingale/progress/{wallet}
**Description:** Get martingale progress

**Parameters:**
- `wallet`: Wallet address (44 characters)

**Response:** JSON
```json
{
  "success": true,
  "strategy_progress": {
    "id": "uuid",
    "status": "active",
    "current_round": 3,
    "total_rounds": 10,
    "remaining_rounds": 7,
    "progress_percentage": 30.0,
    "next_deployment": {
      "amount_sol": 2.0,
      "round_id": 12348
    },
    "totals": {
      "deployed_sol": 3.0,
      "rewards_sol": 4.0,
      "profit_loss_sol": 1.0,
      "overall_roi_percentage": 33.33
    },
    "risk_management": {
      "max_loss_limit_sol": 10.0,
      "current_loss_sol": 0.0,
      "remaining_loss_budget_sol": 8.0,
      "risk_level": "LOW"
    },
    "performance": {
      "total_rounds_played": 3,
      "wins": 2,
      "losses": 1,
      "breakevens": 0,
      "win_rate_percentage": 66.67
    },
    "round_history": [...],
    "created_at": "2025-12-01T18:00:00Z",
    "updated_at": "2025-12-01T18:00:00Z"
  }
}
```

**Curl:**
```bash
curl http://localhost:3000/api/martingale/progress/{wallet_address}
```

---

### GET /api/mining/active/{wallet}
**Description:** Get active mining sessions

**Parameters:**
- `wallet`: Wallet address (44 characters)

**Response:** JSON
```json
{
  "success": true,
  "active_sessions": [
    {
      "id": "uuid",
      "round_id": 12345,
      "deployed_amount_sol": 1.0,
      "rewards_sol": 1.5,
      "rewards_ore": 25.0,
      "squares": [0,1,2,...],
      "status": "active",
      "claimed": false,
      "profitability": "win",
      "created_at": "2025-12-01T18:00:00Z",
      "updated_at": "2025-12-01T18:00:00Z",
      "profit_loss_sol": 0.5
    }
  ],
  "total_active": 1
}
```

**Curl:**
```bash
curl http://localhost:3000/api/mining/active/{wallet_address}
```

---

### GET /api/stats/global
**Description:** Get global statistics

**Response:** JSON
```json
{
  "total_sessions": 100,
  "total_deploys": 100,
  "total_deployed_amount": 100000000000,
  "active_users": {
    "24h": 10,
    "7d": 25,
    "30d": 50
  }
}
```

**Curl:**
```bash
curl http://localhost:3000/api/stats/global
```

## WebSocket API

**Endpoint:** `ws://localhost:3000/ws`

**Message Format:** JSON with `type` and `data` fields

### Client Messages

#### Subscribe
```json
{
  "type": "Subscribe",
  "data": {
    "topic": "board"
  }
}
```

#### Unsubscribe
```json
{
  "type": "Unsubscribe",
  "data": {
    "topic": "board"
  }
}
```

#### Ping
```json
{
  "type": "Ping"
}
```

### Server Messages

#### Pong
```json
{
  "type": "Pong"
}
```

#### BoardUpdate
```json
{
  "type": "BoardUpdate",
  "data": {
    "round_id": 12345,
    "start_slot": 1000,
    "end_slot": 1150,
    "current_slot": 1100,
    "time_remaining_sec": 30.5
  }
}
```

#### MinerUpdate
```json
{
  "type": "MinerUpdate",
  "data": {
    "wallet": "WalletAddress...",
    "stats": {
      "address": "MinerPDA...",
      "authority": "WalletAddress...",
      "rewards_sol": 5.0,
      "rewards_ore": 100.0,
      "refined_ore": 50.0,
      "round_id": 12345,
      "checkpoint_id": 12344,
      "lifetime_rewards_sol": 25.0,
      "lifetime_rewards_ore": 500.0,
      "deployed": [1000, 1000, ...],
      "cumulative": [5000, 5000, ...]
    }
  }
}
```

#### TreasuryUpdate
```json
{
  "type": "TreasuryUpdate",
  "data": {
    "address": "TreasuryPubkey...",
    "balance_sol": 10000.0,
    "motherlode_ore": 50000.0,
    "total_staked": 100000.0,
    "total_unclaimed": 5000.0,
    "total_refined": 20000.0
  }
}
```

#### SquaresUpdate
```json
{
  "type": "SquaresUpdate",
  "data": [
    {
      "square_id": 0,
      "participants": 10,
      "competition_level": 0.5,
      "total_deployed": 1000000
    },
    ...
  ]
}
```

#### RoundComplete
```json
{
  "type": "RoundComplete",
  "data": {
    "round_id": 12345,
    "winners": ["Wallet1", "Wallet2", ...]
  }
}
```

#### Error
```json
{
  "type": "Error",
  "data": {
    "message": "Error message"
  }
}
```

### WebSocket Curl Example
```bash
wscat -c ws://localhost:3000/ws
# Then send: {"type":"Subscribe","data":{"topic":"board"}}