use crate::dirs;
use anyhow::{anyhow, Result};
use std::fs;
use std::path::PathBuf;

// Log rotation constants
pub const MAX_LOG_SIZE: u64 = 32 * 1024 * 1024; // 32MB
pub const MAX_ROTATED_LOGS: usize = 5; // Keep last 5 rotated logs

/// Rotate log file if it exceeds MAX_LOG_SIZE
pub fn rotate_log_if_needed(log_path: &PathBuf) -> Result<()> {
    // Check if log file exists and get its size
    if !log_path.exists() {
        return Ok(());
    }

    let metadata = fs::metadata(log_path)?;
    let file_size = metadata.len();

    // Only rotate if file exceeds MAX_LOG_SIZE
    if file_size <= MAX_LOG_SIZE {
        return Ok(());
    }

    // Create rotated filename with timestamp
    let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let log_dir = log_path
        .parent()
        .ok_or_else(|| anyhow!("Failed to get log dir"))?;
    let log_name = log_path
        .file_stem()
        .ok_or_else(|| anyhow!("Failed to get log filename"))?;
    let rotated_name = format!("{}.{}.log", log_name.to_string_lossy(), timestamp);
    let rotated_path = log_dir.join(rotated_name);

    // Rename current log file
    fs::rename(log_path, &rotated_path)?;

    // Clean up old rotated logs, keeping only MAX_ROTATED_LOGS most recent
    cleanup_old_logs(log_dir, &log_name.to_string_lossy())?;

    Ok(())
}

/// Remove old rotated logs, keeping only the most recent MAX_ROTATED_LOGS
fn cleanup_old_logs(log_dir: &std::path::Path, log_name: &str) -> Result<()> {
    let mut rotated_logs: Vec<_> = fs::read_dir(log_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.starts_with(&format!("{}.", log_name))
                && name.ends_with(".log")
                && name != format!("{}.log", log_name)
        })
        .collect();

    // Sort by modification time (newest first)
    rotated_logs.sort_by_key(|entry| {
        entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    rotated_logs.reverse();

    // Remove logs beyond MAX_ROTATED_LOGS
    for log_to_remove in rotated_logs.iter().skip(MAX_ROTATED_LOGS) {
        fs::remove_file(log_to_remove.path())?;
    }

    Ok(())
}

/// Initialize logging to both console and file
pub fn setup_logging(verbose: bool) -> Result<()> {
    let log_level = if verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    // Get log file path
    let mut log_path = dirs::get_log_dir()?;
    log_path.push("aw-notify.log");

    // Rotate log file if needed before opening
    rotate_log_if_needed(&log_path)?;

    // Create log file
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    // Configure fern to log to both console and file
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{} {} {}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log_level)
        .chain(std::io::stderr())
        .chain(log_file)
        .apply()
        .map_err(|e| anyhow!("Failed to initialize logger: {}", e))?;

    log::info!("Logging initialized (log file: {:?})", log_path);
    Ok(())
}
