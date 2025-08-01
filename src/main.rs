use anyhow::Result;
use aw_client_rust::blocking::AwClient;
use chrono::{DateTime, Duration, Timelike, Utc};
use clap::{Parser, Subcommand};
use dashmap::DashMap;
use log::{debug, error, info, warn};
use notify_rust::{Notification, Timeout};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration as StdDuration;

// Constants
const TIME_OFFSET_HOURS: i64 = 4;
const CACHE_TTL_SECONDS: i64 = 60;

// Type aliases to simplify complex types
type TimeCache = Arc<DashMap<String, (DateTime<Utc>, HashMap<String, Duration>)>>;

#[derive(Parser)]
#[command(name = "aw-notify")]
#[command(about = "ActivityWatch notification service")]
#[command(version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Enable testing mode
    #[arg(long)]
    testing: bool,

    /// Port to connect to ActivityWatch server
    #[arg(long)]
    port: Option<u16>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the notification service
    Start {
        /// Enable testing mode
        #[arg(long)]
        testing: bool,

        /// Port to connect to ActivityWatch server
        #[arg(long)]
        port: Option<u16>,
    },
    /// Send a summary notification
    Checkin {
        /// Enable testing mode
        #[arg(long)]
        testing: bool,
    },
}

#[derive(Clone)]
struct CategoryAlert {
    category: String,
    label: String,
    thresholds: Vec<Duration>,
    max_triggered: Duration,
    time_spent: Duration,
    last_check: DateTime<Utc>,
    positive: bool,
}

impl CategoryAlert {
    fn new(category: &str, thresholds: Vec<Duration>, label: Option<&str>, positive: bool) -> Self {
        Self {
            category: category.to_string(),
            label: label.unwrap_or(category).to_string(),
            thresholds,
            max_triggered: Duration::zero(),
            time_spent: Duration::zero(),
            last_check: DateTime::from_timestamp(0, 0).unwrap_or_else(Utc::now),
            positive,
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
            // If no thresholds to trigger, wait until tomorrow
            let now = Utc::now();
            let day_end = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc()
                + Duration::days(1)
                + Duration::hours(TIME_OFFSET_HOURS);
            let time_to_next_day = day_end - now;
            return time_to_next_day + *self.thresholds.iter().min().unwrap_or(&Duration::zero());
        }

        let zero_duration = Duration::zero();
        let min_threshold = untriggered.iter().min().unwrap_or(&zero_duration);
        *min_threshold - self.time_spent
    }

    fn update(&mut self, time_getter: &dyn Fn() -> Result<HashMap<String, Duration>>) {
        let now = Utc::now();
        let time_to_threshold = self.time_to_next_threshold();

        if now > (self.last_check + time_to_threshold) {
            debug!("Updating {}", self.category);
            match time_getter() {
                Ok(cat_time) => {
                    self.time_spent = cat_time
                        .get(&self.category)
                        .copied()
                        .unwrap_or(Duration::zero());
                }
                Err(e) => {
                    error!("Error getting time for {}: {}", self.category, e);
                }
            }
            self.last_check = now;
        }
    }

    fn check(&mut self, silent: bool) {
        let mut triggered_thresholds: Vec<Duration> = self
            .thresholds_untriggered()
            .into_iter()
            .filter(|&t| t <= self.time_spent)
            .collect();

        triggered_thresholds.sort_by(|a, b| b.cmp(a)); // Sort in descending order

        if let Some(&threshold) = triggered_thresholds.first() {
            self.max_triggered = threshold;
            if !silent {
                let threshold_str = format_duration(threshold);
                let spent_str = format_duration(self.time_spent);
                let title = if self.positive {
                    "Goal reached!"
                } else {
                    "Time spent"
                };
                let message = if threshold_str == spent_str {
                    format!("{}: {}", self.label, threshold_str)
                } else {
                    format!("{}: {}  ({})", self.label, threshold_str, spent_str)
                };
                send_notification(title, &message);
            }
        }
    }

    fn status(&self) -> String {
        format!("{}: {}", self.label, format_duration(self.time_spent))
    }
}

struct NotificationService {
    client: AwClient,
    hostname: String,
    time_cache: TimeCache,
    server_available: Arc<AtomicBool>,
}

impl NotificationService {
    fn new(testing: bool, port: Option<u16>) -> Result<Self> {
        let actual_port = port.unwrap_or(if testing { 5666 } else { 5600 });
        let client = AwClient::new("127.0.0.1", actual_port, "aw-notify-rs")
            .map_err(|e| anyhow::anyhow!("Failed to create ActivityWatch client: {}", e))?;

        // Wait for server to be available
        info!("Waiting for ActivityWatch server...");
        loop {
            match client.get_info() {
                Ok(_) => break,
                Err(_) => {
                    thread::sleep(StdDuration::from_secs(1));
                }
            }
        }

        let hostname = hostname::get()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        Ok(Self {
            client,
            hostname,
            time_cache: Arc::new(DashMap::new()),
            server_available: Arc::new(AtomicBool::new(true)),
        })
    }

    fn get_time(&self, date: Option<DateTime<Utc>>) -> Result<HashMap<String, Duration>> {
        let date = date.unwrap_or_else(Utc::now);
        let cache_key = format!("{}", date.date_naive());

        // Check cache
        if let Some(entry) = self.time_cache.get(&cache_key) {
            let (cached_time, cached_data) = &*entry;
            if Utc::now() - *cached_time < Duration::seconds(CACHE_TTL_SECONDS) {
                debug!("Using cached data for {}", cache_key);
                return Ok(cached_data.clone());
            }
        }

        let date_start = date.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc()
            + Duration::hours(TIME_OFFSET_HOURS);

        let date_end = date_start + Duration::days(1);

        let window_bucket = format!("aw-watcher-window_{}", self.hostname);
        let afk_bucket = format!("aw-watcher-afk_{}", self.hostname);

        let query = format!(
            r#"
            window_events = flood(query_bucket("{}"));
            afk_events = flood(query_bucket("{}"));
            events = filter_period_intersect(window_events, filter_keyvals(afk_events, "status", ["not-afk"]));
            events = categorize(events, [[["Work"], {{"type": "regex", "regex": ".*"}}]]);
            events = merge_events_by_keys(events, ["$category"]);
            events = sort_by_duration(events);
            duration = sum_durations(events);
            cat_events = events;
            RETURN = {{"events": events, "duration": duration, "cat_events": cat_events}};
            "#,
            window_bucket, afk_bucket
        );

        let timeperiods = vec![(date_start, date_end)];

        match self.client.query(&query, timeperiods) {
            Ok(results) => {
                if let Some(result) = results.first() {
                    let mut cat_time = HashMap::new();

                    // Add total duration
                    if let Some(duration) = result.get("duration").and_then(|v| v.as_f64()) {
                        cat_time.insert("All".to_string(), Duration::seconds(duration as i64));
                    }

                    // Add category durations
                    if let Some(cat_events) = result.get("cat_events").and_then(|v| v.as_array()) {
                        for event in cat_events {
                            if let (Some(data), Some(duration)) = (
                                event.get("data"),
                                event.get("duration").and_then(|v| v.as_f64()),
                            ) {
                                if let Some(category_array) =
                                    data.get("$category").and_then(|v| v.as_array())
                                {
                                    if let Some(category) =
                                        category_array.first().and_then(|v| v.as_str())
                                    {
                                        cat_time.insert(
                                            category.to_string(),
                                            Duration::seconds(duration as i64),
                                        );
                                    }
                                }
                            }
                        }
                    }

                    // Cache the result
                    self.time_cache
                        .insert(cache_key, (Utc::now(), cat_time.clone()));
                    Ok(cat_time)
                } else {
                    Ok(HashMap::new())
                }
            }
            Err(e) => Err(anyhow::anyhow!("Query failed: {}", e)),
        }
    }

    fn send_checkin(&self, title: &str, date: Option<DateTime<Utc>>) {
        match self.get_time(date) {
            Ok(cat_time) => {
                let total_time = cat_time.get("All").copied().unwrap_or(Duration::zero());
                let threshold = total_time * 2 / 100; // 2% threshold

                let mut top_categories: Vec<(String, Duration)> = cat_time
                    .iter()
                    .filter(|(_, &duration)| duration > threshold && duration > Duration::zero())
                    .map(|(k, &v)| (k.clone(), v))
                    .collect();

                top_categories.sort_by(|a, b| b.1.cmp(&a.1));
                top_categories.truncate(4);

                if !top_categories.is_empty() {
                    let message = top_categories
                        .iter()
                        .map(|(cat, duration)| format!("- {}: {}", cat, format_duration(*duration)))
                        .collect::<Vec<_>>()
                        .join("\n");

                    send_notification(title, &message);
                } else {
                    debug!("No significant time spent");
                }
            }
            Err(e) => {
                error!("Error getting time: {}", e);
            }
        }
    }

    fn check_server_availability(&self) -> bool {
        match self.client.get_info() {
            Ok(_) => true,
            Err(e) => {
                warn!("Server check failed: {}", e);
                false
            }
        }
    }

    fn get_active_status(&self) -> Option<bool> {
        let afk_bucket = format!("aw-watcher-afk_{}", self.hostname);

        match self.client.get_events(&afk_bucket, None, None, Some(1)) {
            Ok(events) => {
                if let Some(event) = events.first() {
                    let event_end = event.timestamp + event.duration;
                    if event_end < Utc::now() - Duration::minutes(5) {
                        warn!("AFK event is too old, can't use to reliably determine AFK state");
                        return None;
                    }

                    if let Some(status) = event.data.get("status").and_then(|v| v.as_str()) {
                        return Some(status == "not-afk");
                    }
                }
                None
            }
            Err(_) => None,
        }
    }

    fn start_threshold_alerts(&self) {
        let service = self.clone();
        thread::spawn(move || {
            let mut alerts = vec![
                CategoryAlert::new(
                    "All",
                    vec![
                        Duration::hours(1),
                        Duration::hours(2),
                        Duration::hours(4),
                        Duration::hours(6),
                        Duration::hours(8),
                    ],
                    Some("All"),
                    false,
                ),
                CategoryAlert::new(
                    "Twitter",
                    vec![
                        Duration::minutes(15),
                        Duration::minutes(30),
                        Duration::hours(1),
                    ],
                    Some("🐦 Twitter"),
                    false,
                ),
                CategoryAlert::new(
                    "Youtube",
                    vec![
                        Duration::minutes(15),
                        Duration::minutes(30),
                        Duration::hours(1),
                    ],
                    Some("📺 Youtube"),
                    false,
                ),
                CategoryAlert::new(
                    "Work",
                    vec![
                        Duration::minutes(15),
                        Duration::minutes(30),
                        Duration::hours(1),
                        Duration::hours(2),
                        Duration::hours(4),
                    ],
                    Some("💼 Work"),
                    true,
                ),
            ];

            // Initial check (silent)
            for alert in &mut alerts {
                alert.update(&|| service.get_time(None));
                alert.check(true);
            }

            loop {
                for alert in &mut alerts {
                    alert.update(&|| service.get_time(None));
                    alert.check(false);
                    debug!("Alert status: {}", alert.status());
                }

                thread::sleep(StdDuration::from_secs(10));
            }
        });
    }

    fn start_hourly_checkins(&self) {
        let service = self.clone();
        thread::spawn(move || {
            loop {
                // Wait until next whole hour
                let now = Utc::now();
                let next_hour = (now + Duration::hours(1))
                    .with_minute(0)
                    .unwrap()
                    .with_second(0)
                    .unwrap()
                    .with_nanosecond(0)
                    .unwrap();
                let sleep_duration = (next_hour - now)
                    .to_std()
                    .unwrap_or(StdDuration::from_secs(60));

                debug!("Sleeping for {:?} until next hour", sleep_duration);
                thread::sleep(sleep_duration);

                // Check if user is active
                match service.get_active_status() {
                    Some(true) => {
                        info!("Sending hourly checkin");
                        service.send_checkin("Hourly Update", None);
                    }
                    Some(false) => {
                        info!("User is AFK, skipping hourly checkin");
                    }
                    None => {
                        warn!("Can't determine AFK status, skipping hourly checkin");
                    }
                }
            }
        });
    }

    fn start_server_monitor(&self) {
        let service = self.clone();
        thread::spawn(move || loop {
            let current_status = service.check_server_availability();
            let previous_status = service.server_available.load(Ordering::Relaxed);

            if current_status != previous_status {
                if current_status {
                    send_notification("Server Available", "ActivityWatch server is back online.");
                } else {
                    send_notification(
                        "Server Unavailable",
                        "ActivityWatch server is down. Data may not be saved!",
                    );
                }
                service
                    .server_available
                    .store(current_status, Ordering::Relaxed);
            }

            thread::sleep(StdDuration::from_secs(10));
        });
    }

    fn start_new_day_notifications(&self) {
        let service = self.clone();
        thread::spawn(move || {
            let mut last_day = (Utc::now() - Duration::hours(TIME_OFFSET_HOURS)).date_naive();

            loop {
                let now = Utc::now();
                let current_day = (now - Duration::hours(TIME_OFFSET_HOURS)).date_naive();

                if current_day != last_day {
                    match service.get_active_status() {
                        Some(true) => {
                            info!("New day, sending notification");
                            let day_of_week = current_day.format("%A");
                            send_notification(
                                "New day",
                                &format!("It is {}, {}", day_of_week, current_day),
                            );
                            last_day = current_day;
                        }
                        Some(false) => {
                            debug!("User is AFK, not sending new day notification yet");
                        }
                        None => {
                            warn!("Can't determine AFK status, skipping new day check");
                        }
                    }
                } else {
                    // Sleep until tomorrow
                    let start_of_tomorrow = (now + Duration::days(1))
                        .date_naive()
                        .and_hms_opt(0, 0, 0)
                        .unwrap()
                        .and_utc();
                    let sleep_duration = (start_of_tomorrow - now)
                        .to_std()
                        .unwrap_or(StdDuration::from_secs(3600));
                    thread::sleep(sleep_duration);
                }

                thread::sleep(StdDuration::from_secs(60));
            }
        });
    }

    fn start_service(&self) {
        info!("Starting notification service...");

        // Send initial checkins
        self.send_checkin("Time today", None);
        let yesterday = Utc::now() - Duration::days(1);
        self.send_checkin("Time yesterday", Some(yesterday));

        // Start background threads
        self.start_threshold_alerts();
        self.start_hourly_checkins();
        self.start_new_day_notifications();
        self.start_server_monitor();

        info!("Notification service started. Press Ctrl+C to stop.");

        // Keep main thread alive
        loop {
            thread::sleep(StdDuration::from_secs(60));
        }
    }
}

impl Clone for NotificationService {
    fn clone(&self) -> Self {
        // Extract port from baseurl
        let port = self.client.baseurl.port().unwrap_or(5600);

        Self {
            client: AwClient::new("127.0.0.1", port, "aw-notify-rs")
                .expect("Failed to clone client"),
            hostname: self.hostname.clone(),
            time_cache: Arc::clone(&self.time_cache),
            server_available: Arc::clone(&self.server_available),
        }
    }
}

fn send_notification(title: &str, message: &str) {
    info!("Showing: \"{}\" - \"{}\"", title, message);

    match Notification::new()
        .summary(title)
        .body(message)
        .appname("ActivityWatch")
        .timeout(Timeout::Milliseconds(5000))
        .show()
    {
        Ok(_) => debug!("Notification sent successfully"),
        Err(e) => warn!("Failed to send notification: {}", e),
    }
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.num_seconds();
    let days = total_seconds / 86400;
    let hours = (total_seconds % 86400) / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

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

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level)).init();

    info!("Starting aw-notify-rs...");

    match cli.command {
        Some(Commands::Start { testing, port }) => {
            let service = NotificationService::new(testing, port)?;
            service.start_service();
        }
        Some(Commands::Checkin { testing }) => {
            let service = NotificationService::new(testing, None)?;
            service.send_checkin("Time today", None);
        }
        None => {
            // Default to start command
            let service = NotificationService::new(cli.testing, cli.port)?;
            service.start_service();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::seconds(30)), "30s");
        assert_eq!(format_duration(Duration::minutes(5)), "5m");
        assert_eq!(format_duration(Duration::hours(2)), "2h");
        assert_eq!(format_duration(Duration::hours(25)), "1d 1h");
        assert_eq!(format_duration(Duration::minutes(90)), "1h 30m");
    }

    #[test]
    fn test_category_alert_creation() {
        let alert = CategoryAlert::new(
            "Work",
            vec![Duration::minutes(15), Duration::hours(1)],
            Some("💼 Work"),
            true,
        );

        assert_eq!(alert.category, "Work");
        assert_eq!(alert.label, "💼 Work");
        assert_eq!(alert.thresholds.len(), 2);
        assert!(alert.positive);
        assert_eq!(alert.time_spent, Duration::zero());
    }

    #[test]
    fn test_category_alert_thresholds_untriggered() {
        let mut alert = CategoryAlert::new(
            "Test",
            vec![
                Duration::minutes(15),
                Duration::minutes(30),
                Duration::hours(1),
            ],
            None,
            false,
        );

        // Initially all thresholds are untriggered
        assert_eq!(alert.thresholds_untriggered().len(), 3);

        // Trigger first threshold
        alert.max_triggered = Duration::minutes(15);
        assert_eq!(alert.thresholds_untriggered().len(), 2);

        // Trigger second threshold
        alert.max_triggered = Duration::minutes(30);
        assert_eq!(alert.thresholds_untriggered().len(), 1);
    }

    #[test]
    fn test_category_alert_status() {
        let mut alert = CategoryAlert::new("Test", vec![Duration::hours(1)], None, false);
        alert.time_spent = Duration::minutes(45);

        let status = alert.status();
        assert_eq!(status, "Test: 45m");
    }
}
