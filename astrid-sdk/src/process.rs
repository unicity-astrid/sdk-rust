//! Sandboxed host process spawning.
//!
//! Backed by `astrid:process/host@1.0.0`. Synchronous spawns return
//! [`Output`]; background spawns return a RAII [`Process`] handle that
//! drops the kernel-side resource — and reaps the child — when the
//! handle goes out of scope. Per-capsule cap: 8 concurrent background
//! processes.

use super::*;

/// Exit information for a terminated process.
///
/// Mirrors `astrid:process/host.exit-info`. Distinguishes "normal exit
/// with code N" from "killed by signal S" so capsules can branch on
/// SIGKILL vs SIGTERM vs SIGSEGV without parsing exit codes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExitInfo {
    /// Normal exit code if the process exited normally; `None` if
    /// killed by signal or the platform didn't surface one.
    pub exit_code: Option<i32>,
    /// Signal that killed the process (Unix), if any.
    pub signal: Option<i32>,
}

impl ExitInfo {
    fn from_wit(info: wit_process::ExitInfo) -> Self {
        Self {
            exit_code: info.exit_code,
            signal: info.signal,
        }
    }

    /// Whether the process exited with a zero status code.
    #[must_use]
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Result of a synchronous [`spawn`].
#[derive(Debug, Clone)]
pub struct Output {
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// How the process terminated.
    pub exit: ExitInfo,
}

/// Buffered logs and status from a background process. Returned by
/// [`Process::read_logs`].
#[derive(Debug, Clone)]
pub struct Logs {
    /// New stdout output since the last read.
    pub stdout: String,
    /// New stderr output since the last read.
    pub stderr: String,
    /// Whether the process is still running.
    pub running: bool,
    /// Exit info if the process has terminated.
    pub exit: Option<ExitInfo>,
}

/// Result of [`Process::kill`]. Drains the final stdout/stderr buffers
/// into this struct so the caller doesn't lose terminal output that
/// hadn't yet been read.
#[derive(Debug, Clone)]
pub struct KillResult {
    /// Whether the process was successfully killed.
    pub killed: bool,
    /// Exit info if available.
    pub exit: Option<ExitInfo>,
    /// Final buffered stdout.
    pub stdout: String,
    /// Final buffered stderr.
    pub stderr: String,
}

/// Signals a background process can receive.
///
/// `Term` is for graceful shutdown. Use [`Process::kill`] (sends
/// SIGKILL) for non-graceful termination with log drainage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Term,
    Hup,
    Usr1,
    Usr2,
    Int,
}

impl Signal {
    fn to_wit(self) -> wit_process::ProcessSignal {
        match self {
            Self::Term => wit_process::ProcessSignal::Term,
            Self::Hup => wit_process::ProcessSignal::Hup,
            Self::Usr1 => wit_process::ProcessSignal::Usr1,
            Self::Usr2 => wit_process::ProcessSignal::Usr2,
            Self::Int => wit_process::ProcessSignal::Int,
        }
    }
}

/// Builder for the [`spawn`] / [`spawn_background`] request body. The
/// pre-migration helper took just `(cmd, args)`; this builder allows
/// the new contract's `env`, `cwd`, and `stdin` fields without
/// breaking the simple two-arg path via [`spawn`] / [`spawn_background`].
#[derive(Debug, Clone, Default)]
pub struct Command {
    cmd: String,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
    env: Vec<(String, String)>,
    cwd: Option<String>,
}

impl Command {
    /// Start a builder with the given executable.
    pub fn new(cmd: impl Into<String>) -> Self {
        Self {
            cmd: cmd.into(),
            ..Self::default()
        }
    }

    /// Append a single argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments at once.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set initial stdin bytes piped to the spawned process.
    #[must_use]
    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    /// Add an environment variable.
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Set the working directory (relative to the workspace).
    #[must_use]
    pub fn cwd(mut self, path: impl Into<String>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    fn into_wit(self) -> wit_process::SpawnRequest {
        wit_process::SpawnRequest {
            cmd: self.cmd,
            args: self.args,
            stdin: self.stdin,
            env: self
                .env
                .into_iter()
                .map(|(key, value)| wit_process::EnvVar { key, value })
                .collect(),
            cwd: self.cwd,
        }
    }

    /// Spawn synchronously, blocking until the process exits.
    pub fn spawn(self) -> Result<Output, SysError> {
        let req = self.into_wit();
        let result = wit_process::spawn(&req).map_err(host_err)?;
        Ok(Output {
            stdout: result.stdout,
            stderr: result.stderr,
            exit: ExitInfo::from_wit(result.exit),
        })
    }

    /// Spawn as a background process; returns a [`Process`] handle.
    pub fn spawn_background(self) -> Result<Process, SysError> {
        let req = self.into_wit();
        let inner = wit_process::spawn_background(&req).map_err(host_err)?;
        Ok(Process { inner })
    }
}

/// Synchronous spawn helper — `cmd` plus its args, no env / cwd / stdin.
///
/// Mirrors the pre-migration `process::spawn` shape. For the full
/// builder surface use [`Command`].
pub fn spawn(cmd: &str, args: &[&str]) -> Result<Output, SysError> {
    Command::new(cmd).args(args.iter().copied()).spawn()
}

/// Background spawn helper — `cmd` plus its args, no env / cwd / stdin.
///
/// Returns a [`Process`] whose `Drop` reaps the child. For the full
/// builder surface use [`Command`].
pub fn spawn_background(cmd: &str, args: &[&str]) -> Result<Process, SysError> {
    Command::new(cmd)
        .args(args.iter().copied())
        .spawn_background()
}

/// A running (or recently-terminated) background process.
///
/// Owns the kernel-side `process-handle` resource. Drop reaps the
/// child automatically — capsules don't need explicit `wait` or
/// `kill` calls on the happy path.
///
/// Per-capsule cap: 8 concurrent background processes.
#[derive(Debug)]
pub struct Process {
    inner: wit_process::ProcessHandle,
}

impl Process {
    /// Drain newly-buffered stdout/stderr since the previous call and
    /// report whether the process is still running.
    pub fn read_logs(&self) -> Result<Logs, SysError> {
        let result = self.inner.read_logs().map_err(host_err)?;
        Ok(Logs {
            stdout: result.stdout,
            stderr: result.stderr,
            running: result.running,
            exit: result.exit.map(ExitInfo::from_wit),
        })
    }

    /// Write to the process's stdin. Returns bytes actually written;
    /// capped at 1 MB per call.
    pub fn write_stdin(&self, data: &[u8]) -> Result<u32, SysError> {
        self.inner.write_stdin(data).map_err(host_err)
    }

    /// Close the stdin pipe; the child observes EOF on read.
    pub fn close_stdin(&self) -> Result<(), SysError> {
        self.inner.close_stdin().map_err(host_err)
    }

    /// Send a signal. Fire-and-forget. For graceful shutdown use
    /// [`Signal::Term`]; for non-graceful termination use [`kill`].
    pub fn signal(&self, sig: Signal) -> Result<(), SysError> {
        self.inner.signal(sig.to_wit()).map_err(host_err)
    }

    /// Send SIGKILL and drain remaining stdout/stderr buffers.
    pub fn kill(&self) -> Result<KillResult, SysError> {
        let r = self.inner.kill().map_err(host_err)?;
        Ok(KillResult {
            killed: r.killed,
            exit: r.exit.map(ExitInfo::from_wit),
            stdout: r.stdout,
            stderr: r.stderr,
        })
    }

    /// Wait for the process to exit.
    ///
    /// `timeout` of `None` waits indefinitely; a `Some` bounds the
    /// wait. Returns the [`ExitInfo`] on exit, or a host-side
    /// `wait-timeout` error if the timeout elapses first.
    pub fn wait(&self, timeout: Option<std::time::Duration>) -> Result<ExitInfo, SysError> {
        let ms = timeout.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        self.inner
            .wait(ms)
            .map(ExitInfo::from_wit)
            .map_err(host_err)
    }

    /// Wait for the process to exit AND drain remaining stdout / stderr
    /// buffers atomically. Mirrors
    /// [`std::process::Child::wait_with_output`].
    pub fn wait_with_output(
        &self,
        timeout: Option<std::time::Duration>,
    ) -> Result<Output, SysError> {
        let ms = timeout.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        let result = self.inner.wait_with_output(ms).map_err(host_err)?;
        Ok(Output {
            stdout: result.stdout,
            stderr: result.stderr,
            exit: ExitInfo::from_wit(result.exit),
        })
    }

    /// The OS-level PID of the process. Returns `closed` if the
    /// process has already been reaped.
    pub fn os_pid(&self) -> Result<u32, SysError> {
        self.inner.os_pid().map_err(host_err)
    }
}
