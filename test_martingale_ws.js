#!/usr/bin/env node

/**
 * Test WebSocket Client for Martingale Progress Updates
 * 
 * This script demonstrates how to connect to the ORE Mining Backend WebSocket
 * and subscribe to martingale progress updates that are sent every new round.
 * 
 * Usage:
 *   node test_martingale_ws.js
 * 
 * The script will:
 * 1. Connect to the WebSocket endpoint
 * 2. Subscribe to martingale progress updates
 * 3. Listen for and display progress updates
 * 4. Handle connection errors and reconnections
 */

const WebSocket = require('ws');

// WebSocket endpoint
const WS_URL = 'ws://localhost:3000/ws';

class MartingaleWebSocketClient {
    constructor() {
        this.ws = null;
        this.reconnectInterval = 5000; // 5 seconds
        this.isConnecting = false;
        this.reconnectAttempts = 0;
        this.maxReconnectAttempts = 10;
    }

    connect() {
        if (this.isConnecting) {
            console.log('🔄 Connection already in progress...');
            return;
        }

        this.isConnecting = true;
        console.log('🔌 Connecting to WebSocket:', WS_URL);
        
        this.ws = new WebSocket(WS_URL);

        this.ws.on('open', () => {
            console.log('✅ WebSocket connected successfully!');
            this.isConnecting = false;
            this.reconnectAttempts = 0;
            
            // Subscribe to martingale progress updates
            this.subscribeToMartingale();
            
            // Send initial ping to keep connection alive
            this.sendPing();
        });

        this.ws.on('message', (data) => {
            try {
                const message = JSON.parse(data.toString());
                this.handleMessage(message);
            } catch (error) {
                console.error('❌ Failed to parse WebSocket message:', error);
                console.log('Raw data:', data.toString());
            }
        });

        this.ws.on('close', (code, reason) => {
            console.log('🔌 WebSocket connection closed:', code, reason.toString());
            this.isConnecting = false;
            this.handleReconnect();
        });

        this.ws.on('error', (error) => {
            console.error('❌ WebSocket error:', error.message);
            this.isConnecting = false;
            this.handleReconnect();
        });

        // Handle pong responses
        this.ws.on('pong', () => {
            console.log('🏓 Received pong from server');
        });
    }

    handleMessage(message) {
        console.log('📨 Received message:', JSON.stringify(message, null, 2));

        switch (message.type) {
            case 'Pong':
                console.log('🏓 Server responded to ping');
                break;

            case 'MartingaleProgressUpdate':
                this.displayMartingaleProgress(message.data.progress);
                break;

            case 'BoardUpdate':
                console.log('🎯 Board update - Round:', message.data.board.round_id);
                break;

            case 'TreasuryUpdate':
                console.log('💰 Treasury update - Balance:', message.data.treasury.balance_sol, 'SOL');
                break;

            case 'SquaresUpdate':
                console.log('📊 Squares update -', message.data.squares.length, 'squares');
                break;

            case 'RoundComplete':
                console.log('🎉 Round complete! Round ID:', message.data.round_id);
                break;

            case 'Error':
                console.error('❌ Server error:', message.data.message);
                break;

            default:
                console.log('ℹ️  Unknown message type:', message.type);
        }
    }

    displayMartingaleProgress(progressArray) {
        if (!Array.isArray(progressArray) || progressArray.length === 0) {
            console.log('📈 No active martingale strategies found');
            return;
        }

        console.log('\n📈 === MARTINGALE PROGRESS UPDATE ===');
        console.log('🕒 Timestamp:', new Date().toLocaleTimeString());
        console.log('📊 Active Strategies:', progressArray.length);
        console.log('');

        progressArray.forEach((strategy, index) => {
            console.log(`🎯 Strategy ${index + 1}:`);
            console.log(`   🆔 ID: ${strategy.strategy_id}`);
            console.log(`   💳 Wallet: ${strategy.wallet_address.substring(0, 8)}...${strategy.wallet_address.substring(36)}`);
            console.log(`   📊 Progress: ${strategy.current_round}/${strategy.total_rounds} rounds (${strategy.progress_percentage.toFixed(1)}%)`);
            console.log(`   💰 Current Amount: ${strategy.current_amount_sol.toFixed(4)} SOL`);
            console.log(`   📈 Total Deployed: ${strategy.total_deployed_sol.toFixed(4)} SOL`);
            console.log(`   🎁 Total Rewards: ${strategy.total_rewards_sol.toFixed(4)} SOL`);
            console.log(`   💸 Total Loss: ${strategy.total_loss_sol.toFixed(4)} SOL`);
            console.log(`   ⚖️  P&L: ${strategy.profit_loss_sol >= 0 ? '+' : ''}${strategy.profit_loss_sol.toFixed(4)} SOL`);
            console.log(`   🏆 Win Rate: ${strategy.win_rate_percentage.toFixed(1)}%`);
            console.log(`   ⚠️  Risk Level: ${strategy.risk_level}`);
            console.log(`   📋 Status: ${strategy.status}`);
            console.log('');
        });

        console.log('=====================================\n');
    }

    subscribeToMartingale() {
        const subscribeMessage = {
            type: 'Subscribe',
            data: {
                topic: 'martingale'
            }
        };
        
        console.log('📡 Subscribing to martingale progress updates...');
        this.sendMessage(subscribeMessage);
    }

    sendPing() {
        const pingMessage = {
            type: 'Ping',
            data: {}
        };
        
        this.sendMessage(pingMessage);
        
        // Send ping every 30 seconds to keep connection alive
        setTimeout(() => this.sendPing(), 30000);
    }

    sendMessage(message) {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify(message));
        } else {
            console.log('⚠️  Cannot send message - WebSocket not connected');
        }
    }

    handleReconnect() {
        if (this.reconnectAttempts < this.maxReconnectAttempts) {
            this.reconnectAttempts++;
            console.log(`🔄 Attempting to reconnect (${this.reconnectAttempts}/${this.maxReconnectAttempts}) in ${this.reconnectInterval/1000} seconds...`);
            
            setTimeout(() => {
                this.connect();
            }, this.reconnectInterval);
        } else {
            console.log('❌ Max reconnect attempts reached. Giving up.');
            console.log('💡 Please check if the server is running on', WS_URL);
        }
    }

    disconnect() {
        if (this.ws) {
            console.log('🔌 Disconnecting WebSocket...');
            this.ws.close();
            this.ws = null;
        }
    }
}

// Main execution
console.log('🚀 Starting Martingale WebSocket Test Client');
console.log('📡 Connecting to:', WS_URL);
console.log('📝 This client will subscribe to martingale progress updates');
console.log('⏹️  Press Ctrl+C to stop the client\n');

const client = new MartingaleWebSocketClient();
client.connect();

// Handle graceful shutdown
process.on('SIGINT', () => {
    console.log('\n🛑 Received SIGINT, shutting down gracefully...');
    client.disconnect();
    process.exit(0);
});

process.on('SIGTERM', () => {
    console.log('\n🛑 Received SIGTERM, shutting down gracefully...');
    client.disconnect();
    process.exit(0);
});

// Handle uncaught exceptions
process.on('uncaughtException', (error) => {
    console.error('💥 Uncaught Exception:', error);
    client.disconnect();
    process.exit(1);
});

process.on('unhandledRejection', (reason, promise) => {
    console.error('💥 Unhandled Rejection at:', promise, 'reason:', reason);
    client.disconnect();
    process.exit(1);
});