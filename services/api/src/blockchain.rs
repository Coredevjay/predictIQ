use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::Utc;

use anyhow::{anyhow, Context};
use rand::Rng as _;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{sync::RwLock, time::sleep};

use crate::{
    cache::{keys, RedisCache},
    config::{Config, ContractKeySchema},
    db::Database,
    metrics::Metrics,
    shutdown::{ShutdownCoordinator, WorkerHandle},
};

#[derive(Clone)]
pub struct BlockchainClient {
    http: Client,
    rpc_url: String,
    network: String,
    contract_id: String,
    key_schema: ContractKeySchema,
    retry_attempts: u32,
    retry_base_delay_ms: u64,
    rpc_backoff_jitter_factor: f64,
    event_poll_interval: Duration,
    tx_poll_interval: Duration,
    confirmation_ledger_lag: u32,
    sync_market_ids: Vec<i64>,
    cache: RedisCache,
    db: Database,
    metrics: Metrics,
    monitor: Arc<MonitoringState>,
    expected_passphrase: String,
    /// TTL after which a watched-transaction entry is evicted.
    /// Populated from `Config::watched_tx_ttl_secs`.
    watched_tx_ttl: Duration,
    /// Hard cap on the number of entries in the watch map.
    /// Populated from `Config::watched_tx_max_size`.
    watched_tx_max_size: usize,
    /// Whether the service is running in a production environment.
    /// Affects startup passphrase-mismatch behaviour: hard exit vs. warning.
    is_production: bool,
}

/// TTL for watched transaction hashes. Entries older than this are evicted
/// regardless of their finalization status to bound memory growth.
/// This default is used only in tests; the runtime value comes from config.
const WATCHED_TX_TTL_DEFAULT: Duration = Duration::from_secs(30 * 60); // 30 minutes

/// Public alias for tests that need to reference the default TTL directly.
pub const WATCHED_TX_DEFAULT_TTL: Duration = WATCHED_TX_TTL_DEFAULT;

/// Maximum number of entries in `watched_txs` when no config value is provided.
/// The runtime cap comes from `Config::watched_tx_max_size`.
pub const WATCHED_TX_MAX_SIZE: usize = 10_000;

/// Errors that can be returned by [`BlockchainClient::watch_transaction`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchTxError {
    /// The transaction hash is already registered in the watch map.
    /// No second entry is inserted; the caller should treat the existing
    /// registration as authoritative.
    AlreadyWatched,
    /// The watch map has reached its configured capacity cap.
    /// The caller should back-off and retry later, or inform the client with
    /// a `503 Service Unavailable`.
    CapReached,
}

#[derive(Default)]
struct MonitoringState {
    /// Maps tx hash → time it was first watched. Evicted after `WATCHED_TX_TTL`.
    watched_txs: RwLock<HashMap<String, Instant>>,
}

/// Classifies RPC errors as either transient (worth retrying) or permanent.
///
/// Transient errors (network timeouts, 5xx, 429) may succeed if retried.
/// Permanent errors (4xx except 429, invalid params, out-of-range ledger)
/// will fail identically if retried — the client should receive a clear error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcErrorClass {
    /// Network-level or server-side failures (5xx, timeouts, 429).
    /// Retrying may succeed; callers should retry with backoff.
    Transient,
    /// Client-side mistakes or resource constraints that will not change
    /// (4xx except 429, invalid params, from_ledger outside retention window).
    /// Retrying will fail identically; callers should return error immediately.
    Permanent,
}

/// Indicates whether a response was sourced from a live RPC call or a stale
/// cache entry served after an RPC failure.
///
/// Consumers and alerting rules can use this field to distinguish real zeros
/// from error-masked defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    /// Data was fetched live from the RPC node.
    Live,
    /// The RPC call failed; this is a stale cached value served as a fallback.
    StaleFallback,
}

/// Error returned from blockchain operations that distinguishes transient vs permanent failures.
#[derive(Debug)]
pub enum BlockchainError {
    /// An RPC error with its classification (transient vs permanent).
    Rpc {
        message: String,
        class: RpcErrorClass,
    },
    /// Internal error not directly from RPC.
    Internal(String),
}

impl std::fmt::Display for BlockchainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rpc { message, class } => {
                write!(f, "RPC error ({:?}): {}", class, message)
            }
            Self::Internal(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

impl std::error::Error for BlockchainError {}

impl BlockchainError {
    /// Determine whether this error is worth retrying from its message.
    /// Transient errors include timeouts, 5xx, 429. Permanent errors include
    /// 4xx (except 429), invalid params, out-of-range ledger.
    fn classify_from_error_message(msg: &str) -> RpcErrorClass {
        // HTTP client errors (4xx) except 429 are permanent.
        if msg.contains("client error:") && !msg.contains("429") {
            return RpcErrorClass::Permanent;
        }
        // JSON-RPC invalid param errors are permanent.
        if msg.contains("Invalid params") || msg.contains("invalid_params")
            || msg.contains("Parse error") || msg.contains("Invalid request")
            || msg.contains("Method not found") {
            return RpcErrorClass::Permanent;
        }
        // Ledger out of retention window is permanent.
        if msg.contains("outside") && msg.contains("window")
            || msg.contains("out of range") {
            return RpcErrorClass::Permanent;
        }
        // Everything else (5xx, timeouts, network errors, 429, etc.) is transient.
        RpcErrorClass::Transient
    }

    pub fn class(&self) -> RpcErrorClass {
        match self {
            Self::Rpc { class, .. } => *class,
            Self::Internal(_) => RpcErrorClass::Transient,
        }
    }

    /// Convert an anyhow error to BlockchainError with automatic classification.
    pub fn from_anyhow(err: anyhow::Error) -> Self {
        let msg = err.to_string();
        let class = Self::classify_from_error_message(&msg);
        Self::Rpc {
            message: msg,
            class,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainMarketData {
    pub market_id: i64,
    pub title: Option<String>,
    pub status: Option<String>,
    pub onchain_volume: String,
    pub resolved_outcome: Option<u32>,
    pub ledger: u32,
    pub source: DataSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformStatistics {
    pub total_markets: u64,
    pub active_markets: u64,
    pub resolved_markets: u64,
    pub total_volume: String,
    pub ledger: u32,
    pub source: DataSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserBet {
    pub market_id: i64,
    pub outcome: u32,
    pub amount: String,
    pub token: Option<String>,
    pub ledger: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserBetsPage {
    pub user: String,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub items: Vec<UserBet>,
    pub source: DataSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleResult {
    pub market_id: i64,
    pub source_name: Option<String>,
    pub outcome: Option<u32>,
    pub confidence_bps: Option<u64>,
    pub ledger: u32,
    pub source: DataSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionStatus {
    pub hash: String,
    pub status: String,
    pub ledger: Option<u32>,
    pub error: Option<String>,
    pub source: DataSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockchainHealth {
    pub network: String,
    pub rpc_url: String,
    pub latest_ledger: u32,
    pub is_healthy: bool,
    pub contract_reachable: bool,
    pub checked_at_unix: u64,
    pub status: HealthStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractEvent {
    pub id: String,
    pub ledger: u32,
    pub topic: String,
    pub tx_hash: Option<String>,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRequest {
    pub from_ledger: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayProgress {
    pub from_ledger: u32,
    pub events_replayed: usize,
    /// Highest ledger of any stored event (or `from_ledger` if none were stored).
    #[serde(default)]
    pub last_ledger: u32,
    pub completed: bool,
}

#[derive(Debug, Deserialize)]
struct RpcEnvelope<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

/// Standard JSON-RPC error codes that indicate a client-side mistake and
/// should never be retried (retrying will produce the same error).
fn is_non_retryable_rpc_error(code: i64) -> bool {
    matches!(
        code,
        -32700  // Parse error
        | -32600  // Invalid request
        | -32601  // Method not found
        | -32602  // Invalid params
    )
}

impl BlockchainClient {
    pub fn new(config: &Config, cache: RedisCache, db: Database, metrics: Metrics) -> anyhow::Result<Self> {
        let http = Client::builder()
            .pool_max_idle_per_host(16)
            .pool_idle_timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .context("failed to construct RPC http client")?;

        // ── Startup schema validation ─────────────────────────────────────────
        // Validate the key schema eagerly so template drift is caught before
        // any contract reads are attempted.  A validation failure is logged as
        // an error but does not abort startup — the operator can correct the
        // env vars and redeploy without a full restart cycle.
        let key_schema = config.contract_key_schema.clone();
        match key_schema.validate() {
            Ok(()) => tracing::info!(
                schema_version = %key_schema.version,
                market = %key_schema.market,
                platform_stats = %key_schema.platform_stats,
                user_bets = %key_schema.user_bets,
                oracle_result = %key_schema.oracle_result,
                health_check = %key_schema.health_check,
                "Contract key schema loaded and validated"
            ),
            Err(e) => tracing::error!(
                schema_version = %key_schema.version,
                error = %e,
                "Contract key schema validation FAILED — contract reads may use wrong keys"
            ),
        }

        Ok(Self {
            http,
            rpc_url: config.blockchain_rpc_url.clone(),
            network: config.network_name().to_string(),
            contract_id: config.contract_id.clone(),
            key_schema,
            retry_attempts: config.retry_attempts.max(1),
            retry_base_delay_ms: config.retry_base_delay_ms.max(50),
            rpc_backoff_jitter_factor: config.rpc_backoff_jitter_factor,
            event_poll_interval: config.event_poll_interval,
            tx_poll_interval: config.tx_poll_interval,
            confirmation_ledger_lag: config.confirmation_ledger_lag.max(1),
            sync_market_ids: config.sync_market_ids.clone(),
            cache,
            db,
            metrics,
            monitor: Arc::new(MonitoringState::default()),
            expected_passphrase: config.network_passphrase.clone(),
            watched_tx_ttl: Duration::from_secs(config.watched_tx_ttl_secs),
            watched_tx_max_size: config.watched_tx_max_size,
            is_production: config.is_production,
        })
    }

    /// Probe whether `BLOCKCHAIN_RPC_URL` is reachable within a 5-second window.
    ///
    /// Unlike `validate_network_passphrase`, this probe:
    /// - Uses a hard 5-second timeout (no retry backoff)
    /// - Succeeds on any HTTP response, including 4xx/5xx (server is up)
    /// - Only fails on network-level errors (refused, timeout, DNS failure)
    ///
    /// In production (`APP_ENV=production`) callers should treat a non-Ok
    /// result as fatal and refuse to start.  In other environments it is
    /// logged as WARN so developers can work offline.
    pub async fn probe_rpc_reachability(&self) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "startup-probe",
            "method": "getHealth",
            "params": {}
        });
        tokio::time::timeout(
            Duration::from_secs(5),
            self.http.post(&self.rpc_url).json(&payload).send(),
        )
        .await
        .context("RPC probe timed out after 5 seconds")?
        .with_context(|| format!("RPC probe to {} failed", self.rpc_url))?;
        Ok(())
    }

    /// Query the RPC node for its network passphrase and verify it matches
    /// the configured `STELLAR_NETWORK_PASSPHRASE`. Startup must call this and
    /// fail fast if the passphrase does not match, preventing silently signed
    /// transactions for the wrong network.
    ///
    /// When `STELLAR_NETWORK_PASSPHRASE` is unset (empty string, e.g. for a
    /// custom network without a known passphrase), validation is skipped.
    ///
    /// In production (`PREDICTIQ_ENV=production`) a passphrase mismatch causes
    /// `process::exit(1)`.  In all other environments a warning is logged and
    /// the process continues, so developers aren't blocked by a misconfigured
    /// RPC endpoint.
    pub async fn validate_network_passphrase(&self) -> anyhow::Result<()> {
        if self.expected_passphrase.is_empty() {
            tracing::info!("STELLAR_NETWORK_PASSPHRASE not set; skipping passphrase validation");
            return Ok(());
        }

        #[derive(Debug, Deserialize)]
        struct NetworkResult {
            passphrase: String,
        }

        let result = self
            .rpc_call::<NetworkResult>("getNetwork", serde_json::json!({}))
            .await;

        let result = match result {
            Ok(r) => r,
            Err(e) => {
                let msg = format!(
                    "Stellar RPC reachability probe failed — could not call getNetwork: {e}. \
                     Check BLOCKCHAIN_RPC_URL."
                );
                if self.is_production {
                    tracing::error!("{msg}");
                    std::process::exit(1);
                } else {
                    tracing::warn!("{msg}");
                    return Ok(());
                }
            }
        };

        if result.passphrase != self.expected_passphrase {
            let msg = format!(
                "Stellar network passphrase mismatch — \
                 RPC returned {:?} but STELLAR_NETWORK_PASSPHRASE is {:?}. \
                 Check BLOCKCHAIN_NETWORK and STELLAR_NETWORK_PASSPHRASE.",
                result.passphrase, self.expected_passphrase,
            );
            if self.is_production {
                tracing::error!("{msg}");
                std::process::exit(1);
            } else {
                tracing::warn!("{msg}");
                return Ok(());
            }
        }

        tracing::info!(
            passphrase = %result.passphrase,
            "Stellar network passphrase validated"
        );
        Ok(())
    }

    /// Returns the result of the last Stellar RPC reachability probe as a
    /// simple boolean suitable for embedding in `/health/ready` responses.
    /// This performs a live RPC call to `getNetwork` and checks the passphrase.
    pub async fn probe_stellar_ready(&self) -> bool {
        if self.expected_passphrase.is_empty() {
            // Custom network with no passphrase configured — skip check.
            return true;
        }

        #[derive(Debug, Deserialize)]
        struct NetworkResult {
            passphrase: String,
        }

        match self
            .rpc_call::<NetworkResult>("getNetwork", serde_json::json!({}))
            .await
        {
            Ok(r) => r.passphrase == self.expected_passphrase,
            Err(e) => {
                tracing::warn!(error = %e, "probe_stellar_ready: getNetwork failed");
                false
            }
        }
    }

    async fn rpc_call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: Value,
    ) -> anyhow::Result<T> {
        let mut attempt: u32 = 0;

        loop {
            attempt += 1;

            let payload = json!({
                "jsonrpc": "2.0",
                "id": format!("{}-{}", method, attempt),
                "method": method,
                "params": params,
            });

            let response = self.http.post(&self.rpc_url).json(&payload).send().await;

            match response {
                Ok(resp) => {
                    let status = resp.status();

                    // 4xx (except 429 Too Many Requests) are non-retryable client errors.
                    if status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS {
                        return Err(anyhow!(
                            "rpc {} non-retryable client error: {}",
                            method, status
                        ));
                    }

                    if !status.is_success() {
                        // 5xx / 429 are transient — retry with backoff.
                        if attempt >= self.retry_attempts {
                            return Err(anyhow!(
                                "rpc {} http error after {} attempt(s): {}",
                                method, attempt, status
                            ));
                        }
                        tracing::warn!(
                            method, attempt, %status,
                            "rpc http error, retrying"
                        );
                    } else {
                        let parsed = resp
                            .json::<RpcEnvelope<T>>()
                            .await
                            .context("rpc parse error")?;

                        if let Some(err) = parsed.error {
                            if is_non_retryable_rpc_error(err.code) {
                                return Err(anyhow!(
                                    "rpc {} non-retryable error: {} ({})",
                                    method, err.message, err.code
                                ));
                            }
                            if attempt >= self.retry_attempts {
                                return Err(anyhow!(
                                    "rpc {} failed: {} ({})",
                                    method, err.message, err.code
                                ));
                            }
                            tracing::warn!(
                                method, attempt, code = err.code,
                                message = %err.message, "rpc error, retrying"
                            );
                        } else if let Some(result) = parsed.result {
                            return Ok(result);
                        } else if attempt >= self.retry_attempts {
                            return Err(anyhow!("rpc {} returned empty result", method));
                        } else {
                            tracing::warn!(method, attempt, "rpc empty result, retrying");
                        }
                    }
                }
                Err(err) => {
                    if attempt >= self.retry_attempts {
                        return Err(anyhow!("rpc {} transport failed: {err}", method));
                    }
                    tracing::warn!(method, attempt, error = %err, "rpc transport error, retrying");
                }
            }

            // Full-jitter backoff: delay = random(0, min(cap, base * 2^attempt))
            // RPC_BACKOFF_JITTER_FACTOR controls the jitter fraction (1.0 = full).
            let cap_ms = (self.retry_base_delay_ms * (1u64 << (attempt - 1).min(10))).min(60_000);
            let backoff_ms = if self.rpc_backoff_jitter_factor > 0.0 {
                let jitter_range = (cap_ms as f64 * self.rpc_backoff_jitter_factor) as u64;
                let deterministic = cap_ms.saturating_sub(jitter_range);
                deterministic + rand::thread_rng().gen_range(0..=jitter_range)
            } else {
                cap_ms
            };
            tracing::warn!(method, attempt, backoff_ms, "rpc retry scheduled");
            sleep(Duration::from_millis(backoff_ms)).await;
        }
    }

    async fn latest_ledger(&self) -> anyhow::Result<u32> {
        #[derive(Debug, Deserialize)]
        struct LatestLedgerResult {
            sequence: u32,
        }

        #[derive(Debug, Deserialize)]
        struct GetLatestLedgerResult {
            #[serde(rename = "latestLedger")]
            latest_ledger: LatestLedgerResult,
        }

        let result: GetLatestLedgerResult = self.rpc_call("getLatestLedger", json!({})).await?;
        Ok(result.latest_ledger.sequence)
    }

    pub async fn market_data_cached(&self, market_id: i64) -> anyhow::Result<ChainMarketData> {
        let key = keys::chain_market(market_id);
        let ttl = Duration::from_secs(60);
        let endpoint = "market_data";

        let (value, hit) = self
            .cache
            .get_or_set_json(&key, ttl, || async move {
                let ledger = self.latest_ledger().await.unwrap_or(0);
                match self
                    .rpc_call::<Value>(
                        "getContractData",
                        json!({
                            "contractId": self.contract_id,
                            "key": self.key_schema.market_key(market_id),
                        }),
                    )
                    .await
                {
                    Ok(data) => Ok(ChainMarketData {
                        market_id,
                        title: data.get("title").and_then(Value::as_str).map(ToOwned::to_owned),
                        status: data.get("status").and_then(Value::as_str).map(ToOwned::to_owned),
                        onchain_volume: data
                            .get("onchain_volume")
                            .and_then(Value::as_str)
                            .unwrap_or("0")
                            .to_string(),
                        resolved_outcome: data
                            .get("resolved_outcome")
                            .and_then(Value::as_u64)
                            .map(|v| v as u32),
                        ledger,
                        source: DataSource::Live,
                    }),
                    Err(e) => {
                        self.metrics.observe_rpc_error("getContractData");
                        self.metrics.observe_rpc_fallback(endpoint);
                        tracing::warn!(market_id, error = %e, "market_data RPC failed");
                        Err(e)
                    }
                }
            })
            .await?;

        if hit {
            self.metrics.observe_hit("chain", endpoint);
        } else {
            self.metrics.observe_miss("chain", endpoint);
        }

        Ok(value)
    }

    pub async fn platform_statistics_cached(&self) -> anyhow::Result<PlatformStatistics> {
        let key = keys::chain_platform_stats(&self.network);
        let ttl = Duration::from_secs(120);
        let endpoint = "platform_stats";

        let (value, hit) = self
            .cache
            .get_or_set_json(&key, ttl, || async move {
                let ledger = self.latest_ledger().await.unwrap_or(0);
                match self
                    .rpc_call::<Value>(
                        "getContractData",
                        json!({
                            "contractId": self.contract_id,
                            "key": self.key_schema.platform_stats.clone(),
                        }),
                    )
                    .await
                {
                    Ok(data) => Ok(PlatformStatistics {
                        total_markets: data.get("total_markets").and_then(Value::as_u64).unwrap_or(0),
                        active_markets: data.get("active_markets").and_then(Value::as_u64).unwrap_or(0),
                        resolved_markets: data.get("resolved_markets").and_then(Value::as_u64).unwrap_or(0),
                        total_volume: data
                            .get("total_volume")
                            .and_then(Value::as_str)
                            .unwrap_or("0")
                            .to_string(),
                        ledger,
                        source: DataSource::Live,
                    }),
                    Err(e) => {
                        self.metrics.observe_rpc_error("getContractData");
                        self.metrics.observe_rpc_fallback(endpoint);
                        tracing::warn!(error = %e, "platform_statistics RPC failed");
                        Err(e)
                    }
                }
            })
            .await?;

        if hit {
            self.metrics.observe_hit("chain", endpoint);
        } else {
            self.metrics.observe_miss("chain", endpoint);
        }

        Ok(value)
    }

    pub async fn user_bets_page(
        &self,
        user: &str,
        page: i64,
        page_size: i64,
    ) -> anyhow::Result<UserBetsPage> {
        let page = page.max(0);
        let page_size = page_size.clamp(1, 100);
        let offset = page * page_size;

        let key = keys::chain_user_bets_page(&self.network, user, page, page_size);
        let ttl = Duration::from_secs(30);
        let endpoint = "user_bets";

        let (value, hit) = self
            .cache
            .get_or_set_json(&key, ttl, || async move {
                let ledger = self.latest_ledger().await.unwrap_or(0);
                match self
                    .rpc_call::<Value>(
                        "getContractData",
                        json!({
                            "contractId": self.contract_id,
                            "key": self.key_schema.user_bets_key(user),
                            "limit": page_size,
                            "offset": offset,
                        }),
                    )
                    .await
                {
                    Ok(data) => {
                        let bets = data
                            .get("bets")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let total = data
                            .get("total")
                            .and_then(Value::as_i64)
                            .unwrap_or(bets.len() as i64);
                        let items = bets
                            .into_iter()
                            .map(|entry| UserBet {
                                market_id: entry.get("market_id").and_then(Value::as_i64).unwrap_or_default(),
                                outcome: entry.get("outcome").and_then(Value::as_u64).unwrap_or_default() as u32,
                                amount: entry.get("amount").and_then(Value::as_str).unwrap_or("0").to_string(),
                                token: entry.get("token").and_then(Value::as_str).map(ToOwned::to_owned),
                                ledger,
                            })
                            .collect::<Vec<_>>();
                        Ok(UserBetsPage {
                            user: user.to_string(),
                            page,
                            page_size,
                            total,
                            items,
                            source: DataSource::Live,
                        })
                    }
                    Err(e) => {
                        self.metrics.observe_rpc_error("getContractData");
                        self.metrics.observe_rpc_fallback(endpoint);
                        tracing::warn!(user, error = %e, "user_bets RPC failed");
                        Err(e)
                    }
                }
            })
            .await?;

        if hit {
            self.metrics.observe_hit("chain", endpoint);
        } else {
            self.metrics.observe_miss("chain", endpoint);
        }

        Ok(value)
    }

    pub async fn oracle_result_cached(&self, market_id: i64) -> anyhow::Result<OracleResult> {
        let key = keys::chain_oracle_result(&self.network, market_id);
        let ttl = Duration::from_secs(30);
        let endpoint = "oracle_result";

        let (value, hit) = self
            .cache
            .get_or_set_json(&key, ttl, || async move {
                let ledger = self.latest_ledger().await.unwrap_or(0);
                match self
                    .rpc_call::<Value>(
                        "getContractData",
                        json!({
                            "contractId": self.contract_id,
                            "key": self.key_schema.oracle_result_key(market_id),
                        }),
                    )
                    .await
                {
                    Ok(data) => Ok(OracleResult {
                        market_id,
                        source_name: data.get("source").and_then(Value::as_str).map(ToOwned::to_owned),
                        outcome: data.get("outcome").and_then(Value::as_u64).map(|v| v as u32),
                        confidence_bps: data.get("confidence_bps").and_then(Value::as_u64),
                        ledger,
                        source: DataSource::Live,
                    }),
                    Err(e) => {
                        self.metrics.observe_rpc_error("getContractData");
                        self.metrics.observe_rpc_fallback(endpoint);
                        tracing::warn!(market_id, error = %e, "oracle_result RPC failed");
                        Err(e)
                    }
                }
            })
            .await?;

        if hit {
            self.metrics.observe_hit("chain", endpoint);
        } else {
            self.metrics.observe_miss("chain", endpoint);
        }

        Ok(value)
    }

    pub async fn transaction_status_cached(&self, hash: &str) -> anyhow::Result<TransactionStatus> {
        let key = keys::chain_tx_status(&self.network, hash);
        let ttl = Duration::from_secs(20);
        let endpoint = "tx_status";

        let (value, hit) = self
            .cache
            .get_or_set_json(&key, ttl, || async move {
                #[derive(Debug, Deserialize)]
                struct TxResponse {
                    status: String,
                    #[serde(rename = "ledger")]
                    ledger: Option<u32>,
                    #[serde(rename = "errorResultXdr")]
                    error_result_xdr: Option<String>,
                }

                match self
                    .rpc_call::<TxResponse>("getTransaction", json!({ "hash": hash }))
                    .await
                {
                    Ok(tx) => Ok(TransactionStatus {
                        hash: hash.to_string(),
                        status: tx.status,
                        ledger: tx.ledger,
                        error: tx.error_result_xdr,
                        source: DataSource::Live,
                    }),
                    Err(e) => {
                        self.metrics.observe_rpc_error("getTransaction");
                        self.metrics.observe_rpc_fallback(endpoint);
                        tracing::warn!(hash, error = %e, "transaction_status RPC failed");
                        Err(e)
                    }
                }
            })
            .await?;

        if hit {
            self.metrics.observe_hit("chain", endpoint);
        } else {
            self.metrics.observe_miss("chain", endpoint);
        }

        Ok(value)
    }

    pub async fn health_check_cached(&self) -> anyhow::Result<BlockchainHealth> {
        let key = keys::chain_health(&self.network);
        let ttl = Duration::from_secs(15);
        let endpoint = "health";

        let (value, hit) = self
            .cache
            .get_or_set_json(&key, ttl, || async move {
                let latest = self.latest_ledger().await.unwrap_or_else(|e| {
                    self.metrics.observe_rpc_error("getLatestLedger");
                    tracing::warn!(error = %e, "health_check: getLatestLedger failed");
                    0
                });
                let contract_reachable = match self
                    .rpc_call::<Value>(
                        "getContractData",
                        json!({
                            "contractId": self.contract_id,
                            "key": self.key_schema.health_check.clone(),
                        }),
                    )
                    .await
                {
                    Ok(_) => true,
                    Err(e) => {
                        self.metrics.observe_rpc_error("getContractData");
                        tracing::warn!(error = %e, "health_check: contract probe failed");
                        false
                    }
                };

                let checked_at_unix = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let status = if latest > 0 && contract_reachable {
                    HealthStatus::Healthy
                } else if latest > 0 {
                    // Node is reachable but contract read failed — degraded, not healthy.
                    HealthStatus::Degraded
                } else {
                    HealthStatus::Unhealthy
                };

                Ok(BlockchainHealth {
                    network: self.network.clone(),
                    rpc_url: self.rpc_url.clone(),
                    latest_ledger: latest,
                    // is_healthy is true only when both node AND contract are reachable.
                    is_healthy: status == HealthStatus::Healthy,
                    contract_reachable,
                    checked_at_unix,
                    status,
                })
            })
            .await?;

        if hit {
            self.metrics.observe_hit("chain", endpoint);
        } else {
            self.metrics.observe_miss("chain", endpoint);
        }

        Ok(value)
    }

    async fn fetch_events_since(&self, from_ledger: u32) -> anyhow::Result<Vec<ContractEvent>> {
        #[derive(Debug, Deserialize)]
        struct EventsResponse {
            events: Vec<Value>,
            #[serde(rename = "latestLedger")]
            latest_ledger: Option<u32>,
        }

        let mut all_events: Vec<ContractEvent> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages: u64 = 0;

        loop {
            let mut params = json!({
                "startLedger": from_ledger,
                "filters": [{"type": "contract", "contractIds": [self.contract_id]}],
                "limit": 100,
            });
            if let Some(ref c) = cursor {
                params["cursor"] = json!(c);
            }

            let result = self
                .rpc_call::<EventsResponse>("getEvents", params)
                .await
                .map_err(|e| {
                    self.metrics.observe_rpc_error("getEvents");
                    tracing::warn!(from_ledger, error = %e, "getEvents RPC failed");
                    e
                })?;

            pages += 1;
            let batch_len = result.events.len();
            let last_id = result.events.last()
                .and_then(|e| e.get("id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);

            for e in result.events {
                all_events.push(ContractEvent {
                    id: e.get
