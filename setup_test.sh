#!/bin/bash
set -e

# Create test directory
TEST_DIR="/tmp/lmbd_test"
mkdir -p "$TEST_DIR"

# Create test config
cat > test_config.json << EOF
{
    "watch_directory": "$TEST_DIR",
    "rotation_duration_seconds": 10,
    "s3_bucket": "test-log-bucket",
    "file_patterns": [
        "errors\\\\.log\$",
        "access\\\\.log\$",
        "app-.*\\\\.log\$"
    ],
    "s3_region": "us-east-1"
}
EOF

echo "Created test configuration:"
cat test_config.json

# Create test log files
echo "Creating test log files..."
echo "Error log entry 1" > "$TEST_DIR/errors.log"
echo "Access log entry 1" > "$TEST_DIR/access.log"
echo "App log entry 1" > "$TEST_DIR/app-main.log"
echo "Random file" > "$TEST_DIR/random.txt"

echo "Created test files in $TEST_DIR:"
ls -la "$TEST_DIR"

echo "Test setup complete!"
echo "To run the daemon: cargo run -- --config test_config.json"
echo ""
echo "Note: The daemon will fail to upload to S3 without proper AWS credentials,"
echo "but you can see it detect files and attempt the rotation process."