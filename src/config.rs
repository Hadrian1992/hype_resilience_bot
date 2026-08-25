use serde::{Deserialize, Serialize};
use std::{env, error::Error, fs, path::Path};

fn default_tokenomist_poll_secs() -> u64 {
    900
}
fn default_ws_fallback_after_failures() -> u32 {
    5
}
fn default_signal_horizon_secs() -> u64 {
    1800
}
fn default_signal_validate_drop_pct() -> f64 {
    0.005
}

/// Konfiguracja pojedynczego aktywa/monety
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetConfig {
    pub symbol: String,            // "HYPE", "BTC"
    pub vesting_contract: String,  // adres kontraktu vestingu (0x...)
    pub threshold_usd: f64,        // wartość progowa w USD (np. alert jeśli unlock > threshold)
}

/// Konfiguracja Telegram (opcjonalnie)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: Option<String>,
    pub chat_id: Option<String>,
}

/// Główna konfiguracja bota
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotConfig {
    // Endpoints
    pub rpc_url: String,           // Alchemy / QuickNode HTTP RPC
    pub grpc_url: String,          // QuickNode gRPC / StreamL2Book
    // Token/addresses
    pub hype_token_address: String,
    // Śledzone aktywa (wielomonetowość)
    pub tracked_assets: Vec<AssetConfig>,
    // Progi i parametry działania
    pub threshold_hype: f64,       // domyślny threshold HYPE dla claim_rate
    pub window_hours: u64,         // okno czasu dla agregacji (np. 6)
    pub poll_interval_seconds: u64,// poll interval dla eth_getLogs
    // Telegram
    pub telegram: Option<TelegramConfig>,
    // Intelligence / observability
    #[serde(default)]
    pub tokenomist_url: Option<String>,
    #[serde(default = "default_tokenomist_poll_secs")]
    pub tokenomist_poll_secs: u64,
    #[serde(default)]
    pub ws_fallback_url: Option<String>,
    #[serde(default = "default_ws_fallback_after_failures")]
    pub ws_fallback_after_failures: u32,
    #[serde(default = "default_signal_horizon_secs")]
    pub signal_horizon_secs: u64,
    #[serde(default = "default_signal_validate_drop_pct")]
    pub signal_validate_drop_pct: f64,
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            rpc_url: "https://your-alchemy-or-quicknode-rpc/".to_string(),
            grpc_url: "wss://api.hyperliquid.xyz/ws".to_string(),
            hype_token_address: "0x0000000000000000000000000000000000000000".to_string(),
            tracked_assets: Vec::new(),
            threshold_hype: 100_000.0,
            window_hours: 6,
            poll_interval_seconds: 60,
            telegram: None,
            tokenomist_url: None,
            tokenomist_poll_secs: 900,
            ws_fallback_url: None,
            ws_fallback_after_failures: 5,
            signal_horizon_secs: 1800,
            signal_validate_drop_pct: 0.005,
        }
    }
}

impl BotConfig {
    /// Ładuje konfigurację z opcjonalnego pliku JSON (ścieżka), nadpisuje z ENV, waliduje.
    pub fn load_from_file(path: Option<&str>) -> Result<Self, Box<dyn Error>> {
        let mut cfg = if let Some(p) = path {
            if Path::new(p).exists() {
                let s = fs::read_to_string(p)?;
                serde_json::from_str::<BotConfig>(&s)?
            } else {
                BotConfig::default()
            }
        } else {
            // domyślnie szukamy ./config.json
            if Path::new("config.json").exists() {
                let s = fs::read_to_string("config.json")?;
                serde_json::from_str::<BotConfig>(&s)?
            } else {
                BotConfig::default()
            }
        };

        // env overrides (jeśli ustawione)
        cfg.apply_env_overrides();

        // walidacja podstawowych pól
        cfg.validate()?;

        Ok(cfg)
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(v) = env::var("RPC_URL") {
            self.rpc_url = v;
        }
        if let Ok(v) = env::var("GRPC_URL") {
            self.grpc_url = v;
        }
        if let Ok(v) = env::var("HYPE_TOKEN_ADDRESS") {
            self.hype_token_address = v;
        }
        if let Ok(v) = env::var("THRESHOLD_HYPE") {
            if let Ok(parsed) = v.parse::<f64>() {
                self.threshold_hype = parsed;
            }
        }
        if let Ok(v) = env::var("WINDOW_HOURS") {
            if let Ok(parsed) = v.parse::<u64>() {
                self.window_hours = parsed;
            }
        }
        if let Ok(v) = env::var("POLL_INTERVAL_SECONDS") {
            if let Ok(parsed) = v.parse::<u64>() {
                self.poll_interval_seconds = parsed;
            }
        }
        // Telegram vars (opcjonalne; puste stringi traktujemy jak brak)
        let bot = env::var("TELEGRAM_BOT_TOKEN")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let chat = env::var("TELEGRAM_CHAT_ID")
            .ok()
            .filter(|v| !v.trim().is_empty());
        if bot.is_some() || chat.is_some() {
            self.telegram = Some(TelegramConfig {
                bot_token: bot,
                chat_id: chat,
            });
        }

        // Intelligence / observability overrides
        if let Ok(v) = env::var("TOKENOMIST_URL") {
            self.tokenomist_url = if v.trim().is_empty() { None } else { Some(v) };
        }
        if let Ok(v) = env::var("WS_FALLBACK_URL") {
            self.ws_fallback_url = if v.trim().is_empty() { None } else { Some(v) };
        }
    }

    fn validate(&self) -> Result<(), Box<dyn Error>> {
        // prosta walidacja adresów ETH (0x + 40 hex) — nie zależymy tu od zewn. crate
        fn looks_like_eth(addr: &str) -> bool {
            let s = addr.strip_prefix("0x").unwrap_or(addr);
            s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
        }

        if !looks_like_eth(&self.hype_token_address) {
            return Err(format!("Invalid HYPE token address: {}", self.hype_token_address).into());
        }
        for a in &self.tracked_assets {
            if !looks_like_eth(&a.vesting_contract) {
                return Err(format!("Invalid vesting_contract for {}: {}", a.symbol, a.vesting_contract).into());
            }
            if a.symbol.trim().is_empty() {
                return Err(format!("Empty symbol in tracked_assets").into());
            }
        }
        if self.rpc_url.trim().is_empty() {
            return Err("rpc_url cannot be empty".into());
        }
        if self.grpc_url.trim().is_empty() {
            return Err("grpc_url cannot be empty".into());
        }
        Ok(())
    }
}
