//! Sandboxed subprocess execution with rlimits.
//!
//! Spawns a child process with:
//! - RLIMIT_AS (virtual memory cap)
//! - RLIMIT_CPU (CPU seconds cap)
//! - Wall-time timeout via tokio::time::timeout
//! - stdin = input JSON, stdout = output JSON

use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

/// Output from a subprocess execution (matches VEIL's SubprocessOutput).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubprocessOutput {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub emitted_events: Vec<serde_json::Value>,
}

/// Configuration for resource limits on the subprocess.
#[derive(Debug, Clone)]
pub struct RLimits {
    /// Memory limit in bytes (RLIMIT_AS).
    pub memory_bytes: u64,
    /// CPU time limit in seconds (RLIMIT_CPU).
    pub cpu_seconds: u64,
}

/// Execute a subprocess with sandboxed resource limits.
///
/// # Arguments
/// * `binary_path` - Path to the compiled binary
/// * `input_json` - JSON string to pass on stdin
/// * `timeout_ms` - Wall-time timeout in milliseconds
/// * `memory_limit_mb` - Virtual memory limit in MB
///
/// # Returns
/// * `SubprocessOutput` parsed from the process stdout
pub async fn run_sandboxed(
    binary_path: &str,
    input_json: &str,
    timeout_ms: u64,
    memory_limit_mb: u64,
) -> SubprocessOutput {
    let rlimits = RLimits {
        memory_bytes: memory_limit_mb * 1024 * 1024,
        cpu_seconds: (timeout_ms / 1000).max(1) + 5, // CPU limit = wall + 5s grace
    };

    let wall_timeout = Duration::from_millis(timeout_ms);

    let result = timeout(wall_timeout, spawn_with_limits(binary_path, input_json, &rlimits)).await;

    match result {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => SubprocessOutput {
            success: false,
            output: None,
            error: Some(format!("subprocess error: {}", e)),
            emitted_events: vec![],
        },
        Err(_) => SubprocessOutput {
            success: false,
            output: None,
            error: Some(format!(
                "subprocess timed out after {}ms",
                timeout_ms
            )),
            emitted_events: vec![],
        },
    }
}

/// Spawn the subprocess with rlimits applied via pre_exec.
async fn spawn_with_limits(
    binary_path: &str,
    input_json: &str,
    rlimits: &RLimits,
) -> Result<SubprocessOutput, String> {
    let memory_limit = rlimits.memory_bytes;
    let cpu_limit = rlimits.cpu_seconds;

    let mut child = unsafe {
        Command::new(binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .pre_exec(move || {
                use nix::sys::resource::{setrlimit, Resource};

                // Set virtual memory limit
                setrlimit(Resource::RLIMIT_AS, memory_limit, memory_limit)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

                // Set CPU time limit
                setrlimit(Resource::RLIMIT_CPU, cpu_limit, cpu_limit)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

                Ok(())
            })
            .spawn()
            .map_err(|e| format!("failed to spawn subprocess: {}", e))?
    };

    // Write input to stdin
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input_json.as_bytes())
            .await
            .map_err(|e| format!("failed to write stdin: {}", e))?;
        // Close stdin to signal EOF
        drop(stdin);
    }

    // Wait for process to complete
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("failed to wait for subprocess: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit_code = output.status.code().unwrap_or(-1);

        // Killed by signal (e.g. SIGKILL from OOM, SIGXCPU from CPU limit)
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = output.status.signal() {
                return Ok(SubprocessOutput {
                    success: false,
                    output: None,
                    error: Some(format!(
                        "process killed by signal {} ({}). stderr: {}",
                        signal,
                        match signal {
                            9 => "SIGKILL - likely OOM",
                            24 => "SIGXCPU - CPU time exceeded",
                            _ => "unknown signal",
                        },
                        stderr.chars().take(500).collect::<String>()
                    )),
                    emitted_events: vec![],
                });
            }
        }

        return Ok(SubprocessOutput {
            success: false,
            output: None,
            error: Some(format!(
                "exit code {}. stderr: {}",
                exit_code,
                stderr.chars().take(500).collect::<String>()
            )),
            emitted_events: vec![],
        });
    }

    // Parse stdout as JSON SubprocessOutput
    let stdout = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str::<SubprocessOutput>(&stdout) {
        Ok(parsed) => Ok(parsed),
        Err(_) => {
            // If the output isn't a SubprocessOutput JSON, wrap the raw output
            Ok(SubprocessOutput {
                success: true,
                output: Some(serde_json::Value::String(stdout.into_owned())),
                error: None,
                emitted_events: vec![],
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_echo() {
        // Test with a simple echo command
        let output = run_sandboxed(
            "/bin/echo",
            "hello",
            5000,
            256,
        ).await;
        // echo doesn't read stdin or produce JSON, so output is the raw string
        assert!(output.success);
    }

    #[tokio::test]
    async fn test_timeout() {
        // /usr/bin/yes outputs "y\n" forever, will exceed wall timeout
        let output = run_sandboxed(
            "/usr/bin/yes",
            "",
            50, // 50ms timeout
            256,
        ).await;
        assert!(!output.success, "expected failure but got: {:?}", output);
        let err = output.error.unwrap_or_default();
        assert!(
            err.contains("timed out") || err.contains("signal") || err.contains("exit code"),
            "expected timeout/signal/error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_nonexistent_binary() {
        let output = run_sandboxed(
            "/nonexistent/binary",
            "{}",
            5000,
            256,
        ).await;
        assert!(!output.success);
        assert!(output.error.as_ref().unwrap().contains("spawn"));
    }
}
