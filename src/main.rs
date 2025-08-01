//! Simplified aw-notify-rs implementation matching Python version structure
//!
//! This is a complete rewrite that consolidates the functionality into a single file
//! similar to the Python version while maintaining Rust's safety and performance benefits.

use anyhow::{anyhow, Result};
use aw_client_rust::classes::{default_classes, get_classes_from_server, CategoryId, CategorySpec};
use aw_client_rust::queries::{DesktopQueryParams, QueryParams, QueryParamsBase};
use aw_models::TimeInterval;
use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use clap::Parser;
use hostname::get as get_hostname;
use notify_rust::Notification;
use once_cell::sync::Lazy;

use std::collections::HashMap;
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time;

// Global state (matching Python's global variables)
static AW_CLIENT: Lazy<Mutex<Option<aw_client_rust::blocking::AwClient>>> =
    Lazy::new(|| Mutex::new(None));
static HOSTNAME: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new("unknown".to_string()));
static SERVER_AVAILABLE: AtomicBool = AtomicBool::new(true);

// Cache for get_time function (matching Python's @cache_ttl decorator)
type CacheValue = (DateTime<Utc>, HashMap<String, f64>);
static TIME_CACHE: Lazy<Mutex<HashMap<String, CacheValue>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// Constants (matching Python exactly)
const TIME_OFFSET: Duration = Duration::hours(4);
const CACHE_TTL_SECONDS: i64 = 60;

// Duration constants for convenience
const TD_15MIN: Duration = Duration::minutes(15);
const TD_30MIN: Duration = Duration::minutes(30);
const TD_1H: Duration = Duration::hours(1);
const TD_2H: Duration = Duration::hours(2);
const TD_4H: Duration = Duration::hours(4);
const TD_6H: Duration = Duration::hours(6);
const TD_8H: Duration = Duration::hours(8);

// CLI structure (simplified, matching Python's click interface)
#[derive(Parser)]
#[clap(
    name = "aw-notify",
    about = "ActivityWatch notification service",
    version
)]
struct Cli {
    #[clap(short, long, help = "Verbose logging")]
    verbose: bool,

    #[clap(long, help = "Testing mode (port 5666)")]
    testing: bool,

    #[clap(long, help = "Port to connect to ActivityWatch server")]
    port: Option<u16>,

    #[clap(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    #[clap(about = "Start the notification service")]
    Start,
    #[clap(about = "Send a summary notification")]
    Checkin {
        #[clap(long, help = "Testing mode")]
        testing: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Setup logging (matching Python's setup_logging)
    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level)).init();

    // Suppress urllib3 equivalent (reqwest) warnings like Python
    log::set_max_level(if cli.verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    });

    log::info!("Starting...");

    // Handle commands (matching Python's main function logic)
    match cli.command.unwrap_or(Commands::Start) {
        Commands::Start => {
            // Initialize client (matching Python's start function)
            let port = cli.port.unwrap_or(if cli.testing { 5666 } else { 5600 });
            let client =
                match aw_client_rust::blocking::AwClient::new("127.0.0.1", port, "aw-notify") {
                    Ok(client) => client,
                    Err(e) => return Err(anyhow!("Failed to create client: {}", e)),
                };

            // Wait for server to be ready (like Python's wait_for_start)
            client.get_info()?;

            // Get hostname like the original code
            let hostname = get_hostname()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            // Set global state
            *AW_CLIENT.lock().unwrap() = Some(client);
            *HOSTNAME.lock().unwrap() = hostname.clone();

            start_service(hostname)
        }
        Commands::Checkin { testing } => {
            // Initialize client for checkin (matching Python's checkin function)
            let port = cli.port.unwrap_or(if testing { 5666 } else { 5600 });
            let client = match aw_client_rust::blocking::AwClient::new(
                "127.0.0.1",
                port,
                "aw-notify-checkin",
            ) {
                Ok(client) => client,
                Err(e) => return Err(anyhow!("Failed to create client: {}", e)),
            };

            // Get hostname like the original code
            let hostname = get_hostname()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            // Set global state
            *AW_CLIENT.lock().unwrap() = Some(client);
            *HOSTNAME.lock().unwrap() = hostname;

            send_checkin("Time today", None)?;
            Ok(())
        }
    }
}

fn start_service(hostname: String) -> Result<()> {
    log::info!("Starting notification service...");

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

    // Start background threads (matching Python's daemon threads)
    start_hourly(hostname.clone());
    start_new_day(hostname.clone());
    start_server_monitor();

    // Main threshold monitoring loop (matching Python's threshold_alerts function)
    threshold_alerts()
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
            let now = Utc::now();
            let day_end = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
            let mut day_end = DateTime::from_naive_utc_and_offset(day_end, Utc);
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
        let now = Utc::now();
        let time_to_threshold = self.time_to_next_threshold();

        if now > (self.last_check + time_to_threshold) {
            log::debug!("Updating {}", self.category);

            // Get time data (will use cached version if available)
            match get_time(None, true) {
                Ok(cat_time) => {
                    if let Some(&seconds) = cat_time.get(&self.category) {
                        self.time_spent = Duration::seconds(seconds as i64);
                    }
                }
                Err(e) => {
                    log::error!("Error getting time for {}: {}", self.category, e);
                }
            }
            self.last_check = now;
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

fn threshold_alerts() -> Result<()> {
    log::info!("Starting threshold alerts monitoring...");

    // Create alerts (matching Python exactly)
    let mut alerts = vec![
        CategoryAlert::new(
            "All",
            vec![TD_1H, TD_2H, TD_4H, TD_6H, TD_8H],
            Some("All"),
            false,
        ),
        CategoryAlert::new(
            "Twitter",
            vec![TD_15MIN, TD_30MIN, TD_1H],
            Some("🐦 Twitter"),
            false,
        ),
        CategoryAlert::new(
            "Youtube",
            vec![TD_15MIN, TD_30MIN, TD_1H],
            Some("📺 Youtube"),
            false,
        ),
        CategoryAlert::new(
            "Work",
            vec![TD_15MIN, TD_30MIN, TD_1H, TD_2H, TD_4H],
            Some("💼 Work"),
            true,
        ),
    ];

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
                log::debug!("New status: {}", status);
                alert.last_status = Some(status);
            }
        }

        // TODO: make configurable, perhaps increase default to save resources
        thread::sleep(time::Duration::from_secs(10));
    }
}

// Cache implementation (matching Python's @cache_ttl decorator)
fn get_time(date: Option<DateTime<Utc>>, top_level_only: bool) -> Result<HashMap<String, f64>> {
    let cache_key = format!("{:?}_{}", date, top_level_only);

    // Check cache first
    {
        let cache = TIME_CACHE.lock().unwrap();
        if let Some((cached_time, cached_data)) = cache.get(&cache_key) {
            if Utc::now() - *cached_time < Duration::seconds(CACHE_TTL_SECONDS) {
                log::debug!("Using cached data for get_time");
                return Ok(cached_data.clone());
            }
        }
    }

    log::debug!("Cache expired for get_time, updating");

    // Query ActivityWatch (matching Python logic exactly)
    let result = query_activitywatch(date, top_level_only)?;

    // Update cache
    {
        let mut cache = TIME_CACHE.lock().unwrap();
        cache.insert(cache_key, (Utc::now(), result.clone()));
    }

    Ok(result)
}

fn query_activitywatch(
    date: Option<DateTime<Utc>>,
    top_level_only: bool,
) -> Result<HashMap<String, f64>> {
    let client = AW_CLIENT.lock().unwrap();
    let client = client
        .as_ref()
        .ok_or_else(|| anyhow!("Client not initialized"))?;
    let hostname = HOSTNAME.lock().unwrap().clone();

    let date = date.unwrap_or_else(Utc::now);

    // Set timeperiod to the requested date (like old version)
    let day_start = Utc
        .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
        .single()
        .unwrap();

    let timeperiod = TimeInterval::new(
        day_start + TIME_OFFSET,
        day_start + TIME_OFFSET + Duration::days(1),
    );

    // Build QueryParams like old version
    let bid_window = format!("aw-watcher-window_{}", hostname);
    let bid_afk = format!("aw-watcher-afk_{}", hostname);

    let base_params = QueryParamsBase {
        bid_browsers: vec![],
        classes: get_server_classes(&hostname),
        filter_classes: vec![],
        filter_afk: true,
        include_audible: true,
    };

    let desktop_params = DesktopQueryParams {
        base: base_params,
        bid_window,
        bid_afk,
    };
    let query_params = QueryParams::Desktop(desktop_params);

    // Generate canonical events query (like old version)
    let canonical_events = query_params.canonical_events();

    if env::var("AW_NOTIFY_SHOW_QUERIES").is_ok() || log::log_enabled!(log::Level::Debug) {
        log::debug!("Generated canonical events query:");
        log::debug!("=== CANONICAL EVENTS START ===");
        for (i, line) in canonical_events.lines().enumerate() {
            log::debug!("{:2}: {}", i + 1, line.trim());
        }
        log::debug!("=== CANONICAL EVENTS END ===");
    }

    let query = format!(
        r#"{}
duration = sum_durations(events);
cat_events = sort_by_duration(merge_events_by_keys(events, ["$category"]));
RETURN = {{"events": events, "duration": duration, "cat_events": cat_events}};"#,
        canonical_events
    );

    if env::var("AW_NOTIFY_SHOW_QUERIES").is_ok() || log::log_enabled!(log::Level::Debug) {
        log::debug!("Built complete category summary query:");
        log::debug!("=== FULL QUERY START ===");
        for (i, line) in query.lines().enumerate() {
            if !line.trim().is_empty() {
                log::debug!("{:2}: {}", i + 1, line.trim());
            }
        }
        log::debug!("=== FULL QUERY END ===");
    }

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

    // If top_level_only, aggregate hierarchical categories
    if top_level_only {
        return Ok(aggregate_categories_by_top_level(&cat_time));
    }

    Ok(cat_time)
}

fn send_checkin(title: &str, date: Option<DateTime<Utc>>) -> Result<()> {
    log::info!("Sending checkin: {}", title);

    let cat_time = get_time(date, true)?;

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
        log::debug!("No time spent");
    }

    Ok(())
}

fn send_checkin_yesterday() -> Result<()> {
    let yesterday = Utc::now() - Duration::days(1);
    send_checkin("Time yesterday", Some(yesterday))
}

fn start_hourly(hostname: String) {
    thread::spawn(move || {
        log::info!("Starting hourly checkin thread");

        loop {
            // Wait until next whole hour (like Python)
            let now = Utc::now();
            let next_hour = now + Duration::hours(1);
            let next_hour = next_hour
                .date_naive()
                .and_hms_opt(next_hour.hour(), 0, 0)
                .unwrap();
            let next_hour = DateTime::from_naive_utc_and_offset(next_hour, Utc);
            let sleep_time = (next_hour - now)
                .to_std()
                .unwrap_or(time::Duration::from_secs(3600));

            log::debug!(
                "Sleeping for {:?} seconds until next hour",
                sleep_time.as_secs()
            );
            thread::sleep(sleep_time);

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
    });
}

fn start_new_day(hostname: String) {
    thread::spawn(move || {
        log::info!("Starting new day notification thread");

        let mut last_day = (Utc::now() - TIME_OFFSET).date_naive();

        loop {
            let now = Utc::now();
            let day = (now - TIME_OFFSET).date_naive();

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
            } else {
                // Sleep until start of tomorrow if same day
                let tomorrow = now + Duration::days(1);
                let start_of_tomorrow = tomorrow.date_naive().and_hms_opt(0, 0, 0).unwrap();
                let start_of_tomorrow = DateTime::from_naive_utc_and_offset(start_of_tomorrow, Utc);
                let sleep_time = (start_of_tomorrow - now)
                    .to_std()
                    .unwrap_or(time::Duration::from_secs(3600));
                thread::sleep(sleep_time);
            }

            thread::sleep(time::Duration::from_secs(60)); // Check every minute
        }
    });
}

fn start_server_monitor() {
    thread::spawn(|| {
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

            thread::sleep(time::Duration::from_secs(10)); // Check every 10 seconds
        }
    });
}

fn get_active_status(hostname: &str) -> Result<Option<bool>> {
    let client = AW_CLIENT.lock().unwrap();
    let client = client
        .as_ref()
        .ok_or_else(|| anyhow!("Client not initialized"))?;

    let bucket_name = format!("aw-watcher-afk_{}", hostname);
    let events = client.get_events(&bucket_name, None, None, Some(1))?;

    log::debug!("AFK events: {:?}", events);

    if events.is_empty() {
        return Ok(None);
    }

    let event = &events[0];
    let event_end = event.timestamp + event.duration;

    // Check if event is too old (like Python - 5 minutes)
    if event_end < Utc::now() - Duration::minutes(5) {
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
    let client = AW_CLIENT.lock().unwrap();
    if let Some(client) = client.as_ref() {
        match client.get_info() {
            Ok(_) => true,
            Err(e) => {
                log::debug!("Server check failed: {}", e);
                false
            }
        }
    } else {
        false
    }
}

fn notify(title: &str, message: &str) -> Result<()> {
    log::info!(r#"Showing: "{} - {}""#, title, message);

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
                .arg(
                    message
                        .trim_start_matches('-')
                        .replace("- ", ", ")
                        .replace('\n', ""),
                )
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

/// Get categorization classes from server with fallback to defaults
fn get_server_classes(_hostname: &str) -> Vec<(CategoryId, CategorySpec)> {
    // Try to get classes from server (like old version)
    let server_classes = get_classes_from_server("127.0.0.1", 5600);

    if server_classes.is_empty() {
        log::warn!("No server-side classes found, falling back to defaults");
        let classes = default_classes();

        if env::var("AW_NOTIFY_SHOW_QUERIES").is_ok() || log::log_enabled!(log::Level::Debug) {
            log::debug!("Using default classes:");
            for (i, (category, spec)) in classes.iter().enumerate() {
                log::debug!("  {}: {:?} -> {:?}", i + 1, category, spec.regex);
            }
        }
        classes
    } else {
        log::debug!("Fetched {} classes from server", server_classes.len());
        if env::var("AW_NOTIFY_SHOW_QUERIES").is_ok() || log::log_enabled!(log::Level::Debug) {
            log::debug!("Server classes loaded:");
            for (i, (category, spec)) in server_classes.iter().enumerate() {
                log::debug!("  {}: {:?} -> {:?}", i + 1, category, spec.regex);
            }
        }
        server_classes
    }
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
    categories.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

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
