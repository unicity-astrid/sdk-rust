//\! Sandboxed host process spawning.

use super::*;
use serde::Deserialize;

/// Result returned from a spawned host process.
#[derive(Debug, Deserialize)]
pub struct ProcessResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Spawns a native host process (blocks until completion).
/// The Capsule must have the `host_process` capability granted for this command.
pub fn spawn(cmd: &str, args: &[&str]) -> Result<ProcessResult, SysError> {
    let request = wit_types::SpawnRequest {
        cmd: cmd.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
    };
    let result = wit_process::spawn(&request).map_err(SysError::HostError)?;
    Ok(ProcessResult {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
    })
}

// -------------------------------------------------------------------
// Background process management
// -------------------------------------------------------------------

/// Handle returned when a background process is spawned.
#[derive(Debug, Deserialize)]
pub struct BackgroundProcessHandle {
    /// Opaque handle ID (not an OS PID).
    pub(crate) id: u64,
}

impl BackgroundProcessHandle {
    /// Returns the opaque handle ID for this process.
    pub fn id(&self) -> u64 {
        self.id
    }
}

/// Buffered logs and status from a background process.
#[derive(Debug, Deserialize)]
pub struct ProcessLogs {
    /// New stdout output since the last read.
    pub stdout: String,
    /// New stderr output since the last read.
    pub stderr: String,
    /// Whether the process is still running.
    pub running: bool,
    /// Exit code if the process has exited.
    pub exit_code: Option<i32>,
}

/// Result from killing a background process.
#[derive(Debug, Deserialize)]
pub struct KillResult {
    /// Whether the process was successfully killed.
    pub killed: bool,
    /// Exit code of the terminated process.
    pub exit_code: Option<i32>,
    /// Any remaining buffered stdout.
    pub stdout: String,
    /// Any remaining buffered stderr.
    pub stderr: String,
}

/// Spawn a background host process.
///
/// Returns an opaque handle that can be used with [`read_logs`] and
/// [`kill`]. The process runs sandboxed with piped stdout/stderr.
pub fn spawn_background(cmd: &str, args: &[&str]) -> Result<BackgroundProcessHandle, SysError> {
    let request = wit_types::SpawnRequest {
        cmd: cmd.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
    };
    let result = wit_process::spawn_background(&request).map_err(SysError::HostError)?;
    Ok(BackgroundProcessHandle { id: result.id })
}

/// Read buffered output from a background process.
///
/// Each call drains the buffer and returns only NEW output since the
/// last read. Also reports whether the process is still running.
pub fn read_logs(id: u64) -> Result<ProcessLogs, SysError> {
    let result = wit_process::read_logs(id).map_err(SysError::HostError)?;
    Ok(ProcessLogs {
        stdout: result.stdout,
        stderr: result.stderr,
        running: result.running,
        exit_code: result.exit_code,
    })
}

/// Kill a background process and release its resources.
///
/// Returns any remaining buffered output along with the exit code.
pub fn kill(id: u64) -> Result<KillResult, SysError> {
    let result = wit_process::kill(id).map_err(SysError::HostError)?;
    Ok(KillResult {
        killed: result.killed,
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
    })
}
