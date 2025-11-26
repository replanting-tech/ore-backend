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
    pub broadcast: broadcast::Sender<WsMessage>,  // NEW: Broadcast channel
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
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    
    // Subscribe to broadcast
    let mut rx = state.broadcast.subscribe();
    
    // Client info
    info!("WebSocket client connected");
    
    // Subscribed topics for this client
    let mut subscribed_topics: Vec<String> = vec![];
    
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
    
    // Handle incoming and outgoing messages
    loop {
        tokio::select! {
            // Handle messages from client
            Some(Ok(msg)) = receiver.next() => {
                match msg {
                    Message::Text(text) => {
                        if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                            match ws_msg {
                                WsMessage::Subscribe { topic } => {
                                    info!("Client subscribed to: {}", topic);
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
                                    info!("Client unsubscribed from: {}", topic);
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
                        info!("WebSocket client disconnected");
                        break;
                    }
                    _ => {}
                }
            }
            
            // Handle broadcast messages
            Ok(broadcast_msg) = rx.recv() => {
                // Filter based on subscribed topics
                let should_send = match &broadcast_msg {
                    WsMessage::BoardUpdate { .. } => subscribed_topics.contains(&"board".to_string()),
                    WsMessage::MinerUpdate { wallet, .. } => {
                        subscribed_topics.contains(&format!("miner:{}", wallet)) ||
                        subscribed_topics.contains(&"miners".to_string())
                    }
                    WsMessage::TreasuryUpdate { .. } => subscribed_topics.contains(&"treasury".to_string()),
                    WsMessage::SquaresUpdate { .. } => subscribed_topics.contains(&"squares".to_string()),
                    WsMessage::RoundComplete { .. } => true, // Always send round complete
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
        }
    }
    
    info!("WebSocket handler closed");
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
        // Create separate Redis connection for broadcaster to enable caching
        let redis_client = redis::Client::open(state.config.redis_url.as_str())
            .expect("Failed to create Redis client for broadcaster");
        let mut redis = redis_client.get_connection_manager().await
            .expect("Failed to get Redis connection for broadcaster");

        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(15));
        let mut counter = 0u64;

        loop {
            interval.tick().await;
            counter += 1;

            // Broadcast board updates (every 60 seconds, every 4 ticks)
            if counter % 4 == 0 {
                if let Ok(board) = blockchain::get_board_info(&state.rpc_client).await {
                    let msg = WsMessage::BoardUpdate { board };
                    let _ = state.broadcast.send(msg);
                } else {
                    warn!("Failed to fetch board info for broadcast");
                }
            }

            // Broadcast square stats every 45 seconds (every 3 ticks) - now with caching
            if counter % 3 == 0 {
                if let Ok(squares) = blockchain::get_square_stats_cached(&state.rpc_client, &mut redis).await {
                    let msg = WsMessage::SquaresUpdate { squares };
                    let _ = state.broadcast.send(msg);
                } else {
                    warn!("Failed to fetch square stats for broadcast");
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
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerStats {
    pub address: String,
    pub authority: String,
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
    pub amount: f64,
    pub square_id: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimResponse {
    pub signature: String,
    pub sol_claimed: f64,
    pub ore_claimed: f64,
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
                        return Err(ApiError::Internal(format!("Rate limit exceeded after {} attempts: {}", attempts, error_msg)));
                    }
                    
                    // Exponential backoff (500ms, 1s, 2s)
                    let delay_millis = 500 * (2u64.pow(attempts - 1));
                    let delay = Duration::from_millis(delay_millis);
                    warn!("RPC rate limited, retrying in {:?} (attempt {}/{})", delay, attempts, max_attempts);
                    sleep(delay).await;
                    continue;
                }
                
                // For other RPC errors, return immediately
                return Err(ApiError::Internal(format!("RPC error: {}", error_msg)));
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
        with_rpc_retry(|| async {
            let miner_pda = ore_api::state::miner_pda(authority);
            let account = match rpc.get_account(&miner_pda.0).await {
                Ok(account) => account,
                Err(e) => {
                    // Handle account not found - this is expected for miners that haven't been created yet
                    if e.to_string().contains("AccountNotFound") {
                        return Err(ApiError::NotFound);
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

    pub async fn get_treasury_info(rpc: &RpcClient) -> Result<TreasuryInfo, ApiError> {
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

    pub async fn claim_rewards(
        rpc: &RpcClient,
        keypair_path: &str,
    ) -> Result<ClaimResponse, ApiError> {
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

    pub async fn checkpoint_miner(
        rpc: &RpcClient,
        keypair_path: &str,
        authority: Option<Pubkey>,
    ) -> Result<String, ApiError> {
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
            created_at TIMESTAMPTZ DEFAULT NOW(),
            updated_at TIMESTAMPTZ DEFAULT NOW()
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_user ON mining_sessions(user_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_round ON mining_sessions(round_id);
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

async fn get_treasury(State(state): State<Arc<AppState>>) -> Result<Json<TreasuryInfo>, ApiError> {
    let treasury = blockchain::get_treasury_info(&state.rpc_client).await?;
    Ok(Json(treasury))
}

async fn get_miner(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> Result<Json<MinerStats>, ApiError> {
    // GANTI baris ini:
    // let pubkey = solana_sdk::pubkey::Pubkey::from_str(&wallet)
    
    // DENGAN ini:
    let pubkey: solana_sdk::pubkey::Pubkey = wallet.parse()
        .map_err(|_| ApiError::BadRequest("Invalid wallet address".into()))?;
    
    let stats = blockchain::get_miner_stats(&state.rpc_client, pubkey).await?;
    Ok(Json(stats))
}

async fn get_user_sessions(
    State(state): State<Arc<AppState>>,
    Path(wallet): Path<String>,
) -> Result<Json<Vec<MiningSession>>, ApiError> {
    let sessions = sqlx::query_as::<_, MiningSession>(
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

async fn deploy(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DeployRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate wallet address length
    if payload.wallet_address.len() != 44 {
        return Err(ApiError::BadRequest("Invalid wallet address: must be exactly 44 characters".into()));
    }

    info!("Deploy request received: wallet={}, amount={} SOL, square_id={:?}",
          payload.wallet_address, payload.amount, payload.square_id);

    // Convert amount from SOL to lamports
    let amount_lamports = (payload.amount * blockchain::LAMPORTS_PER_SOL as f64) as u64;
    info!("Converted amount: {} SOL = {} lamports", payload.amount, amount_lamports);

    // Get or create user
    info!("Getting or creating user for wallet: {}", payload.wallet_address);
    let user = get_or_create_user(&state.db, &payload.wallet_address).await?;
    info!("User found/created: id={}, wallet={}", user.id, user.wallet_address);

    // Get current board info for round_id
    info!("Fetching current board info");
    let board = blockchain::get_board_info(&state.rpc_client).await?;
    info!("Board info: round_id={}, current_slot={}", board.round_id, board.current_slot);

    // Ensure miner is checkpointed before deploying
    info!("Checkpointing miner before deploy");
    let checkpoint_signature = blockchain::checkpoint_miner(&state.rpc_client, &state.config.keypair_path, None).await?;
    info!("Checkpoint completed: signature={}", checkpoint_signature);

    // Perform blockchain deployment
    info!("Performing blockchain deploy: amount={} lamports, square_id={:?}", amount_lamports, payload.square_id);
    let signature = blockchain::deploy_ore(
        &state.rpc_client,
        &state.config.keypair_path,
        amount_lamports,
        payload.square_id,
    )
    .await?;
    info!("Deploy transaction successful: signature={}", signature);

    // Create mining session record
    let squares = if let Some(square_id) = payload.square_id {
        vec![square_id as i32]
    } else {
        (0..25).collect() // All squares if no specific square
    };
    info!("Creating mining session record: user_id={}, round_id={}, deployed_amount={}, squares={:?}",
          user.id, board.round_id, amount_lamports, squares);

    sqlx::query(
        r#"
        INSERT INTO mining_sessions (
            user_id, round_id, deployed_amount, squares, status
        ) VALUES ($1, $2, $3, $4, 'active')
        "#
    )
    .bind(user.id)
    .bind(board.round_id as i64)
    .bind(amount_lamports as i64)
    .bind(&squares)
    .execute(&state.db)
    .await?;
    info!("Mining session created successfully");

    let response = serde_json::json!({
        "success": true,
        "signature": signature,
        "amount": payload.amount,
        "square": payload.square_id,
        "user_id": user.id
    });
    info!("Deploy request completed successfully: {:?}", response);

    Ok(Json(response))
}

async fn claim(State(state): State<Arc<AppState>>) -> Result<Json<ClaimResponse>, ApiError> {
    let response = blockchain::claim_rewards(&state.rpc_client, &state.config.keypair_path).await?;
    Ok(Json(response))
}

async fn checkpoint(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let signature = blockchain::checkpoint_miner(&state.rpc_client, &state.config.keypair_path, None).await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "signature": signature
    })))
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

// ============================================================================
// Router
// ============================================================================

fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(|| async { "ORE Mining Backend API - Running" }))
        .route("/health", get(health_check))
        .route("/ws", get(websocket_handler))  // NEW: WebSocket endpoint
        .route("/api/board", get(get_board))
        .route("/api/treasury", get(get_treasury))
        .route("/api/miner/:wallet", get(get_miner))
        .route("/api/miner/:wallet/sessions", get(get_user_sessions))
        .route("/api/user/register", post(register_user))
        .route("/api/deploy", post(deploy))
        .route("/api/claim", post(claim))
        .route("/api/checkpoint", post(checkpoint))
        .route("/api/stats/global", get(global_stats))
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

    // Setup database
    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    
    // setup_database(&db).await?;
    info!("Database connected and initialized");

    // Setup Redis
    let redis_client = redis::Client::open(config.redis_url.as_str())?;
    let redis = redis_client.get_connection_manager().await?;
    info!("Redis connected");

    // Setup Solana RPC client
    let rpc_client = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(
        config.rpc_url.clone(),
    ));
    info!("RPC client initialized: {}", config.rpc_url);

    // Create broadcast channel for WebSocket
    let (broadcast_tx, _) = broadcast::channel(100);
    info!("WebSocket broadcast channel created");

    // Create application state
    let state = Arc::new(AppState {
        db,
        redis,
        rpc_client,
        config: config.clone(),
        broadcast: broadcast_tx,  // NEW
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
    info!("   GET  /api/treasury");
    info!("   GET  /api/miner/:wallet");
    info!("   GET  /api/miner/:wallet/sessions");
    info!("   POST /api/user/register");
    info!("   POST /api/deploy");
    info!("   POST /api/claim");
    info!("   POST /api/checkpoint");
    info!("   GET  /api/stats/global");

    axum::serve(listener, app).await?;

    Ok(())
}

// Check Git Action