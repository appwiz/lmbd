# LMBD - Log Rotation Daemon

A Rust daemon that monitors log files, rotates them based on a configurable duration, and uploads the rotated files to Amazon S3.

## Features

- Monitors a directory for files matching specified regex patterns
- Rotates log files based on configurable duration
- Uploads rotated files to S3 with timestamp-based object keys
- Uses the format: `YYYY/MM/DD/T/HH/MM/filename.log` for S3 object keys
- Supports multiple file patterns via regex
- Configurable via JSON configuration file

## Configuration

Create a `config.json` file with the following structure:

```json
{
    "watch_directory": "/var/log/app",
    "rotation_duration_seconds": 1800,
    "s3_bucket": "my-log-bucket", 
    "file_patterns": [
        "errors\\.log$",
        "access\\.log$",
        "app-.*\\.log$"
    ],
    "s3_region": "us-east-1"
}
```

### Configuration Fields

- `watch_directory`: Directory to monitor for log files
- `rotation_duration_seconds`: How often to rotate logs (in seconds)
- `s3_bucket`: S3 bucket name where rotated logs will be uploaded
- `file_patterns`: Array of regex patterns to match log files
- `s3_region`: AWS region for S3 bucket (optional, defaults to us-east-1)

## Usage

```bash
# Run with default config.json
./lmbd

# Specify custom config file
./lmbd --config /path/to/config.json

# Override watch directory
./lmbd --watch-dir /custom/log/dir
```

## AWS Credentials

The daemon uses the AWS SDK which will automatically discover credentials from:
- Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)
- AWS credentials file (~/.aws/credentials)
- IAM roles (when running on EC2)

Required S3 permissions:
- `s3:PutObject` on the target bucket

## Object Key Format

Uploaded files use the following S3 object key format:
```
YYYY/MM/DD/T/HH/MM/filename.log
```

For example, a file `errors.log` rotated on December 15, 2025 at 3:30 AM would be uploaded as:
```
2025/12/15/T/03/30/errors.log
```

## Building

```bash
cargo build --release
```

## Running

```bash
cargo run -- --config config.json
```