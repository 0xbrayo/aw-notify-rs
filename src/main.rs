//! Simplified aw-notify-rs implementation matching Python version structure
//!
//! This is a complete rewrite that consolidates the functionality into a single file
//! similar to the Python version while maintaining Rust's safety and performance benefits.

use anyhow::{anyhow, Result};
use aw_client_rust::classes::{default_classes, CategoryId, CategorySpec, ClassSetting};
use aw_client_rust::queries::{DesktopQueryParams, QueryParams, QueryParamsBase};
use aw_models::TimeInterval;
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Timelike, Utc};
use clap::Parser;
use crossbeam_channel::{bounded, Receiver};
use dashmap::DashMap;
use hostname::get as get_hostname;
use notify_rust::Notification;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering as cmpOrdering;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time;

static AW_CLIENT: OnceLock<aw_client_rust::blocking::AwClient> = OnceLock::new();
static HOSTNAME: OnceLock<String> = OnceLock::new();
static SERVER_AVAILABLE: AtomicBool = AtomicBool::new(true);
static OUTPUT_ONLY: AtomicBool = AtomicBool::new(false);

type CacheValue = (DateTime<Utc>, HashMap<String, f64>);
static TIME_CACHE: Lazy<DashMap<String, CacheValue>> = Lazy::new(DashMap::new);

// Constants (matching Python exactly)
const TIME_OFFSET: Duration = Duration::hours(4);
const CACHE_TTL_SECONDS: i64 = 60;

/// Category aggregation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryAggregation {
    /// No aggregation - return full category hierarchy (e.g., "Work > Programming > Rust")
    None,
    /// Aggregate to top-level categories only (e.g., "Work")
    TopLevelOnly,
    /// Aggregate by all levels - includes both parent and leaf categories
    /// (e.g., "Work", "Work > Programming", "Work > Programming > Rust")
    AllLevels,
}

// Configuration structures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    pub category: String,
    pub label: Option<String>,
    pub thresholds_minutes: Vec<u64>,
    pub positive: bool,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            category: "All".to_string(),
            label: None,
            thresholds_minutes: vec![60, 120, 240, 360, 480], // 1h, 2h, 4h, 6h, 8h
            positive: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationConfig {
    pub alerts: Vec<AlertConfig>,
    pub hourly_checkins: bool,
    pub new_day_greetings: bool,
    pub server_monitoring: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            alerts: vec![
                AlertConfig {
                    category: "All".to_string(),
                    label: Some("All".to_string()),
                    thresholds_minutes: vec![60, 120, 240, 360, 480], // 1h, 2h, 4h, 6h, 8h
                    positive: false,
                },
                AlertConfig {
                    category: "Media > Social Media".to_string(),
                    label: Some("🐦 Social Media".to_string()),
                    thresholds_minutes: vec![15, 30, 60], // 15min, 30min, 1h
                    positive: false,
                },
                AlertConfig {
                    category: "Media".to_string(),
                    label: Some("📺 Media".to_string()),
                    thresholds_minutes: vec![30, 60, 120, 240], // 30min, 1h, 2h, 4h
                    positive: false,
                },
                AlertConfig {
                    category: "Work".to_string(),
                    label: Some("💼 Work".to_string()),
                    thresholds_minutes: vec![15, 30, 60, 120, 240], // 15min, 30min, 1h, 2h, 4h
                    positive: true,
                },
            ],
            hourly_checkins: true,
            new_day_greetings: true,
            server_monitoring: true,
        }
    }
}

#[derive(Parser)]
#[clap(
    name = "aw-notify",
    about = "ActivityWatch notification service",
    long_about = "ActivityWatch notification service\n\nProvides desktop notifications for time tracking data from ActivityWatch.\nUse --output-only to print notifications to stdout instead of showing desktop notifications (useful for scripting or integration with other tools(aw-tauri)).",
    version
)]
struct Cli {
    #[clap(short, long, help = "Verbose logging")]
    verbose: bool,

    #[clap(long, help = "Testing mode (port 5666)")]
    testing: bool,

    #[clap(long, help = "Port to connect to ActivityWatch server")]
    port: Option<u16>,

    #[clap(
        long,
        help = "Output only mode - print notification content to stdout instead of showing desktop notifications"
    )]
    output_only: bool,

    #[clap(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    #[clap(
        about = "Start the notification service (use --output-only to print notifications instead of showing them)"
    )]
    Start,
    #[clap(
        about = "Send a summary notification (use --output-only to print to stdout instead of showing notification)"
    )]
    Checkin {
        #[clap(long, help = "Testing mode")]
        testing: bool,
    },
    #[clap(
        about = "Send a detailed summary with all category levels (parent and leaf categories)"
    )]
    CheckinDetailed {
        #[clap(long, help = "Testing mode")]
        testing: bool,
    },
}

// Configuration loading functions
fn get_default_config_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("aw-notify");
    path.push("config.toml");
    path
}

fn load_config() -> Result<NotificationConfig> {
    let config_path = get_default_config_path();

    if !config_path.exists() {
        log::info!(
            "Config file not found at {:?}, creating default configuration",
            config_path
        );

        // Create parent directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| anyhow!("Failed to create config directory {:?}: {}", parent, e))?;
        }

        let default_config = NotificationConfig::default();
        let config_content = toml::to_string_pretty(&default_config)
            .map_err(|e| anyhow!("Failed to serialize default config: {}", e))?;

        fs::write(&config_path, config_content)
            .map_err(|e| anyhow!("Failed to write config file {:?}: {}", config_path, e))?;

        log::info!("Default configuration saved to {:?}", config_path);
        return Ok(default_config);
    }

    let config_content = fs::read_to_string(&config_path)
        .map_err(|e| anyhow!("Failed to read config file {:?}: {}", config_path, e))?;

    let config: NotificationConfig = toml::from_str(&config_content)
        .map_err(|e| anyhow!("Failed to parse config file {:?}: {}", config_path, e))?;

    log::info!("Loaded configuration from {:?}", config_path);
    Ok(config)
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level)).init();

    // Suppress urllib3 equivalent (reqwest) warnings like Python
    log::set_max_level(if cli.verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    });

    log::info!("Starting...");

    // Set global output-only flag
    OUTPUT_ONLY.store(cli.output_only, Ordering::Relaxed);

    // Handle commands (matching Python's main function logic)
    match cli.command.unwrap_or(Commands::Start) {
        Commands::Start => {
            // Load configuration
            let config = load_config()?;

            // Initialize client (matching Python's start function)
            let port = cli.port.unwrap_or(if cli.testing { 5666 } else { 5600 });
            let host = "127.0.0.1";
            let client = match aw_client_rust::blocking::AwClient::new(host, port, "aw-notify") {
                Ok(client) => client,
                Err(e) => return Err(anyhow!("Failed to create client: {}", e)),
            };

            // Wait for server to be ready (like Python's wait_for_start)
            client.get_info()?;

            let hostname = get_hostname()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            // Set global state
            AW_CLIENT.set(client).ok();
            HOSTNAME.set(hostname.clone()).ok();

            start_service(hostname, config)
        }
        Commands::Checkin { testing } => {
            // Initialize client for checkin (matching Python's checkin function)
            let port = cli.port.unwrap_or(if testing { 5666 } else { 5600 });
            let host = "127.0.0.1";
            let client =
                match aw_client_rust::blocking::AwClient::new(host, port, "aw-notify-checkin") {
                    Ok(client) => client,
                    Err(e) => return Err(anyhow!("Failed to create client: {}", e)),
                };

            let hostname = get_hostname()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            // Set global state
            AW_CLIENT.set(client).ok();
            HOSTNAME.set(hostname).ok();

            send_checkin("Time today", None)?;
            Ok(())
        }
        Commands::CheckinDetailed { testing } => {
            // Initialize client for detailed checkin
            let port = cli.port.unwrap_or(if testing { 5666 } else { 5600 });
            let host = "127.0.0.1";
            let client =
                match aw_client_rust::blocking::AwClient::new(host, port, "aw-notify-checkin") {
                    Ok(client) => client,
                    Err(e) => return Err(anyhow!("Failed to create client: {}", e)),
                };

            let hostname = get_hostname()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            // Set global state
            AW_CLIENT.set(client).ok();
            HOSTNAME.set(hostname).ok();

            send_detailed_checkin("Detailed Time Summary", None)?;
            Ok(())
        }
    }
}

fn start_service(hostname: String, config: NotificationConfig) -> Result<()> {
    log::info!("Starting notification service...");

    // Create shutdown channels for each thread
    let (shutdown_tx_main, shutdown_rx_main) = bounded::<()>(1);
    let (shutdown_tx_hourly, shutdown_rx_hourly) = bounded::<()>(1);
    let (shutdown_tx_newday, shutdown_rx_newday) = bounded::<()>(1);
    let (shutdown_tx_monitor, shutdown_rx_monitor) = bounded::<()>(1);

    // Setup signal handler for graceful shutdown (handles Ctrl+C, SIGTERM, etc.)
    // This uses the ctrlc crate which provides cross-platform signal handling
    let shutdown_senders = vec![
        shutdown_tx_main.clone(),
        shutdown_tx_hourly.clone(),
        shutdown_tx_newday.clone(),
        shutdown_tx_monitor.clone(),
    ];

    if let Err(e) = ctrlc::set_handler(move || {
        log::info!("Received interrupt signal (Ctrl+C/SIGTERM), initiating graceful shutdown...");
        // Send shutdown signal to all waiting threads
        for sender in &shutdown_senders {
            let _ = sender.try_send(());
        }
    }) {
        log::warn!(
            "Failed to setup signal handler: {}. Continuing without graceful shutdown support.",
            e
        );
        // Continue running even if signal handler setup fails
        return threshold_alerts(shutdown_rx_main, config.alerts);
    }

    log::debug!("Signal handler installed successfully");

    // Send initial notifications (matching Python's start function)
    if let Err(e) = send_checkin("Time today", None) {
        log::warn!("Failed to send initial checkin: {} (continuing anyway)", e);
    }

    if let Err(e) = send_checkin_yesterday() {
        log::warn!(
            "Failed to send yesterday checkin: {} (continuing anyway)",
            e
        );
    }

    // Start background threads based on configuration
    if config.hourly_checkins {
        start_hourly(hostname.clone(), shutdown_rx_hourly);
    } else {
        log::info!("Hourly checkins disabled in configuration");
    }

    if config.new_day_greetings {
        start_new_day(hostname.clone(), shutdown_rx_newday);
    } else {
        log::info!("New day greetings disabled in configuration");
    }

    if config.server_monitoring {
        start_server_monitor(shutdown_rx_monitor);
    } else {
        log::info!("Server monitoring disabled in configuration");
    }

    // Main threshold monitoring loop (matching Python's threshold_alerts function)
    let result = threshold_alerts(shutdown_rx_main, config.alerts);

    // Give background threads a moment to finish cleanup
    thread::sleep(time::Duration::from_millis(100));

    log::info!("Shutdown complete");
    result
}

// CategoryAlert struct (exact copy of Python's CategoryAlert logic)
struct CategoryAlert {
    category: String,
    label: String,
    thresholds: Vec<Duration>,
    max_triggered: Duration,
    time_spent: Duration,
    last_check: DateTime<Utc>,
    positive: bool,
    last_status: Option<String>,
}

impl CategoryAlert {
    fn new(category: &str, thresholds: Vec<Duration>, label: Option<&str>, positive: bool) -> Self {
        Self {
            category: category.to_string(),
            label: label.unwrap_or(category).to_string(),
            thresholds,
            max_triggered: Duration::zero(),
            time_spent: Duration::zero(),
            last_check: Utc.timestamp_opt(0, 0).unwrap(),
            positive,
            last_status: None,
        }
    }

    fn thresholds_untriggered(&self) -> Vec<Duration> {
        self.thresholds
            .iter()
            .filter(|&t| *t > self.max_triggered)
            .cloned()
            .collect()
    }

    fn time_to_next_threshold(&self) -> Duration {
        let untriggered = self.thresholds_untriggered();
        if untriggered.is_empty() {
            // If no thresholds to trigger, wait until tomorrow (like Python)
            let now = Local::now();
            let day_end = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
            let mut day_end = Local.from_local_datetime(&day_end).single().unwrap();
            if day_end < now {
                day_end += Duration::days(1);
            }
            let time_to_next_day = day_end - now + TIME_OFFSET;
            return time_to_next_day
                + self
                    .thresholds
                    .iter()
                    .min()
                    .cloned()
                    .unwrap_or(Duration::zero());
        }

        let min_threshold = untriggered.iter().min().cloned().unwrap();
        (min_threshold - self.time_spent).max(Duration::zero())
    }

    fn update(&mut self) {
        let now = Local::now();
        let time_to_threshold = self.time_to_next_threshold();

        if now.with_timezone(&Utc) > (self.last_check + time_to_threshold) {
            // Get time data (will use cached version if available)
            match get_time(None, CategoryAggregation::AllLevels) {
                Ok(cat_time) => {
                    if let Some(&seconds) = cat_time.get(&self.category) {
                        self.time_spent = Duration::seconds(seconds as i64);
                    }
                }
                Err(e) => {
                    log::error!("Error getting time for {}: {}", self.category, e);
                }
            }
            self.last_check = now.with_timezone(&Utc);
        }
    }

    fn check(&mut self, silent: bool) {
        // Sort thresholds in descending order (like Python)
        let mut untriggered = self.thresholds_untriggered();
        untriggered.sort_by(|a, b| b.cmp(a));

        for threshold in untriggered {
            if threshold <= self.time_spent {
                // Threshold reached
                self.max_triggered = threshold;

                if !silent {
                    let threshold_str = to_hms(threshold);
                    let spent_str = to_hms(self.time_spent);

                    let title = if self.positive {
                        "Goal reached!"
                    } else {
                        "Time spent"
                    };
                    let message = if threshold_str != spent_str {
                        format!("{}: {}  ({})", self.label, threshold_str, spent_str)
                    } else {
                        format!("{}: {}", self.label, threshold_str)
                    };

                    if let Err(e) = notify(title, &message) {
                        log::error!("Failed to send notification: {}", e);
                    }
                }
                break;
            }
        }
    }

    fn status(&self) -> String {
        format!("{}: {}", self.label, to_hms(self.time_spent))
    }
}

fn threshold_alerts(shutdown_rx: Receiver<()>, alert_configs: Vec<AlertConfig>) -> Result<()> {
    log::info!("Starting threshold alerts monitoring...");

    if alert_configs.is_empty() {
        log::info!("No alerts configured, threshold monitoring disabled");
        // Still wait for shutdown signal
        let _ = shutdown_rx.recv();
        return Ok(());
    }

    // Create alerts from configuration
    let mut alerts: Vec<CategoryAlert> = alert_configs
        .into_iter()
        .map(|config| {
            let thresholds: Vec<Duration> = config
                .thresholds_minutes
                .into_iter()
                .map(|minutes| Duration::minutes(minutes as i64))
                .collect();

            CategoryAlert::new(
                &config.category,
                thresholds,
                config.label.as_deref(),
                config.positive,
            )
        })
        .collect();

    log::info!("Configured {} alert(s)", alerts.len());

    // Run through them once to check if any thresholds have been reached (silent)
    for alert in &mut alerts {
        alert.update();
        alert.check(true);
    }

    // Main monitoring loop (like Python)
    loop {
        for alert in &mut alerts {
            alert.update();
            alert.check(false);

            // Log status changes (like Python)
            let status = alert.status();
            if Some(&status) != alert.last_status.as_ref() {
                alert.last_status = Some(status);
            }
        }

        // Wait for shutdown signal or timeout (10 seconds for normal monitoring)
        match shutdown_rx.recv_timeout(time::Duration::from_secs(10)) {
            Ok(_) => {
                log::info!("Shutdown signal received, stopping threshold alerts monitoring");
                break;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // Normal timeout, continue monitoring
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::warn!("Shutdown channel disconnected, stopping threshold alerts monitoring");
                break;
            }
        }
    }

    log::info!("Threshold alerts monitoring stopped");
    Ok(())
}

// Cache implementation (matching Python's @cache_ttl decorator)
fn get_time(
    date: Option<DateTime<Utc>>,
    aggregation_mode: CategoryAggregation,
) -> Result<HashMap<String, f64>> {
    let cache_key = format!("{:?}_{:?}", date, aggregation_mode);

    // Check cache first (matching Python's @cache_ttl decorator)
    if let Some(entry) = TIME_CACHE.get(&cache_key) {
        let (cached_time, cached_data) = entry.value();
        if (Local::now().with_timezone(&Utc) - *cached_time).num_seconds() < CACHE_TTL_SECONDS {
            return Ok(cached_data.clone());
        }
    }

    // Query ActivityWatch (matching Python logic exactly)
    let result = query_activitywatch(date, aggregation_mode)?;

    // Cache the result
    TIME_CACHE.insert(
        cache_key,
        (Local::now().with_timezone(&Utc), result.clone()),
    );

    Ok(result)
}

fn query_activitywatch(
    date: Option<DateTime<Utc>>,
    aggregation_mode: CategoryAggregation,
) -> Result<HashMap<String, f64>> {
    let client = AW_CLIENT
        .get()
        .ok_or_else(|| anyhow!("Client not initialized"))?;
    let hostname = HOSTNAME
        .get()
        .ok_or_else(|| anyhow!("Hostname not initialized"))?
        .clone();

    let date = date.unwrap_or_else(|| Local::now().with_timezone(&Utc));

    // Set timeperiod to the requested date (like old version)
    let local_date = date.with_timezone(&Local);
    log::debug!(
        "Query date in local timezone: {}",
        local_date.format("%Y-%m-%d %H:%M:%S %z")
    );
    let day_start = Local
        .with_ymd_and_hms(
            local_date.year(),
            local_date.month(),
            local_date.day(),
            0,
            0,
            0,
        )
        .single()
        .unwrap()
        .with_timezone(&Utc);

    let timeperiod = TimeInterval::new(
        day_start + TIME_OFFSET,
        day_start + TIME_OFFSET + Duration::days(1),
    );

    // Build QueryParams like old version
    let bid_window = format!("aw-watcher-window_{}", hostname);
    let bid_afk = format!("aw-watcher-afk_{}", hostname);

    let always_active_pattern = match client.get_setting("always_active_pattern") {
        Ok(v) => v.as_str().map(|s| s.to_string()),
        Err(e) => {
            log::warn!("Failed to fetch always_active_pattern: {}", e);
            None
        }
    };

    let base_params = QueryParamsBase {
        bid_browsers: vec![],
        classes: get_server_classes(),
        filter_classes: vec![],
        filter_afk: true,
        include_audible: true,
    };

    let desktop_params = DesktopQueryParams {
        base: base_params,
        bid_window,
        bid_afk,
        always_active_pattern,
    };
    let query_params = QueryParams::Desktop(desktop_params);

    // Generate canonical events query (like old version)
    let canonical_events = query_params.canonical_events();

    // Build the complete query
    let query = format!(
        r#"{}
duration = sum_durations(events);
cat_events = sort_by_duration(merge_events_by_keys(events, ["$category"]));
RETURN = {{"events": events, "duration": duration, "cat_events": cat_events}};"#,
        canonical_events
    );

    // Execute the query
    let timeperiods = vec![(*timeperiod.start(), *timeperiod.end())];
    let result = client.query(&query, timeperiods)?;

    // Get first result (like old version)
    let result = result
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("No query results"))?;

    let mut cat_time = HashMap::new();

    // Process cat_events from the query result (exactly like old version)
    if let Some(cat_events) = result.get("cat_events").and_then(|ce| ce.as_array()) {
        for event in cat_events {
            if let (Some(category), Some(duration)) = (
                event.get("data").and_then(|d| d.get("$category")),
                event.get("duration").and_then(|d| d.as_f64()),
            ) {
                // Handle both string and array category formats (like old version)
                let cat_name = if let Some(cat_array) = category.as_array() {
                    // For hierarchical categories like ["Work", "Programming", "ActivityWatch"],
                    // join them with " > " to preserve the full hierarchy
                    let category_parts: Vec<String> = cat_array
                        .iter()
                        .filter_map(|c| c.as_str())
                        .map(|s| s.to_string())
                        .collect();

                    if !category_parts.is_empty() {
                        category_parts.join(" > ")
                    } else {
                        "Unknown".to_string()
                    }
                } else if let Some(cat_str) = category.as_str() {
                    cat_str.to_string()
                } else {
                    "Unknown".to_string()
                };

                *cat_time.entry(cat_name).or_insert(0.0) += duration;
            }
        }
    }

    // Add "All" category with total duration if we have data (like old version)
    if let Some(total_duration) = result.get("duration").and_then(|d| d.as_f64()) {
        cat_time.insert("All".to_string(), total_duration);
    } else if !cat_time.is_empty() {
        // If no duration but we have categories, sum them
        let total: f64 = cat_time.values().sum();
        cat_time.insert("All".to_string(), total);
    }

    // Ensure we always have an "All" category
    if cat_time.is_empty() {
        cat_time.insert("All".to_string(), 0.0);
    }

    // Apply aggregation based on the mode
    match aggregation_mode {
        CategoryAggregation::None => Ok(cat_time),
        CategoryAggregation::TopLevelOnly => Ok(aggregate_categories_by_top_level(&cat_time)),
        CategoryAggregation::AllLevels => Ok(aggregate_categories_by_all_levels(&cat_time)),
    }
}

fn send_checkin(title: &str, date: Option<DateTime<Utc>>) -> Result<()> {
    log::info!("Sending checkin: {}", title);

    let cat_time = get_time(date, CategoryAggregation::TopLevelOnly)?;

    // Get top categories with clean formatting (like old version)
    let top_categories = get_top_level_categories_for_notifications(&cat_time, 0.02, 4);

    if !top_categories.is_empty() {
        let message = top_categories
            .iter()
            .map(|(cat, time)| format!("- {}: {}", decode_unicode_escapes(cat), time))
            .collect::<Vec<_>>()
            .join("\n");

        notify(title, &message)?;
    } else {
        // No time spent
    }

    Ok(())
}

fn send_detailed_checkin(title: &str, date: Option<DateTime<Utc>>) -> Result<()> {
    log::info!("Sending detailed checkin: {}", title);

    let cat_time = get_time(date, CategoryAggregation::AllLevels)?;

    // Get top categories with all-level aggregation
    let top_categories = get_all_level_categories_for_notifications(&cat_time, 0.02, 10);

    if !top_categories.is_empty() {
        let message = top_categories
            .iter()
            .map(|(cat, time)| format!("- {}: {}", decode_unicode_escapes(cat), time))
            .collect::<Vec<_>>()
            .join("\n");

        notify(title, &message)?;
    } else {
        // No time spent
    }

    Ok(())
}

fn send_checkin_yesterday() -> Result<()> {
    let yesterday = Local::now().with_timezone(&Utc) - Duration::days(1);
    send_checkin("Time yesterday", Some(yesterday))
}

fn start_hourly(hostname: String, shutdown_rx: Receiver<()>) {
    thread::spawn(move || {
        log::info!("Starting hourly checkin thread");

        loop {
            // Wait until next whole hour (like Python)
            let now = Local::now();
            let next_hour = now + Duration::hours(1);
            let next_hour = next_hour
                .date_naive()
                .and_hms_opt(next_hour.hour(), 0, 0)
                .unwrap();
            let next_hour = Local.from_local_datetime(&next_hour).single().unwrap();
            let sleep_time = (next_hour - now)
                .to_std()
                .unwrap_or(time::Duration::from_secs(3600));

            // Wait for either timeout (next hour) or shutdown signal
            match shutdown_rx.recv_timeout(sleep_time) {
                Ok(_) => {
                    log::info!("Shutdown signal received, stopping hourly checkin thread");
                    break;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Time for hourly checkin
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    log::warn!("Shutdown channel disconnected, stopping hourly checkin thread");
                    break;
                }
            }

            // Check if user is active (like Python)
            match get_active_status(&hostname) {
                Ok(Some(true)) => {
                    log::info!("User is active, sending hourly checkin");
                    if let Err(e) = send_checkin("Hourly summary", None) {
                        log::error!("Failed to send hourly checkin: {}", e);
                    }
                }
                Ok(Some(false)) => {
                    log::info!("User is AFK, skipping hourly checkin");
                }
                Ok(None) => {
                    log::warn!("Can't determine AFK status, skipping hourly checkin");
                }
                Err(e) => {
                    log::error!("Error getting AFK status: {}", e);
                }
            }
        }

        log::info!("Hourly checkin thread stopped");
    });
}

fn start_new_day(hostname: String, shutdown_rx: Receiver<()>) {
    thread::spawn(move || {
        log::info!("Starting new day notification thread");

        let mut last_day = (Local::now() - TIME_OFFSET).date_naive();
        log::info!(
            "Starting new day detection with local timezone: {}",
            Local::now().format("%Y-%m-%d %H:%M:%S %z")
        );

        loop {
            let now = Local::now();
            let day = (now - TIME_OFFSET).date_naive();
            log::debug!(
                "Checking new day: current local time = {}, day = {}",
                now.format("%Y-%m-%d %H:%M:%S %z"),
                day
            );

            // Check for new day
            if day != last_day {
                match get_active_status(&hostname) {
                    Ok(Some(true)) => {
                        log::info!("New day, sending notification");
                        let day_of_week = day.format("%A");
                        let message = format!("It is {}, {}", day_of_week, day);

                        if let Err(e) = notify("New day", &message) {
                            log::error!("Failed to send new day notification: {}", e);
                        }
                        last_day = day;
                    }
                    Ok(Some(false)) => {
                        log::debug!("User is AFK, not sending new day notification yet");
                    }
                    Ok(None) => {
                        log::warn!("Can't determine AFK status, skipping new day check");
                    }
                    Err(e) => {
                        log::error!("Error getting AFK status: {}", e);
                    }
                }
            }

            // Calculate adaptive polling interval
            let sleep_time = calculate_new_day_polling_interval(now);

            log::debug!("New day thread sleeping for 5 minutes until next check");

            // Wait for shutdown signal or timeout
            match shutdown_rx.recv_timeout(sleep_time) {
                Ok(_) => {
                    log::info!("Shutdown signal received, stopping new day notification thread");
                    break;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Normal timeout, continue checking
                    continue;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    log::warn!(
                        "Shutdown channel disconnected, stopping new day notification thread"
                    );
                    break;
                }
            }
        }

        log::info!("New day notification thread stopped");
    });
}

/// Calculate polling interval for new day detection
/// - Always poll every 5 minutes for consistent checking
fn calculate_new_day_polling_interval(_now: DateTime<Local>) -> time::Duration {
    log::debug!("Using 5-minute polling for new day detection");
    time::Duration::from_secs(5 * 60) // 5 minutes
}

fn start_server_monitor(shutdown_rx: Receiver<()>) {
    thread::spawn(move || {
        log::info!("Starting server monitor thread");

        loop {
            let current_status = check_server_availability();
            let previous_status = SERVER_AVAILABLE.load(Ordering::Relaxed);

            if current_status != previous_status {
                if current_status {
                    log::info!("Server is back online");
                    if let Err(e) =
                        notify("Server Available", "ActivityWatch server is back online.")
                    {
                        log::error!("Failed to send server available notification: {}", e);
                    }
                } else {
                    log::warn!("Server went offline");
                    if let Err(e) = notify(
                        "Server Unavailable",
                        "ActivityWatch server is down. Data may not be saved!",
                    ) {
                        log::error!("Failed to send server unavailable notification: {}", e);
                    }
                }
                SERVER_AVAILABLE.store(current_status, Ordering::Relaxed);
            }

            // Wait for shutdown signal or timeout (10 seconds for monitoring)
            match shutdown_rx.recv_timeout(time::Duration::from_secs(10)) {
                Ok(_) => {
                    log::info!("Shutdown signal received, stopping server monitor thread");
                    break;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Normal timeout, continue monitoring
                    continue;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    log::warn!("Shutdown channel disconnected, stopping server monitor thread");
                    break;
                }
            }
        }

        log::info!("Server monitor thread stopped");
    });
}

fn get_active_status(hostname: &str) -> Result<Option<bool>> {
    let client = AW_CLIENT
        .get()
        .ok_or_else(|| anyhow!("Client not initialized"))?;

    let bucket_name = format!("aw-watcher-afk_{}", hostname);
    let events = client.get_events(&bucket_name, None, None, Some(1))?;

    if events.is_empty() {
        return Ok(None);
    }

    let event = &events[0];
    let event_end = event.timestamp + event.duration;

    // Check if event is too old (like Python - 5 minutes)
    if event_end < Local::now().with_timezone(&Utc) - Duration::minutes(5) {
        log::warn!("AFK event is too old, can't use to reliably determine AFK state");
        return Ok(None);
    }

    if let Some(status) = event.data.get("status") {
        if let Some(status_str) = status.as_str() {
            return Ok(Some(status_str == "not-afk"));
        }
    }

    Ok(None)
}

fn check_server_availability() -> bool {
    if let Some(client) = AW_CLIENT.get() {
        match client.get_info() {
            Ok(_) => true,
            Err(_e) => false,
        }
    } else {
        false
    }
}

fn notify(title: &str, message: &str) -> Result<()> {
    let output_only = OUTPUT_ONLY.load(Ordering::Relaxed);

    if output_only {
        // Output only mode - print to stdout with separators
        println!("{}", "-".repeat(50));
        println!("{}:", title);
        println!("{}", message);
        println!("{}", "-".repeat(50));
        return Ok(());
    }

    log::info!(r#"Showing: "{}\n{}""#, title, message);

    // Try terminal-notifier first on macOS (like Python)
    #[cfg(target_os = "macos")]
    {
        if try_terminal_notifier(title, message)? {
            return Ok(());
        }
    }

    // Fall back to notify-rust (like Python falls back to desktop-notifier)
    Notification::new()
        .summary(title)
        .body(message)
        .appname("ActivityWatch")
        .timeout(5000)
        .show()?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn try_terminal_notifier(title: &str, message: &str) -> Result<bool> {
    use std::process::Command;

    // Check if terminal-notifier is available (like Python's shutil.which)
    match Command::new("which").arg("terminal-notifier").output() {
        Ok(output) if output.status.success() => {
            // terminal-notifier is available, use it
            let result = Command::new("terminal-notifier")
                .arg("-title")
                .arg("ActivityWatch")
                .arg("-subtitle")
                .arg(title)
                .arg("-message")
                .arg(message)
                .arg("-group")
                .arg(title)
                .arg("-open")
                .arg("http://localhost:5600")
                .output()?;

            Ok(result.status.success())
        }
        _ => Ok(false), // terminal-notifier not available
    }
}

#[cfg(not(target_os = "macos"))]
fn try_terminal_notifier(_title: &str, _message: &str) -> Result<bool> {
    Ok(false)
}

fn to_hms(duration: Duration) -> String {
    let days = duration.num_days();
    let hours = duration.num_hours() % 24;
    let minutes = duration.num_minutes() % 60;
    let seconds = duration.num_seconds() % 60;

    let mut parts = Vec::new();

    if days > 0 {
        parts.push(format!("{}d", days));
    }
    if hours > 0 {
        parts.push(format!("{}h", hours));
    }
    if minutes > 0 {
        parts.push(format!("{}m", minutes));
    }
    if parts.is_empty() {
        parts.push(format!("{}s", seconds));
    }

    parts.join(" ")
}

fn decode_unicode_escapes(s: &str) -> String {
    // Simple implementation for now - matches Python's decode_unicode_escapes
    // Could be enhanced to handle actual Unicode escape sequences
    s.to_string()
}

// === CATEGORY MATCHING AND PROCESSING FUNCTIONS ===
//Get categorization classes from server with fallback to defaults
fn get_server_classes() -> Vec<(CategoryId, CategorySpec)> {
    // Try to get classes from server (like old version)
    let client = AW_CLIENT.get().unwrap();

    client
        .get_setting("classes")
        .map(|setting_value| {
            // Try to deserialize the setting into Vec<ClassSetting>
            if setting_value.is_null() {
                return default_classes();
            }

            let class_settings: Vec<ClassSetting> = match serde_json::from_value(setting_value) {
                Ok(classes) => classes,
                Err(e) => {
                    log::warn!(
                        "Failed to deserialize classes setting: {}, using default classes",
                        e
                    );
                    return default_classes();
                }
            };

            // Convert ClassSetting to (CategoryId, CategorySpec) format
            class_settings
                .into_iter()
                .map(|class| (class.name, class.rule))
                .collect()
        })
        .unwrap_or_else(|_| {
            log::warn!("Failed to get classes from server, using default classes as fallback",);
            default_classes()
        })
}

/// Aggregate hierarchical categories by their top-level category
/// E.g., "Work > Programming > ActivityWatch" -> "Work"
fn aggregate_categories_by_top_level(cat_time: &HashMap<String, f64>) -> HashMap<String, f64> {
    let mut aggregated: HashMap<String, f64> = HashMap::new();

    for (category, time) in cat_time {
        if category == "All" {
            // Preserve the "All" category
            aggregated.insert(category.clone(), *time);
            continue;
        }

        // Extract the top-level category (everything before the first " > ")
        let top_level = if let Some(pos) = category.find(" > ") {
            category[..pos].to_string()
        } else {
            category.clone()
        };

        // Add the time to the top-level category
        *aggregated.entry(top_level).or_insert(0.0) += time;
    }

    aggregated
}

/// Aggregate categories by all levels (top-level and nested)
///
/// This function creates entries for every level of the category hierarchy,
/// allowing you to see both overview totals and detailed breakdowns.
///
/// # Example
///
/// Given input categories with times:
/// - "Work > Programming > Rust": 30 minutes
/// - "Work > Programming > Python": 20 minutes
/// - "Work > Meetings": 15 minutes
/// - "Personal > Reading": 10 minutes
///
/// This function returns:
/// - "Work": 65 minutes (total of all Work subcategories)
/// - "Work > Programming": 50 minutes (total of Programming subcategories)
/// - "Work > Programming > Rust": 30 minutes (leaf category)
/// - "Work > Programming > Python": 20 minutes (leaf category)
/// - "Work > Meetings": 15 minutes (leaf category)
/// - "Personal": 10 minutes (total of all Personal subcategories)
/// - "Personal > Reading": 10 minutes (leaf category)
///
/// # Use Cases
///
/// This aggregation mode is useful when you want:
/// - A comprehensive view showing both high-level summaries and details
/// - To understand time distribution across different hierarchy levels
/// - To track both "total time in Work" and "time in specific Work activities"
fn aggregate_categories_by_all_levels(cat_time: &HashMap<String, f64>) -> HashMap<String, f64> {
    let mut aggregated: HashMap<String, f64> = HashMap::new();

    for (category, time) in cat_time {
        if category == "All" {
            // Preserve the "All" category
            aggregated.insert(category.clone(), *time);
            continue;
        }

        // Add time to the full category path (leaf level)
        *aggregated.entry(category.clone()).or_insert(0.0) += time;

        // Split by " > " and add time to all parent categories
        let parts: Vec<&str> = category.split(" > ").collect();

        // Build each parent category path and add time
        for i in 1..parts.len() {
            let parent_path = parts[..i].join(" > ");
            *aggregated.entry(parent_path).or_insert(0.0) += time;
        }
    }

    aggregated
}

/// Get appropriate emoji icon for a category
fn get_category_icon(category: &str) -> &'static str {
    let category_lower = category.to_lowercase();
    match category_lower.as_str() {
        "work" => "💼",
        "programming" | "development" | "coding" => "💻",
        "media" | "entertainment" => "📱",
        "games" | "gaming" => "🎮",
        "video" | "youtube" | "netflix" => "📺",
        "music" | "spotify" | "audio" => "🎵",
        "social" | "twitter" | "facebook" | "instagram" => "💬",
        "communication" | "email" | "slack" | "discord" => "📧",
        "browsing" | "web" => "🌐",
        "reading" => "📖",
        "writing" => "✍️",
        "design" | "graphics" => "🎨",
        "learning" | "education" => "📚",
        _ => "📊", // Default icon for other categories
    }
}

/// Format category name with appropriate emoji icon
fn format_category_for_notification(category: &str) -> String {
    let icon = get_category_icon(category);
    format!("{} {}", icon, category)
}

/// Get top categories sorted by time spent with clean formatting
fn get_top_categories(
    cat_time: &HashMap<String, f64>,
    min_percent: f64,
    max_count: usize,
) -> Vec<(String, String)> {
    let total_time = cat_time.get("All").copied().unwrap_or(0.0);

    if total_time <= 0.0 {
        return Vec::new();
    }

    let mut categories: Vec<(String, f64)> = cat_time
        .iter()
        .filter(|(cat, time)| **time > total_time * min_percent && cat.as_str() != "All")
        .map(|(cat, time)| (cat.clone(), *time))
        .collect();

    // Sort by time spent (descending)
    categories.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(cmpOrdering::Equal));

    // Limit to max_count and format durations
    categories
        .into_iter()
        .take(max_count)
        .map(|(cat, time)| (cat, to_hms(Duration::seconds(time as i64))))
        .collect()
}

/// Get top categories aggregated by top-level with emoji formatting for notifications
fn get_top_level_categories_for_notifications(
    cat_time: &HashMap<String, f64>,
    min_percent: f64,
    max_count: usize,
) -> Vec<(String, String)> {
    // First aggregate by top-level categories
    let aggregated = aggregate_categories_by_top_level(cat_time);

    // Then get the top categories from the aggregated data
    let top_cats = get_top_categories(&aggregated, min_percent, max_count);

    // Format with icons for notifications
    top_cats
        .into_iter()
        .map(|(cat, time)| (format_category_for_notification(&cat), time))
        .collect()
}

/// Get top categories aggregated by all levels (top-level and nested) with emoji formatting for notifications
///
/// This function combines all-level category aggregation with notification formatting.
/// It's useful for showing comprehensive time summaries that include both parent categories
/// and their detailed subcategories.
///
/// # Arguments
///
/// * `cat_time` - HashMap of category names to time spent (in seconds)
/// * `min_percent` - Minimum percentage of total time to include (e.g., 0.02 = 2%)
/// * `max_count` - Maximum number of categories to return
///
/// # Returns
///
/// Vector of tuples containing (formatted_category_name, formatted_time_string)
///
/// # Example
///
/// ```rust,ignore
/// let cat_time = get_time(None, CategoryAggregation::AllLevels)?;
/// let top_cats = get_all_level_categories_for_notifications(&cat_time, 0.02, 5);
/// // Returns: [("💼 Work", "2h 15m"), ("💻 Work > Programming", "1h 30m"), ...]
/// ```
fn get_all_level_categories_for_notifications(
    cat_time: &HashMap<String, f64>,
    min_percent: f64,
    max_count: usize,
) -> Vec<(String, String)> {
    // Get the top categories from the already-aggregated data
    let top_cats = get_top_categories(cat_time, min_percent, max_count);

    // Format with icons for notifications
    top_cats
        .into_iter()
        .map(|(cat, time)| (format_category_for_notification(&cat), time))
        .collect()
}
