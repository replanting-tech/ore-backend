# Martingale Progress WebSocket Updates

This document describes the new WebSocket functionality for real-time martingale strategy progress updates that are sent every time a new round turns.

## Overview

The ORE Mining Backend now includes WebSocket support for real-time martingale progress updates. When new rounds start, all active martingale strategies receive progress updates via WebSocket, allowing frontend applications to display live progress without polling.

## Features

### ✅ Real-time Updates
- **Round-based Updates**: Progress updates are automatically sent when new rounds begin
- **Periodic Updates**: Additional updates sent every 60 seconds for real-time monitoring
- **Multi-strategy Support**: Updates include all active martingale strategies

### ✅ Comprehensive Progress Data
- Strategy ID and wallet address
- Current round progress (current/total)
- Current deployment amount
- Total deployed, rewards, and losses
- Profit/Loss calculations
- Win rate percentage
- Risk level assessment
- Strategy status

### ✅ WebSocket Topics
- `martingale`: Subscribe to all martingale progress updates
- `all`: Subscribe to all updates including martingale

## Implementation Details

### New WebSocket Message Types

#### MartingaleProgressUpdate
```json
{
  "type": "MartingaleProgressUpdate",
  "data": {
    "progress": [
      {
        "strategy_id": "uuid-string",
        "wallet_address": "wallet-address",
        "current_round": 5,
        "total_rounds": 20,
        "progress_percentage": 25.0,
        "current_amount_sol": 0.016,
        "total_deployed_sol": 0.032,
        "total_rewards_sol": 0.036,
        "total_loss_sol": 0.0,
        "profit_loss_sol": 0.004,
        "status": "active",
        "win_rate_percentage": 80.0,
        "risk_level": "LOW"
      }
    ]
  }
}
```

### Subscription Topics

#### Subscribe to Martingale Updates
```json
{
  "type": "Subscribe",
  "data": {
    "topic": "martingale"
  }
}
```

#### Subscribe to All Updates
```json
{
  "type": "Subscribe",
  "data": {
    "topic": "all"
  }
}
```

## How It Works

### 1. Round Change Detection
The system monitors the blockchain for round changes. When a new round starts:

1. **Round Completion Processing**: The previous round's results are processed and stored
2. **Strategy Updates**: All active martingale strategies are updated with new round data
3. **Progress Calculation**: Real-time progress is calculated for each strategy
4. **WebSocket Broadcast**: Progress updates are broadcast to subscribed clients

### 2. Periodic Updates
Every 60 seconds, the system also sends:
- Current board state
- Active martingale strategies progress
- Treasury information
- Square statistics

### 3. Data Calculation

#### Progress Metrics
- **Progress Percentage**: `(current_round / total_rounds) * 100`
- **Profit/Loss**: `total_rewards_sol - total_deployed_sol`
- **Win Rate**: `(wins / total_rounds_played) * 100`
- **Risk Level**: Based on remaining loss budget vs current amount

#### Risk Assessment
- **HIGH**: Remaining loss budget < 50% of current amount
- **MEDIUM**: Remaining loss budget < 100% of current amount
- **LOW**: Remaining loss budget >= 100% of current amount

## Usage Examples

### JavaScript WebSocket Client

```javascript
const WebSocket = require('ws');

class MartingaleMonitor {
    constructor() {
        this.ws = new WebSocket('ws://localhost:3000/ws');
        this.setupEventHandlers();
    }

    setupEventHandlers() {
        this.ws.on('open', () => {
            console.log('Connected to ORE Mining Backend');
            
            // Subscribe to martingale updates
            this.ws.send(JSON.stringify({
                type: 'Subscribe',
                data: { topic: 'martingale' }
            }));
        });

        this.ws.on('message', (data) => {
            const message = JSON.parse(data);
            
            if (message.type === 'MartingaleProgressUpdate') {
                this.displayProgress(message.data.progress);
            }
        });
    }

    displayProgress(strategies) {
        strategies.forEach(strategy => {
            console.log(`Strategy ${strategy.strategy_id}:`);
            console.log(`  Progress: ${strategy.progress_percentage.toFixed(1)}%`);
            console.log(`  P&L: ${strategy.profit_loss_sol.toFixed(4)} SOL`);
            console.log(`  Risk: ${strategy.risk_level}`);
        });
    }
}

const monitor = new MartingaleMonitor();
```

### React Hook Example

```javascript
import { useState, useEffect, useRef } from 'react';

export const useMartingaleProgress = (wsUrl = 'ws://localhost:3000/ws') => {
    const [progress, setProgress] = useState([]);
    const [connected, setConnected] = useState(false);
    const wsRef = useRef(null);

    useEffect(() => {
        const ws = new WebSocket(wsUrl);
        wsRef.current = ws;

        ws.onopen = () => {
            setConnected(true);
            ws.send(JSON.stringify({
                type: 'Subscribe',
                data: { topic: 'martingale' }
            }));
        };

        ws.onmessage = (event) => {
            const message = JSON.parse(event.data);
            
            if (message.type === 'MartingaleProgressUpdate') {
                setProgress(message.data.progress);
            }
        };

        ws.onclose = () => {
            setConnected(false);
        };

        return () => {
            ws.close();
        };
    }, [wsUrl]);

    return { progress, connected };
};

// Usage in component
const MartingaleDashboard = () => {
    const { progress, connected } = useMartingaleProgress();

    return (
        <div>
            <h2>Martingale Progress {connected ? '🟢' : '🔴'}</h2>
            {progress.map(strategy => (
                <div key={strategy.strategy_id}>
                    <h3>Wallet: {strategy.wallet_address}</h3>
                    <p>Progress: {strategy.progress_percentage.toFixed(1)}%</p>
                    <p>P&L: {strategy.profit_loss_sol.toFixed(4)} SOL</p>
                    <p>Risk Level: {strategy.risk_level}</p>
                </div>
            ))}
        </div>
    );
};
```

## Testing

### Using the Test Client

A comprehensive test client is provided in `test_martingale_ws.js`:

```bash
# Install dependencies
npm install ws

# Run the test client
node test_martingale_ws.js
```

The test client will:
- Connect to the WebSocket endpoint
- Subscribe to martingale updates
- Display progress updates in real-time
- Handle reconnection automatically

### Expected Output

```
🚀 Starting Martingale WebSocket Test Client
📡 Connecting to: ws://localhost:3000/ws
📝 This client will subscribe to martingale progress updates
⏹️  Press Ctrl+C to stop the client

🔌 Connecting to WebSocket: ws://localhost:3000/ws
✅ WebSocket connected successfully!
📡 Subscribing to martingale progress updates...
🏓 Received pong from server

📈 === MARTINGALE PROGRESS UPDATE ===
🕒 Timestamp: 2:45:32 PM
📊 Active Strategies: 2

🎯 Strategy 1:
   🆔 ID: 123e4567-e89b-12d3-a456-426614174000
   💳 Wallet: 9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM
   📊 Progress: 5/20 rounds (25.0%)
   💰 Current Amount: 0.0160000 SOL
   📈 Total Deployed: 0.0320000 SOL
   🎁 Total Rewards: 0.0360000 SOL
   💸 Total Loss: 0.0000000 SOL
   ⚖️  P&L: +0.0040000 SOL
   🏆 Win Rate: 80.0%
   ⚠️  Risk Level: LOW
   📋 Status: active

=====================================
```

## Configuration

### Environment Variables
No additional environment variables are required. The WebSocket functionality is enabled by default.

### Server Configuration
The WebSocket endpoint is available at:
- **URL**: `ws://localhost:3000/ws`
- **Protocol**: WebSocket (ws://) or WebSocket Secure (wss://)

### Connection Limits
- Maximum concurrent WebSocket connections: 100
- Connection timeout: 300 seconds
- Message broadcast buffer: 1000 messages

## Error Handling

### Common Issues

1. **Connection Refused**
   - Ensure the server is running on the correct port
   - Check firewall settings
   - Verify WebSocket endpoint URL

2. **No Updates Received**
   - Confirm subscription to correct topics
   - Check if any active martingale strategies exist
   - Verify server logs for errors

3. **Frequent Disconnections**
   - Check network stability
   - Increase timeout values
   - Monitor server resource usage

### Debugging

Enable debug logging by setting the environment variable:
```bash
RUST_LOG=debug cargo run
```

## Performance Considerations

### Database Queries
- Progress updates query active strategies and their mining sessions
- Queries are optimized with proper indexing
- Result caching is not implemented for real-time accuracy

### WebSocket Performance
- Broadcast messages are filtered by subscription topics
- Large numbers of strategies may impact update frequency
- Consider implementing pagination for very large strategy lists

## Future Enhancements

### Potential Improvements
1. **Pagination**: Support for large numbers of strategies
2. **Filtering**: Client-side filtering by wallet, status, etc.
3. **Historical Data**: Include historical progress data
4. **Compression**: Message compression for large updates
5. **Authentication**: WebSocket authentication tokens

### API Extensions
1. **Subscribe by Wallet**: Subscribe to specific wallet strategies only
2. **Strategy-specific Updates**: Individual strategy progress updates
3. **Historical Progress**: Retrieve historical progress data
4. **Strategy Control**: Start/stop strategies via WebSocket

## Support

For issues or questions:
1. Check server logs for error messages
2. Verify database connectivity
3. Test with the provided WebSocket client
4. Review this documentation for configuration details

---

*This feature was implemented to provide real-time monitoring of martingale strategy progress, enabling better user experience and strategy management.*