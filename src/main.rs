use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{HeaderName, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tower_http::cors::{CorsLayer, AllowOrigin, AllowHeaders, AllowMethods};
use axum::http::Method;
use tracing::{info, error, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use tokio::time::{sleep, Duration};

// ============================================================================
// WebSocket Message Types
// ============================================================================ 

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WsMessage {
    // Client -> Server
    Subscribe { topic: String },
    Unsubscribe { topic: String },
    Ping,

    // Server -> Client
    Pong,
    BoardUpdate { board: BoardInfo },
    MinerUpdate { wallet: String, stats: MinerStats },
    TreasuryUpdate { treasury: TreasuryInfo },
    SquaresUpdate { squares: Vec<SquareStats> },
    RoundComplete { round_id: u64, winners: Vec<String> },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsSubscription {
    pub topics: Vec<String>,
}

// ============================================================================
// Configuration & State
// ============================================================================

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub redis: redis::aio::ConnectionManager,
    pub rpc_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>,
    pub config: Arc<Config>,
    pub broadcast: broadcast::Sender<WsMessage>,
    pub connection_count: Arc<AtomicUsize>,  // Track WebSocket connections
    pub max_connections: usize,              // Maximum allowed connections
}

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub redis_url: String,
    pub rpc_url: String,
    pub jwt_secret: String,
    pub keypair_path: String,
}

impl Config {
    pub fn from_env() -> Result<Self, anyhow::Error> {
        dotenv::dotenv().ok();
        Ok(Self {
            database_url: std::env::var("DATABASE_URL")?,
            redis_url: std::env::var("REDIS_URL")?,
            rpc_url: std::env::var("RPC_URL")?,
            jwt_secret: std::env::var("JWT_SECRET")?,
            keypair_path: std::env::var("KEYPAIR_PATH")?,
        })
    }
}

// ============================================================================
// WebSocket Handler
// ============================================================================

#[axum::debug_handler]
pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    // Check connection limit
    let current_connections = state.connection_count.load(Ordering::Relaxed);
    if current_connections >= state.max_connections {
        warn!("Max WebSocket connections reached: {}/{}", current_connections, state.max_connections);
        return axum::response::Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .body(axum::body::Body::from("Server at maximum capacity"))
            .unwrap();
    }
    
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    // Increment connection counter
    state.connection_count.fetch_add(1, Ordering::Relaxed);
    let connection_id = state.connection_count.load(Ordering::Relaxed);
    info!("WebSocket client connected (ID: {}, Total: {})", connection_id, connection_id);
    
    let (mut sender, mut receiver) = socket.split();
    
    // Subscribed topics for this client
    let mut subscribed_topics: Vec<String> = vec![];
    
    // Subscribe to broadcast
    let mut rx = state.broadcast.subscribe();
    
    // Send initial connection message
    let welcome = WsMessage::BoardUpdate {
        board: blockchain::get_board_info(&state.rpc_client)
            .await
            .unwrap_or_else(|_| BoardInfo {
                round_id: 0,
                start_slot: 0,
                end_slot: 0,
                current_slot: 0,
                time_remaining_sec: 0.0,
            }),
    };
    
    if let Ok(msg) = serde_json::to_string(&welcome) {
        let _ = sender.send(Message::Text(msg)).await;
    }
    
    // Handle incoming and outgoing messages with timeout
    loop {
        tokio::select! {
            // Handle messages from client with timeout
            result = tokio::time::timeout(Duration::from_secs(300), receiver.next()) => {
                match result {
                    Ok(Some(Ok(msg))) => {
                        match msg {
                            Message::Text(text) => {
                                if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                                    match ws_msg {
                                        WsMessage::Subscribe { topic } => {
                                            info!("Client {} subscribed to: {}", connection_id, topic);
                                            if !subscribed_topics.contains(&topic) {
                                                subscribed_topics.push(topic.clone());
                                            }
                                            
                                            // Send initial data for the topic
                                            let response = get_initial_data(&state, &topic).await;
                                            if let Ok(msg) = serde_json::to_string(&response) {
                                                let _ = sender.send(Message::Text(msg)).await;
                                            }
                                        }
                                        WsMessage::Unsubscribe { topic } => {
                                            info!("Client {} unsubscribed from: {}", connection_id, topic);
                                            subscribed_topics.retain(|t| t != &topic);
                                        }
                                        WsMessage::Ping => {
                                            if let Ok(msg) = serde_json::to_string(&WsMessage::Pong) {
                                                let _ = sender.send(Message::Text(msg)).await;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Message::Close(_) => {
                                info!("WebSocket client {} disconnected", connection_id);
                                break;
                            }
                            _ => {}
                        }
                    }
                    Ok(Some(Err(e))) => {
                        let error_msg = e.to_string();
                        if error_msg.contains("Connection reset without closing handshake") {
                            info!("WebSocket client {} disconnected (connection reset)", connection_id);
                        } else {
                            warn!("WebSocket error for client {}: {}", connection_id, e);
                        }
                        break;
                    }
                    Ok(None) => {
                        info!("WebSocket client {} connection closed", connection_id);
                        break;
                    }
                    Err(_) => {
                        // Timeout - clean up connection
                        info!("WebSocket client {} timed out", connection_id);
                        break;
                    }
                }
            }
            
            // Handle broadcast messages with timeout
            result = tokio::time::timeout(Duration::from_secs(60), rx.recv()) => {
                match result {
                    Ok(Ok(broadcast_msg)) => {
                        // Filter based on subscribed topics
                        let should_send = match &broadcast_msg {
                            WsMessage::BoardUpdate { .. } => subscribed_topics.contains(&"board".to_string()),
                            WsMessage::TreasuryUpdate { .. } => subscribed_topics.contains(&"treasury".to_string()),
                            WsMessage::SquaresUpdate { .. } => subscribed_topics.contains(&"squares".to_string()),
                            WsMessage::RoundComplete { .. } => true, // Always send round complete
                            WsMessage::MinerUpdate { wallet, .. } => {
                                subscribed_topics.contains(&format!("miner:{}", wallet)) ||
                                subscribed_topics.contains(&"miners".to_string())
                            }
                            _ => false,
                        };
                        
                        if should_send {
                            if let Ok(msg) = serde_json::to_string(&broadcast_msg) {
                                if sender.send(Message::Text(msg)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Ok(Err(_)) => {
                        // Broadcast channel error - continue
                    }
                    Err(_) => {
                        // Timeout on broadcast - normal, continue
                    }
                }
            }
        }
    }
    
    // Decrement connection counter when done
    state.connection_count.fetch_sub(1, Ordering::Relaxed);
    info!("WebSocket handler closed (ID: {})", connection_id);
}

async fn get_initial_data(state: &Arc<AppState>, topic: &str) -> WsMessage {
    match topic {
        "board" => {
            match with_rpc_retry(|| async { blockchain::get_board_info(&state.rpc_client).await }).await {
                Ok(board) => WsMessage::BoardUpdate { board },
                Err(e) => {
                    error!("Failed to fetch board for initial data: {}", e);
                    WsMessage::Error { message: "Failed to fetch board".to_string() }
                }
            }
        }
        "treasury" => {
            match with_rpc_retry(|| async { blockchain::get_treasury_info(&state.rpc_client).await }).await {
                Ok(treasury) => WsMessage::TreasuryUpdate { treasury },
                Err(e) => {
                    error!("Failed to fetch treasury for initial data: {}", e);
                    WsMessage::Error { message: "Failed to fetch treasury".to_string() }
                }
            }
        }
        "squares" => {
            // Note: This would require modifying AppState to allow mutable access to redis
            // For now, we'll use the non-cached version for initial data
            match with_rpc_retry(|| async { blockchain::get_square_stats(&state.rpc_client).await }).await {
                Ok(squares) => WsMessage::SquaresUpdate { squares },
                Err(e) => {
                    error!("Failed to fetch square stats for initial data: {}", e);
                    WsMessage::Error { message: "Failed to fetch square stats".to_string() }
                }
            }
        }
        topic if topic.starts_with("miner:") => {
            let wallet = topic.strip_prefix("miner:").unwrap();
            if let Ok(pubkey) = wallet.parse() {
                match with_rpc_retry(|| async { blockchain::get_miner_stats(&state.rpc_client, pubkey).await }).await {
                    Ok(stats) => WsMessage::MinerUpdate {
                        wallet: wallet.to_string(),
                        stats
                    },
                    Err(e) => {
                        error!("Failed to fetch miner stats for {}: {}", wallet, e);
                        WsMessage::Error { message: "Failed to fetch miner stats".to_string() }
                    }
                }
            } else {
                WsMessage::Error { message: "Invalid wallet address".to_string() }
            }
        }
        _ => WsMessage::Error { message: "Unknown topic".to_string() }
    }
}

// ============================================================================
// Background Update Task
// ============================================================================

pub fn start_update_broadcaster(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut redis_client;
        let mut redis_retry_count = 0;
        const MAX_RETRY_COUNT: usize = 3;

        // Create Redis connection with retry logic
        loop {
            match redis::Client::open(state.config.redis_url.as_str()) {
                Ok(client) => {
                    match client.get_connection_manager().await {
                        Ok(conn) => {
                            redis_client = Some(conn);
                            let _ = redis_retry_count; // Suppress unused variable warning
                            break;
                        }
                        Err(e) => {
                            warn!("Failed to create Redis connection for broadcaster: {}", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to create Redis client for broadcaster: {}", e);
                }
            }

            redis_retry_count += 1;
            if redis_retry_count >= MAX_RETRY_COUNT {
                error!("Failed to initialize Redis connection after {} attempts", MAX_RETRY_COUNT);
                return;
            }

            sleep(Duration::from_secs(5)).await;
        }

        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(15));
        let mut counter = 0u64;
        let mut last_round_id: Option<u64> = None;

        loop {
            interval.tick().await;
            counter += 1;

            // Broadcast board updates (every 60 seconds, every 4 ticks)
            if counter % 4 == 0 {
                if let Ok(board) = blockchain::get_board_info(&state.rpc_client).await {
                    // Check if round changed
                    if let Some(last) = last_round_id {
                        if board.round_id != last {
                            // Round has ended, update sessions for the previous round
                            info!("Round {} ended, updating session results", last);
                            if let Err(e) = update_round_results(&state, last).await {
                                error!("Failed to update round results for round {}: {}", last, e);
                            }
                        }
                    }
                    last_round_id = Some(board.round_id);

                    let msg = WsMessage::BoardUpdate { board };
                    let _ = state.broadcast.send(msg);
                } else {
                    warn!("Failed to fetch board info for broadcast");
                }
            }

            // Broadcast square stats every 45 seconds (every 3 ticks) - with connection management
            if counter % 3 == 0 {
                if let Some(ref mut redis) = redis_client {
                    if let Ok(squares) = blockchain::get_square_stats_cached(&state.rpc_client, redis).await {
                        let msg = WsMessage::SquaresUpdate { squares };
                        let _ = state.broadcast.send(msg);
                    } else {
                        warn!("Failed to fetch square stats for broadcast");
                        // Try to reconnect Redis if connection is broken
                        redis_client = reconnect_redis(&state.config.redis_url).await;
                    }
                }
            }

            // Broadcast treasury updates (every 150 seconds, every 10 ticks)
            if counter % 10 == 0 {
                if let Ok(treasury) = blockchain::get_treasury_info(&state.rpc_client).await {
                    let msg = WsMessage::TreasuryUpdate { treasury };
                    let _ = state.broadcast.send(msg);
                } else {
                    warn!("Failed to fetch treasury info for broadcast");
                }
            }
        }
    });
}

async fn reconnect_redis(redis_url: &str) -> Option<redis::aio::ConnectionManager> {
    for attempt in 1..=3 {
        match redis::Client::open(redis_url) {
            Ok(client) => {
                match client.get_connection_manager().await {
                    Ok(conn) => {
                        info!("Redis reconnected successfully on attempt {}", attempt);
                        return Some(conn);
                    }
                    Err(e) => {
                        warn!("Redis reconnection attempt {} failed: {}", attempt, e);
                    }
                }
            }
            Err(e) => {
                warn!("Redis client creation failed on attempt {}: {}", attempt, e);
            }
        }
        sleep(Duration::from_secs(2)).await;
    }
    None
}

// ============================================================================
// Error Handling
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("Solana RPC error: {0}")]
    Rpc(#[from] solana_client::client_error::ClientError),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Not found")]
    NotFound,
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Bad request: {0}")]
    BadRequest(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            ApiError::Database(ref e) => {
                error!("Database error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
            ApiError::Redis(ref e) => {
                error!("Redis error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
            ApiError::Rpc(ref e) => {
                error!("RPC error: {}", e);
                (StatusCode::BAD_GATEWAY, e.to_string())
            }
            ApiError::Json(ref e) => {
                error!("JSON error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
            ApiError::NotFound => (StatusCode::NOT_FOUND, "Resource not found".to_string()),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized".to_string()),
            ApiError::BadRequest(ref msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Internal(ref msg) => {
                error!("Internal error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, msg.clone())
            }
        };

        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

// ============================================================================
// Models
// ============================================================================

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: uuid::Uuid,
    pub wallet_address: String,
    pub email: Option<String>,
    pub burner_address: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct MiningSession {
    pub id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub round_id: i64,
    pub deployed_amount: i64,
    pub squares: Vec<i32>,
    pub status: String,
    pub rewards_sol: i64,
    pub rewards_ore: i64,
    pub claimed: bool,
    pub profitability: Option<String>, // "win", "loss", "breakeven", or null if not calculated
    pub winning_square: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct MartingaleStrategy {
    pub id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub wallet_address: String,
    pub rounds: i32,
    pub base_amount_sol: f64,
    pub loss_multiplier: f64,
    pub max_loss_sol: f64,
    pub status: String, // "active", "completed", "stopped"
    pub current_round: i32,
    pub current_amount_sol: f64,
    pub total_deployed_sol: f64,
    pub total_rewards_sol: f64,
    pub total_loss_sol: f64,
    pub last_round_id: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerStats {
    pub address: String,
    pub authority: String,
    pub balance_sol: f64,
    pub rewards_sol: f64,
    pub rewards_ore: f64,
    pub refined_ore: f64,
    pub round_id: u64,
    pub checkpoint_id: u64,
    pub lifetime_rewards_sol: f64,
    pub lifetime_rewards_ore: f64,
    pub deployed: Vec<u64>,
    pub cumulative: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardInfo {
    pub round_id: u64,
    pub start_slot: u64,
    pub end_slot: u64,
    pub current_slot: u64,
    pub time_remaining_sec: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundInfo {
    pub address: String,
    pub id: u64,
    pub count: Vec<u64>,
    pub deployed: Vec<u64>,
    pub expires_at: u64,
    pub motherlode: u64,
    pub rent_payer: String,
    pub slot_hash: Vec<u8>,
    pub top_miner: String,
    pub top_miner_reward: u64,
    pub total_deployed: u64,
    pub total_vaulted: u64,
    pub total_winnings: u64,
    pub winning_square: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreasuryInfo {
    pub address: String,
    pub balance_sol: f64,
    pub motherlode_ore: f64,
    pub total_staked: f64,
    pub total_unclaimed: f64,
    pub total_refined: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SquareStats {
    pub square_id: usize,
    pub participants: u64,
    pub competition_level: f64,
    pub total_deployed: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeployRequest {
    pub wallet_address: String,
    pub amount: u64,
    pub square_ids: Option<Vec<u64>>,  // UBAH: terima array
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StartMartingaleRequest {
    pub wallet_address: String,
    pub rounds: i32,
    pub base_amount_sol: f64,
    pub loss_multiplier: f64,
    pub max_loss_sol: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateBurnerWalletRequest {
    pub wallet_address: String,
    pub burner_address: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimResponse {
    pub signature: String,
    pub sol_claimed: f64,
    pub ore_claimed: f64,
}

// ============================================================================
// RPC Error Handling Helpers
// ============================================================================

fn handle_transaction_rpc_error(e: &solana_client::client_error::ClientError) -> ApiError {
    let error_msg = e.to_string();

    if error_msg.contains("Insufficient funds for fee") {
        ApiError::BadRequest(
            "Insufficient SOL balance to pay for transaction fees. Please ensure your account has enough SOL to cover both the transaction amount and fees.".to_string()
        )
    } else if error_msg.contains("AccountNotFound") {
        ApiError::NotFound
    } else if error_msg.contains("BlockhashNotFound") {
        ApiError::Internal(
            "Transaction blockhash not found. This may be due to network delays. Please try again.".to_string()
        )
    } else if error_msg.contains("InvalidSignature") || error_msg.contains("signature verification failed") {
        ApiError::BadRequest(
            "Invalid transaction signature. Please check your keypair configuration.".to_string()
        )
    } else if error_msg.contains("Transaction simulation failed") {
        if error_msg.contains("custom program error") {
            ApiError::Internal(
                "Transaction failed due to program logic error. This may be a temporary issue with the ORE program.".to_string()
            )
        } else {
            ApiError::Internal(
                "Transaction simulation failed. Please check your account balance and try again.".to_string()
            )
        }
    } else if error_msg.contains("RPC response error") {
        ApiError::Internal(
            "Solana RPC network error. The blockchain network may be experiencing issues. Please try again later.".to_string()
        )
    } else {
        error!("Unhandled transaction RPC error: {}", error_msg);
        ApiError::Internal(
            "Transaction failed due to blockchain network error. Please try again.".to_string()
        )
    }
}

// ============================================================================
// RPC Retry Wrapper
// ============================================================================

async fn with_rpc_retry<T, F, Fut>(operation: F) -> Result<T, ApiError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, ApiError>>,
{
    let mut attempts = 0;
    let max_attempts = 3;

    loop {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(ApiError::Rpc(ref e)) => {
                let error_msg = e.to_string();

                // Check if it's a rate limiting error
                if error_msg.contains("429") || error_msg.contains("Too Many Requests") {
                    attempts += 1;
                    if attempts >= max_attempts {
                        error!("RPC rate limit exceeded after {} attempts: {}", attempts, error_msg);
                        return Err(ApiError::Internal(format!("RPC rate limit exceeded. The Solana network is experiencing high traffic. Please try again later.")));
                    }

                    // Exponential backoff (500ms, 1s, 2s)
                    let delay_millis = 500 * (2u64.pow(attempts - 1));
                    let delay = Duration::from_millis(delay_millis);
                    warn!("RPC rate limited, retrying in {:?} (attempt {}/{})", delay, attempts, max_attempts);
                    sleep(delay).await;
                    continue;
                }

                // Handle specific RPC errors with clear messages
                if error_msg.contains("Insufficient funds for fee") {
                    return Err(ApiError::BadRequest(
                        "Insufficient SOL balance to pay for transaction fees. Please ensure your account has enough SOL to cover transaction costs.".to_string()
                    ));
                }

                if error_msg.contains("AccountNotFound") {
                    return Err(ApiError::NotFound);
                }

                if error_msg.contains("BlockhashNotFound") {
                    return Err(ApiError::Internal(
                        "Transaction blockhash not found. This may be due to network delays. Please try again.".to_string()
                    ));
                }

                if error_msg.contains("InvalidSignature") || error_msg.contains("signature verification failed") {
                    return Err(ApiError::BadRequest(
                        "Invalid transaction signature. Please check your keypair configuration.".to_string()
                    ));
                }

                if error_msg.contains("Transaction simulation failed") {
                    // Extract more specific simulation error if possible
                    if error_msg.contains("custom program error") {
                        return Err(ApiError::Internal(
                            "Transaction failed due to program logic error. This may be a temporary issue with the ORE program.".to_string()
                        ));
                    }
                    return Err(ApiError::Internal(
                        "Transaction simulation failed. Please check your account balance and try again.".to_string()
                    ));
                }

                if error_msg.contains("RPC response error") {
                    return Err(ApiError::Internal(
                        "Solana RPC network error. The blockchain network may be experiencing issues. Please try again later.".to_string()
                    ));
                }

                // For other RPC errors, return with generic message
                error!("Unhandled RPC error: {}", error_msg);
                return Err(ApiError::Internal(
                    "Blockchain network error occurred. Please try again in a few moments.".to_string()
                ));
            }
            Err(other_error) => return Err(other_error),
        }
    }
}

// ============================================================================
// Blockchain Integration
// ============================================================================

pub mod blockchain {
    use super::*;
    use solana_client::nonblocking::rpc_client::RpcClient;
    use solana_sdk::{
        pubkey::Pubkey,
        signature::{read_keypair_file, Signer},
        transaction::Transaction,
        compute_budget::ComputeBudgetInstruction,
    };
    // use std::str::FromStr;

    pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
    const TOKEN_DECIMALS: u8 = 11;

    pub async fn get_miner_stats(
        rpc: &RpcClient,
        authority: Pubkey,
    ) -> Result<MinerStats, ApiError> {
        if std::env::var("SIMULATE_ORE").unwrap_or_default() == "true" {
            use std::time::{SystemTime, UNIX_EPOCH};

            // Get current simulated round
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let current_round_id = 12345 + (now / 60) as u64;

            // Simulate win/loss based on round_id (even = win, odd = loss)
            let is_win = current_round_id % 2 == 0;
            let rewards_sol = if is_win { 5.0 } else { 0.0 };
            let rewards_ore = if is_win { 100.0 } else { 0.0 };

            return Ok(MinerStats {
                address: "SimulatedMinerAddress".to_string(),
                authority: authority.to_string(),
                balance_sol: 25.0, // Simulated wallet balance
                rewards_sol,
                rewards_ore,
                refined_ore: if is_win { 50.0 } else { 0.0 },
                round_id: current_round_id,
                checkpoint_id: current_round_id.saturating_sub(1),
                lifetime_rewards_sol: if is_win { 25.0 } else { 0.0 },
                lifetime_rewards_ore: if is_win { 500.0 } else { 0.0 },
                deployed: vec![1000; 25],
                cumulative: vec![5000; 25],
            });
        }

        with_rpc_retry(|| async {
            // Get wallet balance first
            let balance = rpc.get_balance(&authority).await
                .map_err(|e| {
                    // If account doesn't exist, return balance as 0
                    if e.to_string().contains("AccountNotFound") {
                        ApiError::Internal("Wallet account not found on blockchain".to_string())
                    } else {
                        ApiError::Rpc(e)
                    }
                })?;

            let balance_sol = (balance as f64) / LAMPORTS_PER_SOL as f64;

            let miner_pda = ore_api::state::miner_pda(authority);
            let account = match rpc.get_account(&miner_pda.0).await {
                Ok(account) => account,
                Err(e) => {
                    // Handle account not found - auto-register by returning default stats
                    if e.to_string().contains("AccountNotFound") {
                        return Ok(MinerStats {
                            address: miner_pda.0.to_string(),
                            authority: authority.to_string(),
                            balance_sol,
                            rewards_sol: 0.0,
                            rewards_ore: 0.0,
                            refined_ore: 0.0,
                            round_id: 0,
                            checkpoint_id: 0,
                            lifetime_rewards_sol: 0.0,
                            lifetime_rewards_ore: 0.0,
                            deployed: vec![0; 25],
                            cumulative: vec![0; 25],
                        });
                    }
                    return Err(ApiError::Rpc(e));
                }
            };

            // Slice to the exact size of Miner struct, skipping 8-byte discriminator
            let miner_size = std::mem::size_of::<ore_api::state::Miner>();
            if account.data.len() < 8 + miner_size {
                return Err(ApiError::Internal("Miner account data too small".to_string()));
            }
            let miner_data = &account.data[8..8 + miner_size];

            // BENAR: Gunakan bytemuck
            let miner = bytemuck::try_from_bytes::<ore_api::state::Miner>(miner_data)
                .map_err(|e| ApiError::Internal(format!("Failed to parse miner: {:?}", e)))?;

            Ok(MinerStats {
                address: miner_pda.0.to_string(),
                authority: authority.to_string(),
                balance_sol,
                rewards_sol: (miner.rewards_sol as f64) / LAMPORTS_PER_SOL as f64,
                rewards_ore: amount_to_ui(miner.rewards_ore),
                refined_ore: amount_to_ui(miner.refined_ore),
                round_id: miner.round_id,
                checkpoint_id: miner.checkpoint_id,
                lifetime_rewards_sol: (miner.lifetime_rewards_sol as f64) / LAMPORTS_PER_SOL as f64,
                lifetime_rewards_ore: amount_to_ui(miner.lifetime_rewards_ore),
                deployed: miner.deployed.to_vec(),
                cumulative: miner.cumulative.to_vec(),
            })
        }).await
    }

    pub async fn get_board_info(rpc: &RpcClient) -> Result<BoardInfo, ApiError> {
        if std::env::var("SIMULATE_ORE").unwrap_or_default() == "true" {
            use std::time::{SystemTime, UNIX_EPOCH};

            // Simulate round progression based on current time
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            // Each round lasts 60 seconds, base round_id on time
            let round_id = 12345 + (now / 60) as u64;
            let round_start_time = (now / 60) * 60;
            let elapsed_in_round = now - round_start_time;

            // Calculate slots (0.4 sec per slot)
            let start_slot = 1000 + (round_id - 12345) * 150;
            let end_slot = start_slot + 150;
            let current_slot = start_slot + (elapsed_in_round as f64 * 2.5) as u64; // 2.5 slots per second
            let time_remaining_sec = 60.0 - elapsed_in_round as f64;

            return Ok(BoardInfo {
                round_id,
                start_slot,
                end_slot,
                current_slot: current_slot.min(end_slot),
                time_remaining_sec: time_remaining_sec.max(0.0),
            });
        }

        with_rpc_retry(|| async {
            let board_pda = ore_api::state::board_pda();
            let account = match rpc.get_account(&board_pda.0).await {
                Ok(account) => account,
                Err(e) => {
                    // Handle account not found
                    if e.to_string().contains("AccountNotFound") {
                        return Err(ApiError::NotFound);
                    }
                    return Err(ApiError::Rpc(e));
                }
            };

            // Slice to the exact size of Board struct, skipping 8-byte discriminator
            let board_size = std::mem::size_of::<ore_api::state::Board>();
            if account.data.len() < 8 + board_size {
                return Err(ApiError::Internal("Board account data too small".to_string()));
            }
            let board_data = &account.data[8..8 + board_size];

            // BENAR: Gunakan bytemuck untuk deserialize
            let board = bytemuck::try_from_bytes::<ore_api::state::Board>(board_data)
                .map_err(|e| ApiError::Internal(format!("Failed to parse board: {:?}", e)))?;

            // Get current clock
            let clock_account = match rpc.get_account(&solana_sdk::sysvar::clock::ID).await {
                Ok(account) => account,
                Err(e) => {
                    // Handle clock account not found
                    if e.to_string().contains("AccountNotFound") {
                        return Err(ApiError::NotFound);
                    }
                    return Err(ApiError::Rpc(e));
                }
            };
            let clock: solana_sdk::clock::Clock = bincode::deserialize(&clock_account.data)
                .map_err(|e| ApiError::Internal(format!("Clock deserialize error: {}", e)))?;

            // Calculate time remaining dengan benar
            let slots_remaining = board.end_slot.saturating_sub(clock.slot);
            let time_remaining = (slots_remaining as f64) * 0.4;

            Ok(BoardInfo {
                round_id: board.round_id,
                start_slot: board.start_slot,
                end_slot: board.end_slot,
                current_slot: clock.slot,
                time_remaining_sec: time_remaining.max(0.0),
            })
        }).await
    }

    pub async fn get_round_info(rpc: &RpcClient, round_id: u64) -> Result<RoundInfo, ApiError> {
        if std::env::var("SIMULATE_ORE").unwrap_or_default() == "true" {
            // Simulate round info
            return Ok(RoundInfo {
                address: "SimulatedRoundAddress".to_string(),
                id: round_id,
                count: vec![10; 25],
                deployed: vec![100000; 25],
                expires_at: 1000000,
                motherlode: 0,
                rent_payer: "SimulatedRentPayer".to_string(),
                slot_hash: vec![0; 32],
                top_miner: "SimulatedTopMiner".to_string(),
                top_miner_reward: 1000000000,
                total_deployed: 2500000,
                total_vaulted: 250000,
                total_winnings: 2250000,
                winning_square: 12,
            });
        }

        with_rpc_retry(|| async {
            let round_pda = ore_api::state::round_pda(round_id);
            let account = match rpc.get_account(&round_pda.0).await {
                Ok(account) => account,
                Err(e) => {
                    if e.to_string().contains("AccountNotFound") {
                        return Err(ApiError::NotFound);
                    }
                    return Err(ApiError::Rpc(e));
                }
            };

            let round_size = std::mem::size_of::<ore_api::state::Round>();
            if account.data.len() < 8 + round_size {
                return Err(ApiError::Internal("Round account data too small".to_string()));
            }
            let round_data = &account.data[8..8 + round_size];

            let round = bytemuck::try_from_bytes::<ore_api::state::Round>(round_data)
                .map_err(|e| ApiError::Internal(format!("Failed to parse round: {:?}", e)))?;

            // Calculate winning square from slot hash
            let winning_square = (round.slot_hash[0] as usize) % 25;

            Ok(RoundInfo {
                address: round_pda.0.to_string(),
                id: round.id,
                count: round.count.to_vec(),
                deployed: round.deployed.to_vec(),
                expires_at: round.expires_at,
                motherlode: round.motherlode,
                rent_payer: round.rent_payer.to_string(),
                slot_hash: round.slot_hash.to_vec(),
                top_miner: round.top_miner.to_string(),
                top_miner_reward: round.top_miner_reward,
                total_deployed: round.total_deployed,
                total_vaulted: round.total_vaulted,
                total_winnings: round.total_winnings,
                winning_square,
            })
        }).await
    }

    pub async fn get_treasury_info(rpc: &RpcClient) -> Result<TreasuryInfo, ApiError> {
        if std::env::var("SIMULATE_ORE").unwrap_or_default() == "true" {
            return Ok(TreasuryInfo {
                address: "SimulatedTreasuryAddress".to_string(),
                balance_sol: 10000.0,
                motherlode_ore: 50000.0,
                total_staked: 100000.0,
                total_unclaimed: 5000.0,
                total_refined: 20000.0,
            });
        }

        with_rpc_retry(|| async {
            let treasury_pda = ore_api::state::treasury_pda();
            let account = match rpc.get_account(&treasury_pda.0).await {
                Ok(account) => account,
                Err(e) => {
                    // Handle account not found
                    if e.to_string().contains("AccountNotFound") {
                        return Err(ApiError::NotFound);
                    }
                    return Err(ApiError::Rpc(e));
                }
            };

            // Slice to the exact size of Treasury struct, skipping 8-byte discriminator
            let treasury_size = std::mem::size_of::<ore_api::state::Treasury>();
            if account.data.len() < 8 + treasury_size {
                return Err(ApiError::Internal("Treasury account data too small".to_string()));
            }
            let treasury_data = &account.data[8..8 + treasury_size];

            // BENAR: Gunakan bytemuck
            let treasury = bytemuck::try_from_bytes::<ore_api::state::Treasury>(treasury_data)
                .map_err(|e| ApiError::Internal(format!("Failed to parse treasury: {:?}", e)))?;

            Ok(TreasuryInfo {
                address: treasury_pda.0.to_string(),
                balance_sol: (treasury.balance as f64) / LAMPORTS_PER_SOL as f64,
                motherlode_ore: amount_to_ui(treasury.motherlode),
                total_staked: amount_to_ui(treasury.total_staked),
                total_unclaimed: amount_to_ui(treasury.total_unclaimed),
                total_refined: amount_to_ui(treasury.total_refined),
            })
        }).await
    }

    pub async fn get_square_stats(rpc: &RpcClient) -> Result<Vec<SquareStats>, ApiError> {
        if std::env::var("SIMULATE_ORE").unwrap_or_default() == "true" {
            let mut square_stats = Vec::new();
            for i in 0..25 {
                square_stats.push(SquareStats {
                    square_id: i,
                    participants: 10 + (i as u64 % 15),
                    competition_level: 0.5 + (i as f64 * 0.02),
                    total_deployed: 1000000 + (i as u64 * 50000),
                });
            }
            return Ok(square_stats);
        }

        let board_pda = ore_api::state::board_pda();
        let account = match rpc.get_account(&board_pda.0).await {
            Ok(account) => account,
            Err(e) => {
                if e.to_string().contains("AccountNotFound") {
                    return Err(ApiError::NotFound);
                }
                return Err(ApiError::Rpc(e));
            }
        };

        let board_size = std::mem::size_of::<ore_api::state::Board>();
        if account.data.len() < 8 + board_size {
            return Err(ApiError::Internal("Board account data too small".to_string()));
        }
        let board_data = &account.data[8..8 + board_size];
        let board = bytemuck::try_from_bytes::<ore_api::state::Board>(board_data)
            .map_err(|e| ApiError::Internal(format!("Failed to parse board: {:?}", e)))?;

        // Parse square data dari board
        let mut square_stats = Vec::new();
        for i in 0..25 {
            let participants = count_participants_in_square(rpc, board.round_id, i).await?;
            let total_deployed = get_square_deployed(rpc, board.round_id, i).await?;

            // Hitung competition level (total deployed / max possible)
            let competition = if total_deployed > 0 {
                (total_deployed as f64) / 1_000_000_000.0 // Normalize
            } else {
                0.0
            };

            square_stats.push(SquareStats {
                square_id: i,
                participants,
                competition_level: competition,
                total_deployed,
            });
        }

        Ok(square_stats)
    }

    pub async fn get_square_stats_cached(
        rpc: &RpcClient,
        redis: &mut redis::aio::ConnectionManager,
    ) -> Result<Vec<SquareStats>, ApiError> {
        // Try cache first
        let cache_key = "squares:current";
        if let Ok(cached) = redis::cmd("GET")
            .arg(cache_key)
            .query_async::<_, Option<String>>(redis)
            .await
        {
            if let Some(data) = cached {
                if let Ok(stats) = serde_json::from_str::<Vec<SquareStats>>(&data) {
                    return Ok(stats);
                }
            }
        }

        // Fetch fresh data
        let stats = get_square_stats(rpc).await?;

        // Cache for 30 seconds
        let serialized = serde_json::to_string(&stats)?;
        let _: () = redis::cmd("SETEX")
            .arg(cache_key)
            .arg(30) // TTL 30 detik - more aggressive caching
            .arg(serialized)
            .query_async(redis)
            .await
            .map_err(|e| ApiError::Redis(e))?;

        Ok(stats)
    }

    pub async fn deploy_ore(
        rpc: &RpcClient,
        keypair_path: &str,
        amount: u64,
        square_id: Option<u64>,
    ) -> Result<String, ApiError> {
        if std::env::var("SIMULATE_ORE").unwrap_or_default() == "true" {
            return Ok("SimulatedDeploySignature1234567890abcdef".to_string());
        }

        let payer = read_keypair_file(keypair_path)
            .map_err(|e| ApiError::BadRequest(format!("Keypair error: {}", e)))?;

        // Get current board
        let board_pda = ore_api::state::board_pda();
        let account = rpc.get_account(&board_pda.0).await?;
        let board_size = std::mem::size_of::<ore_api::state::Board>();
        if account.data.len() < 8 + board_size {
            return Err(ApiError::Internal("Board account data too small".to_string()));
        }
        let board_data = &account.data[8..8 + board_size];
        let board = bytemuck::try_from_bytes::<ore_api::state::Board>(board_data)
            .map_err(|e| ApiError::Internal(format!("Failed to parse board: {:?}", e)))?;

        // Setup squares
        let squares = if let Some(id) = square_id {
            let mut s = [false; 25];
            s[id as usize] = true;
            s
        } else {
            [true; 25]
        };

        // Create deploy instruction
        let ix = ore_api::sdk::deploy(
            payer.pubkey(),
            payer.pubkey(),
            amount,
            board.round_id,
            squares,
        );

        // Add compute budget
        let compute_limit = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
        let compute_price = ComputeBudgetInstruction::set_compute_unit_price(1_000_000);

        let blockhash = rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[compute_limit, compute_price, ix],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );

        let signature = rpc.send_and_confirm_transaction(&tx).await?;
        info!("Deploy transaction: {}", signature);
    
        Ok(signature.to_string())
    }
    
        pub async fn deploy_ore_multiple(
            rpc: &RpcClient,
            keypair_path: &str,
            amount: u64,
            square_ids: Option<Vec<u64>>,
        ) -> Result<String, ApiError> {
            let payer = read_keypair_file(keypair_path)
                .map_err(|e| ApiError::BadRequest(format!("Keypair error: {}", e)))?;
    
            let board_pda = ore_api::state::board_pda();
            let account = rpc.get_account(&board_pda.0).await?;
            let board_size = std::mem::size_of::<ore_api::state::Board>();
            if account.data.len() < 8 + board_size {
                return Err(ApiError::Internal("Board account data too small".to_string()));
            }
            let board_data = &account.data[8..8 + board_size];
            let board = bytemuck::try_from_bytes::<ore_api::state::Board>(board_data)
                .map_err(|e| ApiError::Internal(format!("Failed to parse board: {:?}", e)))?;
    
            // Setup squares berdasarkan square_ids
            let squares = if let Some(ids) = square_ids {
                let mut s = [false; 25];
                for id in ids {
                    if id < 25 {
                        s[id as usize] = true;
                    }
                }
                s
            } else {
                [true; 25]  // Default: semua squares
            };
    
            // TETAP 1 instruksi saja
            let ix = ore_api::sdk::deploy(
                payer.pubkey(),
                payer.pubkey(),
                amount,
                board.round_id,
                squares,
            );
    
            let compute_limit = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
            let compute_price = ComputeBudgetInstruction::set_compute_unit_price(1_000_000);
    
            let blockhash = rpc.get_latest_blockhash().await?;
            let tx = Transaction::new_signed_with_payer(
                &[compute_limit, compute_price, ix],
                Some(&payer.pubkey()),
                &[&payer],
                blockhash,
            );
    
            let signature = rpc.send_and_confirm_transaction(&tx).await?;
            Ok(signature.to_string())
        }
    
        pub async fn claim_rewards(
        rpc: &RpcClient,
        keypair_path: &str,
    ) -> Result<ClaimResponse, ApiError> {
        if std::env::var("SIMULATE_ORE").unwrap_or_default() == "true" {
            return Ok(ClaimResponse {
                signature: "SimulatedClaimSignature1234567890abcdef".to_string(),
                sol_claimed: 2.5,
                ore_claimed: 50.0,
            });
        }

        let payer = read_keypair_file(keypair_path)
            .map_err(|e| ApiError::BadRequest(format!("Keypair error: {}", e)))?;

        // Get current rewards
        let stats = get_miner_stats(rpc, payer.pubkey()).await?;
        let sol_claimed = stats.rewards_sol;
        let ore_claimed = stats.rewards_ore;

        // Create claim instructions
        let ix_sol = ore_api::sdk::claim_sol(payer.pubkey());
        let ix_ore = ore_api::sdk::claim_ore(payer.pubkey());

        // Add compute budget
        let compute_limit = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
        let compute_price = ComputeBudgetInstruction::set_compute_unit_price(1_000_000);

        let blockhash = rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[compute_limit, compute_price, ix_sol, ix_ore],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );

        let signature = rpc.send_and_confirm_transaction(&tx).await?;
        info!("Claim transaction: {}", signature);

        Ok(ClaimResponse {
            signature: signature.to_string(),
            sol_claimed,
            ore_claimed,
        })
    }

    pub async fn register_miner(
        rpc: &RpcClient,
        keypair_path: &str,
    ) -> Result<String, ApiError> {
        if std::env::var("SIMULATE_ORE").unwrap_or_default() == "true" {
            return Ok("SimulatedRegisterSignature1234567890abcdef".to_string());
        }

        let payer = read_keypair_file(keypair_path)
            .map_err(|e| ApiError::BadRequest(format!("Keypair error: {}", e)))?;

        // Create register instruction
        let ix = ore_api::instruction::register(payer.pubkey());

        // Add compute budget
        let compute_limit = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
        let compute_price = ComputeBudgetInstruction::set_compute_unit_price(1_000_000);

        let blockhash = rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[compute_limit, compute_price, ix],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );

        let signature = rpc.send_and_confirm_transaction(&tx).await?;
        info!("Register miner transaction: {}", signature);

        Ok(signature.to_string())
    }

    pub async fn checkpoint_miner(
        rpc: &RpcClient,
        keypair_path: &str,
        authority: Option<Pubkey>,
    ) -> Result<String, ApiError> {
        if std::env::var("SIMULATE_ORE").unwrap_or_default() == "true" {
            return Ok("SimulatedCheckpointSignature1234567890abcdef".to_string());
        }

        let payer = read_keypair_file(keypair_path)
            .map_err(|e| ApiError::BadRequest(format!("Keypair error: {}", e)))?;

        let authority = authority.unwrap_or(payer.pubkey());

        // Get miner info
        let miner_pda = ore_api::state::miner_pda(authority);
        let account = rpc.get_account(&miner_pda.0).await?;

        let miner_size = std::mem::size_of::<ore_api::state::Miner>();
        if account.data.len() < 8 + miner_size {
            return Err(ApiError::Internal("Miner account data too small".to_string()));
        }
        let miner_data = &account.data[8..8 + miner_size];
        let miner = bytemuck::try_from_bytes::<ore_api::state::Miner>(miner_data)
            .map_err(|e| ApiError::Internal(format!("Failed to parse miner: {:?}", e)))?;

        // Create checkpoint instruction
        let ix = ore_api::sdk::checkpoint(payer.pubkey(), authority, miner.round_id);

        // Add compute budget
        let compute_limit = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
        let compute_price = ComputeBudgetInstruction::set_compute_unit_price(1_000_000);

        let blockhash = rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &[compute_limit, compute_price, ix],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );

        let signature = rpc.send_and_confirm_transaction(&tx).await?;
        info!("Checkpoint transaction: {}", signature);

        Ok(signature.to_string())
    }

    async fn count_participants_in_square(
        _rpc: &RpcClient,
        _round_id: u64,
        square_id: usize
    ) -> Result<u64, ApiError> {
        // TEMPORARY: Return dummy data for testing WebSocket functionality
        // TODO: Implement proper participant counting using getProgramAccounts
        // The current implementation may be too slow or have filter issues on mainnet

        // Return varying numbers based on square_id for testing
        // This simulates different levels of participation per square
        let base_participants = match square_id % 5 {
            0 => 5,  // Popular squares
            1 => 3,
            2 => 8,
            3 => 2,
            4 => 6,
            _ => 1,
        };

        // Add some randomness for realism
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let variation = rng.gen_range(0..4);
        Ok(base_participants + variation)
    }

    async fn get_square_deployed(
        _rpc: &RpcClient,
        _round_id: u64,
        square_id: usize
    ) -> Result<u64, ApiError> {
        // TEMPORARY: Return dummy data for testing WebSocket functionality
        // TODO: Implement proper deployed amount calculation

        // Return amounts that correlate with participant counts
        // Higher participants = higher deployed amounts
        let base_amount = match square_id % 5 {
            0 => 500_000,  // Popular squares have more deployment
            1 => 200_000,
            2 => 800_000,
            3 => 100_000,
            4 => 400_000,
            _ => 50_000,
        };

        // Add some variation
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let variation = rng.gen_range(0..100_000);
        Ok(base_amount + variation)
    }

    fn amount_to_ui(amount: u64) -> f64 {
        (amount as f64) / 10u64.pow(TOKEN_DECIMALS as u32) as f64
    }
}

// ============================================================================
// Database Setup
// ============================================================================

pub async fn setup_database(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            wallet_address VARCHAR(44) UNIQUE NOT NULL,
            email VARCHAR(255),
            burner_wallet VARCHAR(44),
            created_at TIMESTAMPTZ DEFAULT NOW(),
            updated_at TIMESTAMPTZ DEFAULT NOW()
        );

        CREATE TABLE IF NOT EXISTS mining_sessions (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            user_id UUID NOT NULL REFERENCES users(id),
            round_id BIGINT NOT NULL,
            deployed_amount BIGINT NOT NULL,
            squares INTEGER[] NOT NULL,
            status VARCHAR(50) NOT NULL DEFAULT 'active',
            rewards_sol BIGINT DEFAULT 0,
            rewards_ore BIGINT DEFAULT 0,
            claimed BOOLEAN DEFAULT FALSE,
            profitability VARCHAR(20), -- "win", "loss", "breakeven", or null
            winning_square INTEGER,
            created_at TIMESTAMPTZ DEFAULT NOW(),
            updated_at TIMESTAMPTZ DEFAULT NOW()
        );

        CREATE TABLE IF NOT EXISTS martingale_strategies (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            user_id UUID NOT NULL REFERENCES users(id),
            wallet_address VARCHAR(44) NOT NULL,
            rounds INTEGER NOT NULL,
            base_amount_sol DOUBLE PRECISION NOT NULL,
            loss_multiplier DOUBLE PRECISION NOT NULL,
            max_loss_sol DOUBLE PRECISION NOT NULL,
            status VARCHAR(20) NOT NULL DEFAULT 'active',
            current_round INTEGER DEFAULT 0,
            current_amount_sol DOUBLE PRECISION NOT NULL,
            total_deployed_sol DOUBLE PRECISION DEFAULT 0,
            total_rewards_sol DOUBLE PRECISION DEFAULT 0,
            total_loss_sol DOUBLE PRECISION DEFAULT 0,
            last_round_id BIGINT,
            created_at TIMESTAMPTZ DEFAULT NOW(),
            updated_at TIMESTAMPTZ DEFAULT NOW()
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_user ON mining_sessions(user_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_round ON mining_sessions(round_id);
        CREATE INDEX IF NOT EXISTS idx_martingale_user ON martingale_strategies(user_id);
        CREATE INDEX IF NOT EXISTS idx_martingale_wallet_address ON martingale_strategies(wallet_address);
        CREATE INDEX IF NOT EXISTS idx_martingale_status ON martingale_strategies(status);
        "#,
    )
    .execute(pool)
    .await?;

    info!("Database tables created/verified");
    Ok(())
}

// ============================================================================
// API Handlers
// ============================================================================

async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

async fn register_user(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let wallet_address = payload.get("wallet_address")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("wallet_address is required".into()))?;

    // Validate wallet address format (basic check)
    if wallet_address.len() != 44 {
        return Err(ApiError::BadRequest("Invalid wallet address format".into()));
    }

    let user = get_or_create_user(&state.db, wallet_address).await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "user_id": user.id,
        "wallet_address": user.wallet_address,
        "created_at": user.created_at
    })))
}

async fn get_board(State(state): State<Arc<AppState>>) -> Result<Json<BoardInfo>, ApiError> {
    let board = blockchain::get_board_info(&state.rpc_client).await?;
    Ok(Json(board))
}

async fn get_round(
    State(state): State<Arc<AppState>>,
    Path(round_id): Path<u64>,
) -> Result<Json<RoundInfo>, ApiError> {
    let round = blockchain::get_round_info(&state.rpc_client, round_id).await?;
    Ok(Json(round))
}

async fn get_treasury(State(state): State<Arc<AppState>>) -> Result<Json<TreasuryInfo>, ApiError> {
    let treasury = blockchain::get_treasury_info(&state.rpc_client).await?;
    Ok(Json(treasury))
}

async fn get_miner(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> Result<Json<MinerStats>, ApiError> {
    // Validate wallet address length
    if wallet.len() != 44 {
        return Err(ApiError::BadRequest("Invalid wallet address: must be exactly 44 characters".into()));
    }

    // Auto-register user in database
    let _user = get_or_create_user(&state.db, &wallet).await?;

    // Parse wallet address
    let pubkey: solana_sdk::pubkey::Pubkey = wallet.parse()
        .map_err(|_| ApiError::BadRequest("Invalid wallet address".into()))?;

    let stats = blockchain::get_miner_stats(&state.rpc_client, pubkey).await?;
    Ok(Json(stats))
}

async fn get_user_sessions(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> Result<Json<Vec<MiningSession>>, ApiError> {
    let mut sessions = sqlx::query_as::<_, MiningSession>(
        r#"
        SELECT ms.* FROM mining_sessions ms
        JOIN users u ON ms.user_id = u.id
        WHERE u.wallet_address = $1
        ORDER BY ms.created_at DESC
        LIMIT 50
        "#,
    )
    .bind(&wallet)
    .fetch_all(&state.db)
    .await?;

    // Calculate profitability for sessions that don't have it set
    for session in &mut sessions {
        if session.profitability.is_none() {
            // Get round info if winning_square not set
            if session.winning_square.is_none() {
                if let Ok(round_info) = blockchain::get_round_info(&state.rpc_client, session.round_id as u64).await {
                    session.winning_square = Some(round_info.winning_square as i32);
                    // Update winning_square in db
                    sqlx::query(
                        "UPDATE mining_sessions SET winning_square = $1, updated_at = NOW() WHERE id = $2"
                    )
                    .bind(session.winning_square)
                    .bind(session.id)
                    .execute(&state.db)
                    .await?;
                }
            }

            // Calculate profitability based on winning square
            if let Some(winning_square) = session.winning_square {
                let user_squares: Vec<i32> = session.squares.iter().map(|&s| s as i32).collect();
                let won = user_squares.contains(&winning_square);
                session.profitability = Some(if won { "win".to_string() } else { "loss".to_string() });

                // Update in database
                sqlx::query(
                    "UPDATE mining_sessions SET profitability = $1, updated_at = NOW() WHERE id = $2"
                )
                .bind(&session.profitability)
                .bind(session.id)
                .execute(&state.db)
                .await?;
            }
        }
    }

    Ok(Json(sessions))
}

// Helper function to get or create user by wallet address
async fn get_or_create_user(db: &PgPool, wallet_address: &str) -> Result<User, ApiError> {
    // Try to find existing user
    if let Ok(user) = sqlx::query_as::<_, User>(
        "SELECT * FROM users WHERE wallet_address = $1"
    )
    .bind(wallet_address)
    .fetch_one(db)
    .await {
        return Ok(user);
    }

    // Create new user if not found
    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (wallet_address) VALUES ($1) RETURNING *"
    )
    .bind(wallet_address)
    .fetch_one(db)
    .await?;

    info!("Created new user: {}", wallet_address);
    Ok(user)
}

// Calculate profitability for a mining session
fn calculate_profitability(deployed_amount_lamports: i64, rewards_sol: i64) -> String {
    let deployed_sol = deployed_amount_lamports as f64 / blockchain::LAMPORTS_PER_SOL as f64;
    let rewards_sol_f64 = rewards_sol as f64 / blockchain::LAMPORTS_PER_SOL as f64;

    if (rewards_sol_f64 - deployed_sol).abs() < 0.000001 { // Account for floating point precision
        "breakeven".to_string()
    } else if rewards_sol_f64 > deployed_sol {
        "win".to_string()
    } else {
        "loss".to_string()
    }
}


async fn deploy(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DeployRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate wallet address length
    if payload.wallet_address.len() != 44 {
        return Err(ApiError::BadRequest("Invalid wallet address: must be exactly 44 characters".into()));
    }

    info!("Deploy request received: wallet={}, amount={} lamports, square_ids={:?}",
          payload.wallet_address, payload.amount, payload.square_ids);

    // Get or create user
    info!("Getting or creating user for wallet: {}", payload.wallet_address);
    let user = get_or_create_user(&state.db, &payload.wallet_address).await?;
    info!("User found/created: id={}, wallet={}", user.id, user.wallet_address);

    // Get current board info for round_id
    info!("Fetching current board info");
    let board = blockchain::get_board_info(&state.rpc_client).await?;
    info!("Board info: round_id={}, current_slot={}", board.round_id, board.current_slot);

    // Check if miner account exists before checkpointing
    let miner_pda = ore_api::state::miner_pda(state.config.keypair_path.parse::<solana_sdk::pubkey::Pubkey>().unwrap_or_default());
    let miner_exists = match state.rpc_client.get_account(&miner_pda.0).await {
        Ok(_) => true,
        Err(e) => {
            if e.to_string().contains("AccountNotFound") {
                info!("Miner account not found, skipping checkpoint");
                false
            } else {
                error!("Error checking miner account: {}", e);
                return Err(ApiError::Rpc(e));
            }
        }
    };

    if miner_exists {
        info!("Checkpointing miner before deploy");
        match blockchain::checkpoint_miner(&state.rpc_client, &state.config.keypair_path, None).await {
            Ok(sig) => {
                info!("Checkpoint completed: signature={}", sig);
            }
            Err(ApiError::Rpc(e)) => {
                error!("Checkpoint before deploy failed: {}", e);
                return Err(handle_transaction_rpc_error(&e));
            }
            Err(other) => return Err(other),
        }
    } else {
        info!("Skipping checkpoint - miner account doesn't exist yet");
    }

    // Perform blockchain deployment
    info!("Performing blockchain deploy: amount={} lamports, square_ids={:?}", payload.amount, payload.square_ids);
    let signature = match blockchain::deploy_ore_multiple(
        &state.rpc_client,
        &state.config.keypair_path,
        payload.amount,
        payload.square_ids.clone(),
    )
    .await {
        Ok(sig) => sig,
        Err(ApiError::Rpc(e)) => {
            error!("Deploy transaction failed: {}", e);
            return Err(handle_transaction_rpc_error(&e));
        }
        Err(other) => return Err(other),
    };
    info!("Deploy transaction successful: signature={}", signature);

    // Create mining session record
    let squares: Vec<i32> = if let Some(ids) = &payload.square_ids {
        ids.iter().map(|&id| id as i32).collect()
    } else {
        (0..25).collect() // All squares if no specific squares
    };
    info!("Creating mining session record: user_id={}, round_id={}, deployed_amount={}, squares={:?}",
          user.id, board.round_id, payload.amount, squares);

    sqlx::query(
        r#"
        INSERT INTO mining_sessions (
            user_id, round_id, deployed_amount, squares, status, winning_square
        ) VALUES ($1, $2, $3, $4, 'active', $5)
        "#
    )
    .bind(user.id)
    .bind(board.round_id as i64)
    .bind(payload.amount as i64)
    .bind(&squares)
    .bind(None::<i32>)
    .execute(&state.db)
    .await?;
    info!("Mining session created successfully");

    let response = serde_json::json!({
        "success": true,
        "signature": signature,
        "amount": payload.amount,
        "squares": payload.square_ids,
        "user_id": user.id
    });
    info!("Deploy request completed successfully: {:?}", response);

    Ok(Json(response))
}

async fn claim(State(state): State<Arc<AppState>>) -> Result<Json<ClaimResponse>, ApiError> {
    let response = match blockchain::claim_rewards(&state.rpc_client, &state.config.keypair_path).await {
        Ok(res) => res,
        Err(ApiError::Rpc(e)) => {
            error!("Claim transaction failed: {}", e);
            return Err(handle_transaction_rpc_error(&e));
        }
        Err(other) => return Err(other),
    };
    Ok(Json(response))
}

async fn checkpoint(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let signature = match blockchain::checkpoint_miner(&state.rpc_client, &state.config.keypair_path, None).await {
        Ok(sig) => sig,
        Err(ApiError::Rpc(e)) => {
            error!("Checkpoint transaction failed: {}", e);
            return Err(handle_transaction_rpc_error(&e));
        }
        Err(other) => return Err(other),
    };
    Ok(Json(serde_json::json!({
        "success": true,
        "signature": signature
    })))
}

async fn get_active_martingale(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if wallet.len() != 44 {
        return Err(ApiError::BadRequest("Invalid wallet address format".into()));
    }

    let strategy: Option<MartingaleStrategy> = sqlx::query_as::<_, MartingaleStrategy>(
        r#"
        SELECT * FROM martingale_strategies
        WHERE wallet_address = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#
    )
    .bind(&wallet)
    .fetch_optional(&state.db)
    .await?;

    match strategy {
        Some(s) => Ok(Json(serde_json::json!({
            "success": true,
            "strategy": {
                "id": s.id,
                "rounds": s.rounds,
                "base_amount_sol": s.base_amount_sol,
                "loss_multiplier": s.loss_multiplier,
                "max_loss_sol": s.max_loss_sol,
                "status": s.status,
                "current_round": s.current_round,
                "current_amount_sol": s.current_amount_sol,
                "total_deployed_sol": s.total_deployed_sol,
                "total_rewards_sol": s.total_rewards_sol,
                "total_loss_sol": s.total_loss_sol,
                "last_round_id": s.last_round_id,
                "created_at": s.created_at,
                "updated_at": s.updated_at
            }
        }))),
        None => Ok(Json(serde_json::json!({
            "success": true,
            "strategy": null,
            "message": "No active martingale strategy found"
        })))
    }
}

async fn get_active_mining_sessions(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if wallet.len() != 44 {
        return Err(ApiError::BadRequest("Invalid wallet address format".into()));
    }

    let sessions: Vec<MiningSession> = sqlx::query_as::<_, MiningSession>(
        r#"
        SELECT ms.* FROM mining_sessions ms
        JOIN users u ON ms.user_id = u.id
        WHERE u.wallet_address = $1 AND ms.status = 'active'
        ORDER BY ms.created_at DESC
        LIMIT 50
        "#
    )
    .bind(&wallet)
    .fetch_all(&state.db)
    .await?;

    let sessions_with_calculations = sessions.into_iter().map(|s| {
        let deployed_sol = s.deployed_amount as f64 / blockchain::LAMPORTS_PER_SOL as f64;
        let rewards_sol = s.rewards_sol as f64 / blockchain::LAMPORTS_PER_SOL as f64;
        
        serde_json::json!({
            "id": s.id,
            "round_id": s.round_id,
            "deployed_amount_sol": deployed_sol,
            "rewards_sol": rewards_sol,
            "rewards_ore": s.rewards_ore,
            "squares": s.squares,
            "status": s.status,
            "claimed": s.claimed,
            "profitability": s.profitability,
            "created_at": s.created_at,
            "updated_at": s.updated_at,
            "profit_loss_sol": rewards_sol - deployed_sol
        })
    }).collect::<Vec<_>>();

    Ok(Json(serde_json::json!({
        "success": true,
        "active_sessions": sessions_with_calculations,
        "total_active": sessions_with_calculations.len()
    })))
}

async fn get_martingale_progress(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if wallet.len() != 44 {
        return Err(ApiError::BadRequest("Invalid wallet address format".into()));
    }

    // Get active strategy
    let strategy: Option<MartingaleStrategy> = sqlx::query_as::<_, MartingaleStrategy>(
        r#"
        SELECT * FROM martingale_strategies
        WHERE wallet_address = $1 AND status = 'active'
        ORDER BY created_at DESC
        LIMIT 1
        "#
    )
    .bind(&wallet)
    .fetch_optional(&state.db)
    .await?;

    if let Some(s) = strategy {
        // Get all mining sessions for this strategy
        let sessions: Vec<MiningSession> = sqlx::query_as::<_, MiningSession>(
            r#"
            SELECT ms.* FROM mining_sessions ms
            WHERE ms.user_id = $1 AND ms.round_id <= $2
            ORDER BY ms.round_id DESC
            "#
        )
        .bind(s.user_id)
        .bind(s.last_round_id.unwrap_or(0) as i64)
        .fetch_all(&state.db)
        .await?;

        // Calculate detailed progress
        let mut round_history = Vec::new();
        let mut total_deployed = 0.0;
        let mut total_rewards = 0.0;
        let mut wins = 0;
        let mut losses = 0;
        let mut breakevens = 0;

        for session in sessions {
            let deployed_sol = session.deployed_amount as f64 / blockchain::LAMPORTS_PER_SOL as f64;
            let rewards_sol = session.rewards_sol as f64 / blockchain::LAMPORTS_PER_SOL as f64;
            let profit_loss = rewards_sol - deployed_sol;
            let roi_percentage = if deployed_sol > 0.0 { (profit_loss / deployed_sol) * 100.0 } else { 0.0 };

            let profitability = session.profitability
                .unwrap_or_else(|| calculate_profitability(session.deployed_amount, session.rewards_sol));

            match profitability.as_str() {
                "win" => wins += 1,
                "loss" => losses += 1,
                "breakeven" => breakevens += 1,
                _ => {}
            }

            total_deployed += deployed_sol;
            total_rewards += rewards_sol;

            round_history.push(serde_json::json!({
                "round_id": session.round_id,
                "deployed_amount_sol": deployed_sol,
                "rewards_sol": rewards_sol,
                "profit_loss_sol": profit_loss,
                "roi_percentage": roi_percentage,
                "profitability": profitability,
                "status": session.status,
                "claimed": session.claimed,
                "created_at": session.created_at
            }));
        }

        let remaining_rounds = s.rounds - s.current_round;
        let total_profit_loss = total_rewards - total_deployed;
        let overall_roi = if total_deployed > 0.0 { (total_profit_loss / total_deployed) * 100.0 } else { 0.0 };
        let max_possible_loss = s.max_loss_sol - s.total_loss_sol;
        let risk_level = if max_possible_loss < s.current_amount_sol * 0.5 {
            "HIGH".to_string()
        } else if max_possible_loss < s.current_amount_sol {
            "MEDIUM".to_string()
        } else {
            "LOW".to_string()
        };

        Ok(Json(serde_json::json!({
            "success": true,
            "strategy_progress": {
                "id": s.id,
                "status": s.status,
                "current_round": s.current_round,
                "total_rounds": s.rounds,
                "remaining_rounds": remaining_rounds,
                "progress_percentage": (s.current_round as f64 / s.rounds as f64) * 100.0,
                
                "next_deployment": {
                    "amount_sol": s.current_amount_sol,
                    "round_id": s.last_round_id.map(|id| id + 1)
                },
                
                "totals": {
                    "deployed_sol": total_deployed,
                    "rewards_sol": total_rewards,
                    "profit_loss_sol": total_profit_loss,
                    "overall_roi_percentage": overall_roi
                },
                
                "risk_management": {
                    "max_loss_limit_sol": s.max_loss_sol,
                    "current_loss_sol": s.total_loss_sol,
                    "remaining_loss_budget_sol": max_possible_loss,
                    "risk_level": risk_level
                },
                
                "performance": {
                    "total_rounds_played": s.current_round,
                    "wins": wins,
                    "losses": losses,
                    "breakevens": breakevens,
                    "win_rate_percentage": if s.current_round > 0 { (wins as f64 / s.current_round as f64) * 100.0 } else { 0.0 }
                },
                
                "round_history": round_history,
                
                "created_at": s.created_at,
                "updated_at": s.updated_at
            }
        })))
    } else {
        Ok(Json(serde_json::json!({
            "success": false,
            "message": "No active martingale strategy found"
        })))
    }
}

async fn start_martingale(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<StartMartingaleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate input
    if payload.rounds <= 0 || payload.base_amount_sol <= 0.0 || payload.loss_multiplier <= 1.0 || payload.max_loss_sol <= 0.0 {
        return Err(ApiError::BadRequest("Invalid parameters: rounds > 0, base_amount > 0, loss_multiplier > 1, max_loss > 0".into()));
    }

    if payload.wallet_address.len() != 44 {
        return Err(ApiError::BadRequest("Invalid wallet address format".into()));
    }

    // Get or create user
    let user = get_or_create_user(&state.db, &payload.wallet_address).await?;

    // Check if user already has an active strategy
    let existing: Option<MartingaleStrategy> = sqlx::query_as::<_, MartingaleStrategy>(
        "SELECT * FROM martingale_strategies WHERE wallet_address = $1 AND status = 'active'"
    )
    .bind(&payload.wallet_address)
    .fetch_optional(&state.db)
    .await?;

    if existing.is_some() {
        return Err(ApiError::BadRequest("User already has an active Martingale strategy".into()));
    }

    // Create strategy record
    let strategy: MartingaleStrategy = sqlx::query_as::<_, MartingaleStrategy>(
        r#"
        INSERT INTO martingale_strategies (
            user_id, wallet_address, rounds, base_amount_sol, loss_multiplier, max_loss_sol,
            current_amount_sol
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING *
        "#
    )
    .bind(user.id)
    .bind(&payload.wallet_address)
    .bind(payload.rounds)
    .bind(payload.base_amount_sol)
    .bind(payload.loss_multiplier)
    .bind(payload.max_loss_sol)
    .bind(payload.base_amount_sol) // start with base amount
    .fetch_one(&state.db)
    .await?;

    // Try initial deploy immediately
    if let Ok(board) = blockchain::get_board_info(&state.rpc_client).await {
        if strategy.last_round_id != Some(board.round_id as i64) && strategy.current_round < strategy.rounds {
            let _ = deploy_for_round(&state, &strategy, board.round_id).await;
        }
    }

    // Start the background task
    let state_clone = state.clone();
    let strategy_id = strategy.id;
    tokio::spawn(async move {
        run_martingale_strategy(state_clone, strategy_id).await;
    });

    Ok(Json(serde_json::json!({
        "success": true,
        "strategy_id": strategy.id,
        "message": "Martingale strategy started"
    })))
}

async fn run_martingale_strategy(state: Arc<AppState>, strategy_id: uuid::Uuid) {
    info!("Starting Martingale strategy execution for strategy: {}", strategy_id);

    let mut last_round_id: Option<u64> = None;

    loop {
        // Check if strategy is still active
        let strategy: Option<MartingaleStrategy> = sqlx::query_as::<_, MartingaleStrategy>(
            "SELECT * FROM martingale_strategies WHERE id = $1"
        )
        .bind(strategy_id)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

        let strategy = match strategy {
            Some(s) if s.status == "active" => s,
            _ => {
                info!("Martingale strategy {} is no longer active", strategy_id);
                break;
            }
        };

        // Get current board
        let board = match blockchain::get_board_info(&state.rpc_client).await {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to get board info for strategy {}: {}", strategy_id, e);
                sleep(Duration::from_secs(10)).await;
                continue;
            }
        };

        // Check if round has changed
        if let Some(last) = last_round_id {
            if board.round_id != last {
                // Round has ended, process the result
                info!("Round {} ended for strategy {}, processing result", last, strategy_id);
                if let Err(e) = process_round_result(&state, &strategy, last).await {
                    error!("Failed to process round result for strategy {}: {}", strategy_id, e);
                    // Continue anyway
                }
            }
        }

        // Update last_round_id
        last_round_id = Some(board.round_id);

        // Check if we need to deploy for current round
        if strategy.last_round_id != Some(board.round_id as i64) && strategy.current_round < strategy.rounds {
            info!("Deploying for round {} in strategy {}", board.round_id, strategy_id);
            if let Err(e) = deploy_for_round(&state, &strategy, board.round_id).await {
                error!("Failed to deploy for strategy {}: {}", strategy_id, e);
                // If deployment fails, stop the strategy
                sqlx::query(
                    "UPDATE martingale_strategies SET status = 'stopped', updated_at = NOW() WHERE id = $1"
                )
                .bind(strategy_id)
                .execute(&state.db)
                .await
                .unwrap_or_default();
                break;
            }
        }

        // Check if strategy is complete
        let updated_strategy: Option<MartingaleStrategy> = sqlx::query_as::<_, MartingaleStrategy>(
            "SELECT * FROM martingale_strategies WHERE id = $1"
        )
        .bind(strategy_id)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

        if let Some(s) = updated_strategy {
            if s.current_round >= s.rounds || s.status != "active" {
                info!("Martingale strategy {} completed", strategy_id);
                break;
            }
        }

        // Wait before next check
        sleep(Duration::from_secs(30)).await;
    }

    info!("Martingale strategy {} execution finished", strategy_id);
}

async fn process_round_result(
    state: &Arc<AppState>,
    strategy: &MartingaleStrategy,
    round_id: u64,
) -> Result<(), ApiError> {
    // Find the mining session for this round and user
    let session: Option<MiningSession> = sqlx::query_as::<_, MiningSession>(
        r#"
        SELECT ms.* FROM mining_sessions ms
        JOIN users u ON ms.user_id = u.id
        WHERE u.id = $1 AND ms.round_id = $2
        ORDER BY ms.created_at DESC
        LIMIT 1
        "#
    )
    .bind(strategy.user_id)
    .bind(round_id as i64)
    .fetch_optional(&state.db)
    .await?;

    let session = match session {
        Some(s) => s,
        None => {
            warn!("No mining session found for user {} in round {}", strategy.user_id, round_id);
            return Ok(());
        }
    };

    // Get round info to determine winning square
    let round_info = match blockchain::get_round_info(&state.rpc_client, round_id).await {
        Ok(r) => r,
        Err(e) => {
            warn!("Failed to get round info for round {}: {}", round_id, e);
            return Ok(());
        }
    };

    // Update session with winning square
    sqlx::query(
        "UPDATE mining_sessions SET winning_square = $1, updated_at = NOW() WHERE id = $2"
    )
    .bind(round_info.winning_square as i32)
    .bind(session.id)
    .execute(&state.db)
    .await?;

    // Check if user won (deployed to winning square)
    let user_squares: Vec<usize> = session.squares.iter().map(|&s| s as usize).collect();
    let won = user_squares.contains(&round_info.winning_square);

    // Get user wallet for miner stats
    let user: User = sqlx::query_as::<_, User>(
        "SELECT * FROM users WHERE id = $1"
    )
    .bind(strategy.user_id)
    .fetch_one(&state.db)
    .await?;

    let pubkey: solana_sdk::pubkey::Pubkey = user.wallet_address.parse()
        .map_err(|_| ApiError::Internal("Invalid wallet address".to_string()))?;

    // Get current miner rewards to update the session
    let miner_stats = blockchain::get_miner_stats(&state.rpc_client, pubkey).await?;
    let current_rewards_sol = (miner_stats.rewards_sol * blockchain::LAMPORTS_PER_SOL as f64) as i64;
    let current_rewards_ore = miner_stats.rewards_ore as i64;

    // Update session with current rewards
    sqlx::query(
        "UPDATE mining_sessions SET rewards_sol = $1, rewards_ore = $2, updated_at = NOW() WHERE id = $3"
    )
    .bind(current_rewards_sol)
    .bind(current_rewards_ore)
    .bind(session.id)
    .execute(&state.db)
    .await?;

    // Calculate profitability based on win/loss
    let profitability = if won {
        "win"
    } else {
        "loss"
    }.to_string();

    // Update session profitability
    sqlx::query(
        "UPDATE mining_sessions SET profitability = $1, updated_at = NOW() WHERE id = $2"
    )
    .bind(&profitability)
    .bind(session.id)
    .execute(&state.db)
    .await?;

    let deployed_sol = session.deployed_amount as f64 / blockchain::LAMPORTS_PER_SOL as f64;
    let rewards_sol = session.rewards_sol as f64 / blockchain::LAMPORTS_PER_SOL as f64;
    let loss = deployed_sol - rewards_sol;

    // Update strategy totals
    let new_total_deployed = strategy.total_deployed_sol + deployed_sol;
    let new_total_rewards = strategy.total_rewards_sol + rewards_sol;
    let new_total_loss = strategy.total_loss_sol + if loss > 0.0 { loss } else { 0.0 };

    let mut new_current_amount = strategy.current_amount_sol;
    let mut new_current_round = strategy.current_round;

    if profitability == "loss" {
        new_current_amount *= strategy.loss_multiplier;
        // Check max loss
        if new_total_loss + new_current_amount > strategy.max_loss_sol {
            // Stop strategy
            sqlx::query(
                r#"
                UPDATE martingale_strategies
                SET status = 'stopped', total_deployed_sol = $1, total_rewards_sol = $2,
                    total_loss_sol = $3, updated_at = NOW()
                WHERE id = $4
                "#
            )
            .bind(new_total_deployed)
            .bind(new_total_rewards)
            .bind(new_total_loss)
            .bind(strategy.id)
            .execute(&state.db)
            .await?;
            return Ok(());
        }
    } else {
        // Win or breakeven, reset to base amount
        new_current_amount = strategy.base_amount_sol;
    }

    new_current_round += 1;

    // Update strategy
    sqlx::query(
        r#"
        UPDATE martingale_strategies
        SET current_round = $1, current_amount_sol = $2, total_deployed_sol = $3,
            total_rewards_sol = $4, total_loss_sol = $5, updated_at = NOW()
        WHERE id = $6
        "#
    )
    .bind(new_current_round)
    .bind(new_current_amount)
    .bind(new_total_deployed)
    .bind(new_total_rewards)
    .bind(new_total_loss)
    .bind(strategy.id)
    .execute(&state.db)
    .await?;

    Ok(())
}

async fn deploy_for_round(
    state: &Arc<AppState>,
    strategy: &MartingaleStrategy,
    round_id: u64,
) -> Result<(), ApiError> {
    // Get user wallet
    let _user: User = sqlx::query_as::<_, User>(
        "SELECT * FROM users WHERE id = $1"
    )
    .bind(strategy.user_id)
    .fetch_one(&state.db)
    .await?;

    // Checkpoint miner first
    match blockchain::checkpoint_miner(&state.rpc_client, &state.config.keypair_path, None).await {
        Ok(_) => {},
        Err(ApiError::Rpc(e)) => {
            error!("Martingale strategy {} checkpoint failed: {}", strategy.id, e);
            return Err(handle_transaction_rpc_error(&e));
        }
        Err(other) => return Err(other),
    };

    // Deploy
    let amount_lamports = (strategy.current_amount_sol * blockchain::LAMPORTS_PER_SOL as f64) as u64;
    let _signature = match blockchain::deploy_ore(
        &state.rpc_client,
        &state.config.keypair_path,
        amount_lamports,
        None, // deploy to all squares
    )
    .await {
        Ok(sig) => sig,
        Err(ApiError::Rpc(e)) => {
            error!("Martingale strategy {} deployment failed: {}", strategy.id, e);
            return Err(handle_transaction_rpc_error(&e));
        }
        Err(other) => return Err(other),
    };

    // Create mining session record
    sqlx::query(
        r#"
        INSERT INTO mining_sessions (
            user_id, round_id, deployed_amount, squares, status, winning_square
        ) VALUES ($1, $2, $3, $4, 'active', $5)
        "#
    )
    .bind(strategy.user_id)
    .bind(round_id as i64)
    .bind(amount_lamports as i64)
    .bind(vec![0i32,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24]) // all squares
    .bind(None::<i32>)
    .execute(&state.db)
    .await?;

    // Update strategy last_round_id
    sqlx::query(
        "UPDATE martingale_strategies SET last_round_id = $1, updated_at = NOW() WHERE id = $2"
    )
    .bind(round_id as i64)
    .bind(strategy.id)
    .execute(&state.db)
    .await?;

    info!("Deployed {} SOL for strategy {} in round {}", strategy.current_amount_sol, strategy.id, round_id);

    Ok(())
}

async fn update_round_results(state: &Arc<AppState>, round_id: u64) -> Result<(), ApiError> {
    // Get round info
    let round_info = blockchain::get_round_info(&state.rpc_client, round_id).await?;

    // Find all mining sessions for this round that don't have winning_square set
    let sessions: Vec<MiningSession> = sqlx::query_as::<_, MiningSession>(
        "SELECT * FROM mining_sessions WHERE round_id = $1 AND winning_square IS NULL"
    )
    .bind(round_id as i64)
    .fetch_all(&state.db)
    .await?;

    for session in sessions {
        // Update winning_square
        sqlx::query(
            "UPDATE mining_sessions SET winning_square = $1, updated_at = NOW() WHERE id = $2"
        )
        .bind(round_info.winning_square as i32)
        .bind(session.id)
        .execute(&state.db)
        .await?;

        // Calculate profitability
        let user_squares: Vec<i32> = session.squares.iter().map(|&s| s as i32).collect();
        let won = user_squares.contains(&(round_info.winning_square as i32));
        let profitability = if won { "win" } else { "loss" };

        sqlx::query(
            "UPDATE mining_sessions SET profitability = $1, updated_at = NOW() WHERE id = $2"
        )
        .bind(profitability)
        .bind(session.id)
        .execute(&state.db)
        .await?;

        info!("Updated session {} for round {}: won={}, profitability={}", session.id, round_id, won, profitability);
    }

    Ok(())
}

async fn global_stats(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    // Total sessions (deployments)
    let total_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mining_sessions")
        .fetch_one(&state.db)
        .await?;

    // Total deployed amount
    let total_deployed_amount: Option<i64> = sqlx::query_scalar(
        "SELECT CAST(SUM(deployed_amount) AS BIGINT) FROM mining_sessions"
    )
    .fetch_one(&state.db)
    .await?;

    // Active users for different time periods
    let active_users_24h: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT user_id) FROM mining_sessions WHERE created_at > NOW() - INTERVAL '24 hours'"
    )
    .fetch_one(&state.db)
    .await?;

    let active_users_7d: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT user_id) FROM mining_sessions WHERE created_at > NOW() - INTERVAL '7 days'"
    )
    .fetch_one(&state.db)
    .await?;

    let active_users_30d: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT user_id) FROM mining_sessions WHERE created_at > NOW() - INTERVAL '30 days'"
    )
    .fetch_one(&state.db)
    .await?;

    Ok(Json(serde_json::json!({
        "total_sessions": total_sessions,
        "total_deploys": total_sessions, // Alias for clarity
        "total_deployed_amount": total_deployed_amount.unwrap_or(0),
        "active_users": {
            "24h": active_users_24h,
            "7d": active_users_7d,
            "30d": active_users_30d
        }
    })))
}

async fn deployment_history(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get user sessions with profitability
    let mut sessions: Vec<MiningSession> = sqlx::query_as::<_, MiningSession>(
        r#"
        SELECT ms.* FROM mining_sessions ms
        JOIN users u ON ms.user_id = u.id
        WHERE u.wallet_address = $1
        ORDER BY ms.created_at DESC
        LIMIT 100
        "#,
    )
    .bind(&wallet)
    .fetch_all(&state.db)
    .await?;

    // Calculate profitability for sessions that don't have it set
    for session in &mut sessions {
        if session.profitability.is_none() {
            // Get round info if winning_square not set
            if session.winning_square.is_none() {
                if let Ok(round_info) = blockchain::get_round_info(&state.rpc_client, session.round_id as u64).await {
                    session.winning_square = Some(round_info.winning_square as i32);
                    // Update winning_square in db
                    sqlx::query(
                        "UPDATE mining_sessions SET winning_square = $1, updated_at = NOW() WHERE id = $2"
                    )
                    .bind(session.winning_square)
                    .bind(session.id)
                    .execute(&state.db)
                    .await?;
                }
            }

            // Calculate profitability based on winning square
            if let Some(winning_square) = session.winning_square {
                let user_squares: Vec<i32> = session.squares.iter().map(|&s| s as i32).collect();
                let won = user_squares.contains(&winning_square);
                session.profitability = Some(if won { "win".to_string() } else { "loss".to_string() });

                // Update in database
                sqlx::query(
                    "UPDATE mining_sessions SET profitability = $1, updated_at = NOW() WHERE id = $2"
                )
                .bind(&session.profitability)
                .bind(session.id)
                .execute(&state.db)
                .await?;
            }
        }
    }

    // Calculate stats
    let total_deploys = sessions.len();
    let wins = sessions.iter().filter(|s| s.profitability.as_deref() == Some("win")).count();
    let losses = sessions.iter().filter(|s| s.profitability.as_deref() == Some("loss")).count();
    let breakevens = sessions.iter().filter(|s| s.profitability.as_deref() == Some("breakeven")).count();
    let pending = sessions.iter().filter(|s| s.profitability.is_none()).count();

    let total_deployed_lamports: i64 = sessions.iter().map(|s| s.deployed_amount).sum();
    let total_rewards_sol: i64 = sessions.iter().map(|s| s.rewards_sol).sum();

    let win_rate = if total_deploys > 0 {
        (wins as f64) / (total_deploys as f64) * 100.0
    } else {
        0.0
    };

    Ok(Json(serde_json::json!({
        "wallet_address": wallet,
        "total_deploys": total_deploys,
        "stats": {
            "wins": wins,
            "losses": losses,
            "breakevens": breakevens,
            "pending": pending,
            "win_rate_percentage": win_rate
        },
        "totals": {
            "deployed_sol": total_deployed_lamports as f64 / blockchain::LAMPORTS_PER_SOL as f64,
            "rewards_sol": total_rewards_sol as f64 / blockchain::LAMPORTS_PER_SOL as f64
        },
        "sessions": sessions.into_iter().map(|s| {
            serde_json::json!({
                "id": s.id,
                "round_id": s.round_id,
                "deployed_amount_sol": s.deployed_amount as f64 / blockchain::LAMPORTS_PER_SOL as f64,
                "rewards_sol": s.rewards_sol as f64 / blockchain::LAMPORTS_PER_SOL as f64,
                "rewards_ore": s.rewards_ore,
                "claimed": s.claimed,
                "profitability": s.profitability,
                "winning_square": s.winning_square,
                "created_at": s.created_at,
                "updated_at": s.updated_at
            })
        }).collect::<Vec<_>>()
    })))
}

async fn update_burner_wallet(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateBurnerWalletRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate wallet addresses
    if payload.wallet_address.len() != 44 {
        return Err(ApiError::BadRequest("Invalid main wallet address format".into()));
    }
    if payload.burner_address.len() != 44 {
        return Err(ApiError::BadRequest("Invalid burner wallet address format".into()));
    }

    // Update the burner_address for the user
    let rows_affected = sqlx::query(
        "UPDATE users SET burner_address = $1, updated_at = NOW() WHERE wallet_address = $2"
    )
    .bind(&payload.burner_address)
    .bind(&payload.wallet_address)
    .execute(&state.db)
    .await?
    .rows_affected();

    if rows_affected == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Burner address updated successfully",
        "wallet_address": payload.wallet_address,
        "burner_address": payload.burner_address
    })))
}

// ============================================================================
// Router
// ============================================================================

fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(|| async { "ORE Mining Backend API - Running" }))
        .route("/health", get(health_check))
        .route("/ws", get(websocket_handler))  // NEW: WebSocket endpoint
        .route("/api/board", get(get_board))
        .route("/api/round/:id", get(get_round))
        .route("/api/treasury", get(get_treasury))
        .route("/api/miner/:wallet", get(get_miner))
        .route("/api/miner/:wallet/sessions", get(get_user_sessions))
        .route("/api/user/register", post(register_user))
        .route("/api/deploy", post(deploy))
        .route("/api/claim", post(claim))
        .route("/api/checkpoint", post(checkpoint))
        .route("/api/martingale/start", post(start_martingale))
        .route("/api/martingale/active/:wallet", get(get_active_martingale))
        .route("/api/mining/active/:wallet", get(get_active_mining_sessions))
        .route("/api/martingale/progress/:wallet", get(get_martingale_progress))
        .route("/api/stats/global", get(global_stats))
        .route("/api/miner/:wallet/history", get(deployment_history))
        .route("/api/user/burner-address", post(update_burner_wallet))
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::any())
                .allow_methods(AllowMethods::list([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::DELETE,
                    Method::OPTIONS,
                ]))
                .allow_headers(AllowHeaders::list([
                    HeaderName::from_static("content-type"),
                    HeaderName::from_static("authorization"),
                    HeaderName::from_static("x-requested-with"),
                ]))
                .max_age(tokio::time::Duration::from_secs(86400))
        )
        .with_state(state)
}

// ============================================================================
// Main Application
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,ore_backend=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting ORE Mining Backend with WebSocket support...");

    // Load configuration
    let config = Arc::new(Config::from_env()?);
    info!("Configuration loaded");

    // Setup database with better connection management
    let db = PgPoolOptions::new()
        .max_connections(50)              // Increased from 10
        .min_connections(5)               // Keep minimum connections
        .max_lifetime(Duration::from_secs(1800)) // 30 minutes
        .idle_timeout(Duration::from_secs(300))  // 5 minutes
        .acquire_timeout(Duration::from_secs(30)) // 30 seconds to acquire
        .test_before_acquire(true)        // Test connections before acquiring to prevent cached plan issues
        .connect(&config.database_url)
        .await?;
    
    // setup_database(&db).await?;
    info!("Database connected and initialized with connection pool");

    // Setup Redis
    let redis_client = redis::Client::open(config.redis_url.as_str())?;
    let redis = redis_client.get_connection_manager().await?;
    info!("Redis connected");

    // Setup Solana RPC client
    let rpc_client = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(
        config.rpc_url.clone(),
    ));
    info!("RPC client initialized: {}", config.rpc_url);

    // Create broadcast channel for WebSocket with larger buffer
    let (broadcast_tx, _) = broadcast::channel(1000);
    info!("WebSocket broadcast channel created");

    // Create connection tracking
    let connection_count = Arc::new(AtomicUsize::new(0));
    let max_connections = 100; // Limit concurrent WebSocket connections

    // Create application state
    let state = Arc::new(AppState {
        db,
        redis,
        rpc_client,
        config: config.clone(),
        broadcast: broadcast_tx,
        connection_count: connection_count.clone(),
        max_connections,
    });

    // Start background update broadcaster
    start_update_broadcaster(state.clone());
    info!("Background update broadcaster started");

    // Build router
    let app = create_router(state);

    // Start server
    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    info!("🚀 Server listening on http://0.0.0.0:3000");
    info!("📊 API Endpoints:");
    info!("   GET  /health");
    info!("   GET  /ws              <- WebSocket endpoint");
    info!("   GET  /api/board");
    info!("   GET  /api/round/:id   <- Round information with winning square");
    info!("   GET  /api/treasury");
    info!("   GET  /api/miner/:wallet");
    info!("   GET  /api/miner/:wallet/sessions");
    info!("   GET  /api/miner/:wallet/history    <- Deployment history with win/loss");
    info!("   POST /api/user/register");
    info!("   POST /api/deploy");
    info!("   POST /api/claim");
    info!("   POST /api/checkpoint");
    info!("   POST /api/martingale/start");
    info!("   GET  /api/martingale/active/:wallet");
    info!("   GET  /api/martingale/progress/:wallet");
    info!("   GET  /api/mining/active/:wallet");
    info!("   GET  /api/stats/global"); 
    info!("   POST /api/user/burner-address");

    axum::serve(listener, app).await?;

    Ok(())
}

// Check Git Action