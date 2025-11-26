import WebSocket from 'ws';

console.log('🧪 Testing ORE Mining WebSocket - Square Updates');
console.log('================================================\n');

// Connect to WebSocket
const ws = new WebSocket('ws://ore-backend.lombokit.com/ws');

ws.on('open', function open() {
    console.log('✅ Connected to WebSocket server');

    // Subscribe to squares topic
    const subscribeMsg = {
        type: 'Subscribe',
        data: { topic: 'squares' }
    };

    console.log('📡 Subscribing to squares topic...');
    ws.send(JSON.stringify(subscribeMsg, null, 2));
});

ws.on('message', function message(data) {
    try {
        const msg = JSON.parse(data.toString());
        console.log('\n📨 Received message:');
        console.log('Type:', msg.type);

        if (msg.type === 'SquaresUpdate') {
            console.log('🎯 Square Update Details:');
            console.log(`Total squares: ${msg.data.squares.length}`);

            // Show stats for first few squares
            msg.data.squares.slice(0, 5).forEach(square => {
                console.log(`Square ${square.square_id + 1}: 👥 ${square.participants} participants, 📊 ${square.competition_level.toFixed(4)} competition`);
            });

            if (msg.data.squares.length > 5) {
                console.log(`... and ${msg.data.squares.length - 5} more squares`);
            }
        } else if (msg.type === 'BoardUpdate') {
            console.log('📊 Board Update:', {
                round_id: msg.data.board.round_id,
                time_remaining: msg.data.board.time_remaining_sec.toFixed(1) + 's'
            });
        } else {
            console.log('Other message:', JSON.stringify(msg, null, 2));
        }
    } catch (e) {
        console.log('Raw message:', data.toString());
    }
});

ws.on('error', function error(err) {
    console.error('❌ WebSocket error:', err.message);
});

ws.on('close', function close(code, reason) {
    console.log('🔌 Connection closed:', code, reason.toString());
});

// Auto-close after 30 seconds for testing
setTimeout(() => {
    console.log('\n⏰ Test completed (30 seconds)');
    ws.close();
}, 30000);