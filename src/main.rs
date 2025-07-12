use anyhow::Result;
use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use aws_types::region::Region;
use chrono::{Datelike, Timelike, Utc};
use clap::Parser;
use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;
use tokio::sync::mpsc;
use tokio::time::{interval, Instant};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(name = "lmbd")]
#[command(about = "Log rotation daemon with S3 upload")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.json")]
    config: PathBuf,
    
    /// Directory to monitor for log files
    #[arg(short, long)]
    watch_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    /// Directory to monitor for log files
    pub watch_directory: PathBuf,
    
    /// Log rotation duration in seconds
    pub rotation_duration_seconds: u64,
    
    /// S3 bucket name
    pub s3_bucket: String,
    
    /// File patterns to monitor (regex patterns)
    pub file_patterns: Vec<String>,
    
    /// S3 region (optional, defaults to us-east-1)
    pub s3_region: Option<String>,
}

#[derive(Debug)]
struct FileInfo {
    path: PathBuf,
    pattern_index: usize,
    last_rotation: Instant,
}

struct LogRotationDaemon {
    config: Config,
    s3_client: S3Client,
    file_patterns: Vec<Regex>,
    monitored_files: HashMap<PathBuf, FileInfo>,
}

impl LogRotationDaemon {
    async fn new(config: Config) -> Result<Self> {
        // Setup AWS S3 client
        let region = Region::new(config.s3_region.clone().unwrap_or_else(|| "us-east-1".to_string()));
        let aws_config = aws_config::defaults(BehaviorVersion::latest())
            .region(region)
            .load()
            .await;
        let s3_client = S3Client::new(&aws_config);

        // Compile regex patterns
        let file_patterns = config
            .file_patterns
            .iter()
            .map(|pattern| Regex::new(pattern))
            .collect::<Result<Vec<_>, regex::Error>>()?;

        Ok(Self {
            config,
            s3_client,
            file_patterns,
            monitored_files: HashMap::new(),
        })
    }

    fn matches_pattern(&self, path: &Path) -> Option<usize> {
        let filename = path.file_name()?.to_str()?;
        self.file_patterns
            .iter()
            .position(|pattern| pattern.is_match(filename))
    }

    async fn rotate_and_upload(&mut self, file_path: &Path) -> Result<()> {
        let timestamp = Utc::now();
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?;

        // Create S3 object key with timestamp path: YYYY/MM/DD/T/HH/MM/filename
        let object_key = format!(
            "{:04}/{:02}/{:02}/T/{:02}/{:02}/{}",
            timestamp.year(),
            timestamp.month(),
            timestamp.day(),
            timestamp.hour(),
            timestamp.minute(),
            filename
        );

        info!("Rotating log file: {} -> s3://{}/{}", 
              file_path.display(), self.config.s3_bucket, object_key);

        // Read the file content
        let file_content = fs::read(file_path).await?;

        // Upload to S3
        self.s3_client
            .put_object()
            .bucket(&self.config.s3_bucket)
            .key(&object_key)
            .body(aws_sdk_s3::primitives::ByteStream::from(file_content))
            .send()
            .await?;

        info!("Successfully uploaded {} to S3", object_key);

        // Truncate the original file (rotate it)
        fs::write(file_path, "").await?;

        Ok(())
    }

    async fn check_and_rotate_files(&mut self) -> Result<()> {
        let now = Instant::now();
        let rotation_duration = Duration::from_secs(self.config.rotation_duration_seconds);

        let files_to_rotate: Vec<PathBuf> = self
            .monitored_files
            .iter()
            .filter(|(_, file_info)| now.duration_since(file_info.last_rotation) >= rotation_duration)
            .map(|(path, _)| path.clone())
            .collect();

        for file_path in files_to_rotate {
            if let Err(e) = self.rotate_and_upload(&file_path).await {
                error!("Failed to rotate file {}: {}", file_path.display(), e);
            } else {
                // Update the last rotation time
                if let Some(file_info) = self.monitored_files.get_mut(&file_path) {
                    file_info.last_rotation = now;
                }
            }
        }

        Ok(())
    }

    async fn scan_directory(&mut self) -> Result<()> {
        let mut entries = fs::read_dir(&self.config.watch_directory).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            
            if path.is_file() {
                if let Some(pattern_index) = self.matches_pattern(&path) {
                    if !self.monitored_files.contains_key(&path) {
                        info!("Found matching file: {}", path.display());
                        self.monitored_files.insert(
                            path.clone(),
                            FileInfo {
                                path: path.clone(),
                                pattern_index,
                                last_rotation: Instant::now(),
                            },
                        );
                    }
                }
            }
        }

        Ok(())
    }

    async fn run(&mut self) -> Result<()> {
        info!("Starting log rotation daemon");
        info!("Watching directory: {}", self.config.watch_directory.display());
        info!("Rotation interval: {} seconds", self.config.rotation_duration_seconds);
        info!("S3 bucket: {}", self.config.s3_bucket);

        // Initial scan of the directory
        self.scan_directory().await?;

        // Set up file system watcher
        let (tx, mut rx) = mpsc::channel(100);
        let watch_dir = self.config.watch_directory.clone();
        
        tokio::spawn(async move {
            let mut watcher = RecommendedWatcher::new(
                move |res| {
                    if let Err(e) = tx.blocking_send(res) {
                        error!("Failed to send file event: {}", e);
                    }
                },
                NotifyConfig::default(),
            ).expect("Failed to create file watcher");

            watcher
                .watch(&watch_dir, RecursiveMode::NonRecursive)
                .expect("Failed to watch directory");

            // Keep the watcher alive
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });

        // Set up rotation timer
        let mut rotation_timer = interval(Duration::from_secs(self.config.rotation_duration_seconds));
        
        loop {
            tokio::select! {
                _ = rotation_timer.tick() => {
                    if let Err(e) = self.check_and_rotate_files().await {
                        error!("Error during rotation check: {}", e);
                    }
                }
                
                event = rx.recv() => {
                    match event {
                        Some(Ok(event)) => {
                            info!("File system event: {:?}", event);
                            // Re-scan directory when files change
                            if let Err(e) = self.scan_directory().await {
                                error!("Error scanning directory: {}", e);
                            }
                        }
                        Some(Err(e)) => {
                            error!("File watcher error: {}", e);
                        }
                        None => break,
                    }
                }
            }
        }

        Ok(())
    }
}

async fn load_config(config_path: &Path) -> Result<Config> {
    let config_content = fs::read_to_string(config_path).await?;
    let config: Config = serde_json::from_str(&config_content)?;
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    // Load configuration
    let mut config = load_config(&args.config).await?;
    
    // Override watch directory if provided via CLI
    if let Some(watch_dir) = args.watch_dir {
        config.watch_directory = watch_dir;
    }

    // Create and run the daemon
    let mut daemon = LogRotationDaemon::new(config).await?;
    daemon.run().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_config_loading() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test_config.json");
        
        let config_content = r#"
        {
            "watch_directory": "/tmp/test",
            "rotation_duration_seconds": 3600,
            "s3_bucket": "test-bucket",
            "file_patterns": ["test\\.log$"],
            "s3_region": "us-west-2"
        }
        "#;
        
        fs::write(&config_path, config_content).unwrap();
        let config = load_config(&config_path).await.unwrap();
        
        assert_eq!(config.watch_directory, PathBuf::from("/tmp/test"));
        assert_eq!(config.rotation_duration_seconds, 3600);
        assert_eq!(config.s3_bucket, "test-bucket");
        assert_eq!(config.file_patterns, vec!["test\\.log$"]);
        assert_eq!(config.s3_region, Some("us-west-2".to_string()));
    }

    #[tokio::test]
    async fn test_pattern_matching() {
        let config = Config {
            watch_directory: PathBuf::from("/tmp"),
            rotation_duration_seconds: 60,
            s3_bucket: "test".to_string(),
            file_patterns: vec![
                "errors\\.log$".to_string(),
                "app-.*\\.log$".to_string(),
            ],
            s3_region: None,
        };

        // This test doesn't need AWS credentials since we're not testing the full daemon
        // We'll just test pattern matching logic
        let patterns: Vec<Regex> = config
            .file_patterns
            .iter()
            .map(|pattern| Regex::new(pattern).unwrap())
            .collect();

        // Test pattern matching function
        let matches_pattern = |path: &Path| -> Option<usize> {
            let filename = path.file_name()?.to_str()?;
            patterns.iter().position(|pattern| pattern.is_match(filename))
        };

        // Test cases
        assert!(matches_pattern(Path::new("/tmp/errors.log")).is_some());
        assert!(matches_pattern(Path::new("/tmp/app-main.log")).is_some());
        assert!(matches_pattern(Path::new("/tmp/app-worker.log")).is_some());
        assert!(matches_pattern(Path::new("/tmp/random.txt")).is_none());
        assert!(matches_pattern(Path::new("/tmp/access.log")).is_none());
    }

    #[test]
    fn test_s3_key_generation() {
        use chrono::{TimeZone, Utc};
        
        // Create a fixed timestamp for consistent testing
        let timestamp = Utc.with_ymd_and_hms(2025, 12, 15, 3, 30, 45).unwrap();
        let filename = "errors.log";

        let object_key = format!(
            "{:04}/{:02}/{:02}/T/{:02}/{:02}/{}",
            timestamp.year(),
            timestamp.month(),
            timestamp.day(),
            timestamp.hour(),
            timestamp.minute(),
            filename
        );

        assert_eq!(object_key, "2025/12/15/T/03/30/errors.log");
    }
}
