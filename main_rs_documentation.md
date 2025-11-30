# Dokumentasi Lengkap main.rs - Backend Mining ORE

## Pendahuluan

File `main.rs` adalah kode utama dari sebuah aplikasi backend yang ditulis dalam bahasa pemrograman Rust. Aplikasi ini dirancang untuk mendukung aktivitas mining (penambangan) cryptocurrency bernama ORE di jaringan blockchain Solana.

### Apa itu Aplikasi Ini?

Aplikasi ini berfungsi sebagai:
- **Server web API** yang menyediakan endpoint untuk interaksi dengan blockchain
- **WebSocket server** untuk komunikasi real-time dengan klien
- **Sistem manajemen database** untuk menyimpan data pengguna dan sesi mining
- **Integrasi blockchain** untuk berinteraksi dengan jaringan Solana
- **Sistem trading otomatis** menggunakan strategi Martingale

### Siapa yang Menggunakan Ini?

Aplikasi ini ditujukan untuk:
- Pengguna yang ingin mining ORE secara otomatis
- Developer yang membangun aplikasi frontend untuk mining
- Sistem yang memerlukan data real-time dari blockchain ORE

## Konsep Blockchain Dasar (Untuk Pemula)

Sebelum memahami kode ini, mari pahami konsep-konsep blockchain dasar:

### Apa itu Blockchain?
Blockchain adalah sistem pencatatan digital yang terdesentralisasi dan aman. Bayangkan seperti buku besar digital yang tidak bisa diubah dan didistribusikan ke banyak komputer.

### Apa itu Solana?
Solana adalah platform blockchain yang sangat cepat dan scalable. Ia bisa memproses ribuan transaksi per detik, jauh lebih cepat dari Bitcoin atau Ethereum.

### Apa itu ORE?
ORE adalah cryptocurrency yang bisa "ditambang" di jaringan Solana. Mining ORE melibatkan:
- **Deployment**: Menempatkan SOL (cryptocurrency utama Solana) ke dalam "square" virtual
- **Waiting**: Menunggu ronde mining selesai
- **Claiming**: Mengklaim reward ORE dan SOL yang didapat

### Konsep-Konsep Penting:
- **Wallet**: Dompet digital untuk menyimpan cryptocurrency
- **Transaction**: Transfer atau interaksi dengan blockchain
- **RPC**: Remote Procedure Call - cara berkomunikasi dengan node blockchain
- **PDA**: Program Derived Address - alamat khusus di Solana
- **Lamports**: Unit terkecil dari SOL (1 SOL = 1 miliar lamports)

## Arsitektur Aplikasi

Aplikasi ini menggunakan arsitektur berikut:

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   Frontend      │    │   Backend       │    │   Blockchain    │
│   (Browser)     │◄──►│   (Rust/Axum)   │◄──►│   (Solana)      │
└─────────────────┘    └─────────────────┘    └─────────────────┘
                              │
                              ▼
                       ┌─────────────────┐
                       │   Database      │
                       │   (PostgreSQL)  │
                       └─────────────────┘
                              │
                              ▼
                       ┌─────────────────┐
                       │   Cache         │
                       │   (Redis)       │
                       └─────────────────┘
```

### Teknologi yang Digunakan:
- **Rust**: Bahasa pemrograman utama
- **Axum**: Web framework untuk Rust
- **PostgreSQL**: Database relasional
- **Redis**: Database cache untuk performa
- **Solana SDK**: Library untuk interaksi dengan blockchain Solana
- **Tokio**: Runtime async untuk Rust

## Komponen Utama

### 1. Konfigurasi dan State Aplikasi

```rust
pub struct AppState {
    pub db: PgPool,                    // Koneksi database
    pub redis: redis::aio::ConnectionManager,  // Koneksi Redis
    pub rpc_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>, // Koneksi Solana
    pub config: Arc<Config>,           // Konfigurasi aplikasi
    pub broadcast: broadcast::Sender<WsMessage>, // Channel WebSocket
}
```

**AppState** menyimpan semua koneksi dan konfigurasi yang dibutuhkan aplikasi. Ini adalah "state global" yang diakses oleh semua handler.

### 2. WebSocket System

Aplikasi ini menyediakan komunikasi real-time melalui WebSocket. Klien bisa subscribe ke berbagai "topic" seperti:
- `board`: Informasi ronde mining saat ini
- `treasury`: Data treasury (kas) global
- `miner:{wallet}`: Statistik miner tertentu
- `squares`: Data semua square mining

**Cara Kerja:**
1. Klien connect ke `/ws`
2. Klien kirim pesan `Subscribe { topic: "board" }`
3. Server kirim update real-time setiap 15-60 detik

### 3. API Endpoints

Aplikasi menyediakan REST API endpoints:

#### GET Endpoints:
- `/health`: Cek status server
- `/api/board`: Info ronde mining saat ini
- `/api/treasury`: Info treasury global
- `/api/miner/{wallet}`: Statistik miner
- `/api/miner/{wallet}/sessions`: Riwayat mining user
- `/api/miner/{wallet}/history`: History deployment dengan win/loss
- `/api/martingale/active/{wallet}`: Strategi Martingale aktif
- `/api/martingale/progress/{wallet}`: Progress strategi Martingale
- `/api/mining/active/{wallet}`: Sesi mining aktif
- `/api/stats/global`: Statistik global aplikasi

#### POST Endpoints:
- `/api/user/register`: Registrasi user baru
- `/api/deploy`: Deploy SOL untuk mining
- `/api/claim`: Klaim reward
- `/api/checkpoint`: Checkpoint miner
- `/api/martingale/start`: Mulai strategi Martingale

### 4. Sistem Database

Aplikasi menggunakan PostgreSQL dengan tabel:
- `users`: Data pengguna
- `mining_sessions`: Record setiap deployment
- `martingale_strategies`: Konfigurasi strategi trading otomatis

### 5. Integrasi Blockchain

Modul `blockchain` menangani semua interaksi dengan Solana:

#### Fungsi Utama:
- `get_board_info()`: Ambil info ronde saat ini
- `get_miner_stats()`: Ambil statistik miner
- `get_treasury_info()`: Ambil info treasury
- `deploy_ore()`: Lakukan deployment mining
- `claim_rewards()`: Klaim reward
- `checkpoint_miner()`: Checkpoint untuk update data

## Penjelasan Fungsi-Fungsi Utama

### Fungsi `main()`

```rust
#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // 1. Setup logging
    // 2. Load config dari environment
    // 3. Connect ke database
    // 4. Connect ke Redis
    // 5. Setup RPC client Solana
    // 6. Create WebSocket broadcast channel
    // 7. Start background broadcaster
    // 8. Setup router dengan semua endpoints
    // 9. Start server di port 3000
}
```

**main()** adalah entry point aplikasi. Ia melakukan setup semua komponen dan menjalankan server.

### Fungsi WebSocket Handler

```rust
pub async fn websocket_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}
```

**websocket_handler()** menangani upgrade HTTP ke WebSocket dan memanggil `handle_socket()`.

**handle_socket()**:
- Menerima koneksi WebSocket
- Subscribe ke broadcast channel
- Handle pesan dari klien (subscribe/unsubscribe/ping)
- Forward broadcast messages ke klien berdasarkan subscription

### Fungsi API Handlers

#### `deploy()` - Fungsi Deployment

```rust
async fn deploy(State(state): State<Arc<AppState>>, Json(payload): Json<DeployRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    // 1. Validasi input
    // 2. Get/create user
    // 3. Get board info untuk round_id
    // 4. Checkpoint miner
    // 5. Deploy ke blockchain
    // 6. Save ke database
}
```

**deploy()** menangani request deployment:
1. Validasi wallet address dan amount
2. Cari atau buat user di database
3. Ambil info ronde saat ini
4. Checkpoint miner (update data)
5. Lakukan transaksi deploy di blockchain
6. Simpan record di database

#### `start_martingale()` - Mulai Strategi Trading Otomatis

```rust
async fn start_martingale(State(state): State<Arc<AppState>>, Json(payload): Json<StartMartingaleRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    // 1. Validasi parameter
    // 2. Get/create user
    // 3. Cek tidak ada strategi aktif
    // 4. Create strategy record
    // 5. Start background task
}
```

**Strategi Martingale** adalah sistem trading yang:
- Jika kalah: Naikkan amount deployment berikutnya
- Jika menang: Reset ke amount awal
- Ada batas maksimal loss untuk menghindari kerugian besar

### Fungsi Background Tasks

#### `start_update_broadcaster()`

```rust
pub fn start_update_broadcaster(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            // Broadcast board updates setiap 60 detik
            // Broadcast square stats setiap 45 detik
            // Broadcast treasury setiap 150 detik
        }
    });
}
```

**start_update_broadcaster()** menjalankan task background yang:
- Fetch data terbaru dari blockchain
- Broadcast ke semua klien WebSocket yang subscribe
- Menggunakan cache Redis untuk performa

#### `run_martingale_strategy()`

```rust
async fn run_martingale_strategy(state: Arc<AppState>, strategy_id: uuid::Uuid) {
    loop {
        // 1. Cek strategy masih aktif
        // 2. Get board info
        // 3. Process hasil ronde sebelumnya
        // 4. Deploy untuk ronde saat ini jika perlu
        // 5. Wait 30 detik
    }
}
```

**run_martingale_strategy()** menjalankan strategi trading otomatis:
1. Monitor ronde mining
2. Hitung profit/loss setiap ronde
3. Deploy amount berikutnya berdasarkan strategi
4. Stop jika mencapai limit atau selesai

### Fungsi Blockchain Integration

#### `get_board_info()`

```rust
pub async fn get_board_info(rpc: &RpcClient) -> Result<BoardInfo, ApiError> {
    // 1. Get board PDA
    // 2. Fetch account data dari Solana
    // 3. Parse binary data menggunakan bytemuck
    // 4. Get current clock untuk calculate time remaining
    // 5. Return BoardInfo struct
}
```

**get_board_info()** mengambil data ronde mining saat ini:
- `round_id`: ID ronde saat ini
- `start_slot`/`end_slot`: Slot Solana dimana ronde dimulai/berakhir
- `time_remaining_sec`: Waktu tersisa dalam detik

#### `deploy_ore()`

```rust
pub async fn deploy_ore(rpc: &RpcClient, keypair_path: &str, amount: u64, square_id: Option<u64>) -> Result<String, ApiError> {
    // 1. Load keypair dari file
    // 2. Get board info
    // 3. Setup squares array
    // 4. Create deploy instruction
    // 5. Add compute budget
    // 6. Sign and send transaction
}
```

**deploy_ore()** melakukan deployment mining:
1. Load private key dari file
2. Tentukan square mana yang akan di-deploy
3. Buat instruksi deploy menggunakan ore_api
4. Set compute budget untuk fee
5. Sign dan kirim transaksi

## Error Handling

Aplikasi menggunakan custom error type `ApiError`:

```rust
pub enum ApiError {
    Database(sqlx::Error),
    Redis(redis::RedisError),
    Rpc(solana_client::client_error::ClientError),
    Json(serde_json::Error),
    NotFound,
    Unauthorized,
    BadRequest(String),
    Internal(String),
}
```

Error handling mencakup:
- Retry mechanism untuk RPC calls
- Exponential backoff untuk rate limiting
- Proper HTTP status codes
- Detailed error logging

## Sistem Caching dengan Redis

Aplikasi menggunakan Redis untuk cache data yang sering diakses:

```rust
pub async fn get_square_stats_cached(rpc: &RpcClient, redis: &mut redis::aio::ConnectionManager) -> Result<Vec<SquareStats>, ApiError> {
    // Try cache first
    let cache_key = "squares:current";
    if let Ok(Some(data)) = redis.get::<_, Option<String>>(cache_key).await {
        // Return cached data
    }
    // Fetch fresh data, cache for 30 seconds
}
```

**Keuntungan caching:**
- Mengurangi load ke RPC Solana
- Mempercepat response API
- Mengurangi biaya RPC calls

## Kesimpulan

File `main.rs` ini adalah backend lengkap untuk aplikasi mining ORE yang mencakup:

1. **Real-time Communication**: WebSocket untuk update live
2. **REST API**: Endpoint lengkap untuk semua operasi
3. **Blockchain Integration**: Interaksi penuh dengan Solana
4. **Database Management**: Persistent storage untuk data user
5. **Automated Trading**: Strategi Martingale untuk trading otomatis
6. **Caching**: Redis untuk performa optimal
7. **Error Handling**: Robust error handling dengan retry

Aplikasi ini memungkinkan pengguna untuk mining ORE secara manual atau otomatis, dengan monitoring real-time dan manajemen risiko.

### Untuk Developer

Jika Anda ingin mengembangkan aplikasi serupa:
- Pahami konsep Solana dan ORE mining
- Setup environment dengan Rust, PostgreSQL, Redis
- Configure Solana RPC endpoint dan wallet
- Test dengan devnet sebelum mainnet
- Implementasi proper error handling dan logging

### Untuk User

Jika Anda ingin menggunakan aplikasi ini:
- Setup wallet Solana
- Fund wallet dengan SOL untuk fee dan deployment
- Monitor profit/loss melalui API atau WebSocket
- Gunakan strategi Martingale dengan hati-hati (risiko tinggi)

---

*Dokumentasi ini dibuat untuk memahami kode main.rs secara mendalam. Untuk pertanyaan lebih lanjut, silakan tanyakan kepada developer atau tim teknis.*