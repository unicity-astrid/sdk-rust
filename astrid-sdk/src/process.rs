//! Sandboxed host process spawning.
//!
//! Backed by `astrid:process/host@1.1.0` (requires a host that serves
//! it — astrid >= 0.9.1). Synchronous spawns return
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
    /// SIGSTOP — pause the child (cannot be caught). Lets a supervisor
    /// throttle a runaway without killing it.
    Stop,
    /// SIGCONT — resume a paused child.
    Cont,
}

impl Signal {
    fn to_wit(self) -> wit_process::ProcessSignal {
        match self {
            Self::Term => wit_process::ProcessSignal::Term,
            Self::Hup => wit_process::ProcessSignal::Hup,
            Self::Usr1 => wit_process::ProcessSignal::Usr1,
            Self::Usr2 => wit_process::ProcessSignal::Usr2,
            Self::Int => wit_process::ProcessSignal::Int,
            Self::Stop => wit_process::ProcessSignal::Stop,
            Self::Cont => wit_process::ProcessSignal::Cont,
        }
    }
}

/// How an injected read-only file is exposed to the spawned child.
///
/// Both modes expose the SAME bytes read-only — the child (and any
/// subprocess it spawns) cannot modify them, and neither can the spawning
/// principal's `fs` surface. They differ only in how the child finds the
/// file, chosen to match the target program's config mechanism. See
/// [`Command::inject_env_file`] / [`Command::inject_file_at`].
#[derive(Debug, Clone)]
pub enum InjectionPlacement {
    /// The host materializes the bytes at a host-owned path (outside every
    /// VFS mount) and sets the named environment variable on the child to
    /// that path. The host owns the path — there is no caller-chosen target.
    /// Works on **Linux and macOS** (the OS-agnostic mode); use it for
    /// programs whose enforced config tier is reachable via an env-redirected
    /// file. The `String` is the env-var name.
    EnvPointer(String),
    /// The host ro-binds the bytes at this absolute in-sandbox path (the
    /// mount point is created, so `path` need not pre-exist). **Linux only**
    /// — rejected on macOS with `invalid-input`, since Seatbelt has no mount
    /// namespace and materializing at a caller-named host path would be an
    /// arbitrary host write. Use it for programs whose enforced tier is a
    /// fixed path with no env redirect.
    FixedPath(String),
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
    /// Read-only files injected into the child, honored by all spawn modes.
    file_injections: Vec<(Vec<u8>, InjectionPlacement)>,
    // Persistent-tier knobs — honored ONLY by [`spawn_persistent`], ignored
    // by [`spawn`] / [`spawn_background`] (per the WIT contract).
    label: Option<String>,
    keep_stdin_open: bool,
    overflow: Option<OverflowPolicy>,
    log_ring_bytes: Option<u32>,
    max_lifetime: Option<std::time::Duration>,
    idle_timeout: Option<std::time::Duration>,
    exit_retention: Option<std::time::Duration>,
    limits: Option<ResourceLimits>,
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

    /// Inject `content` into the child as a read-only file the child cannot
    /// modify, exposed via the named environment variable pointing at a
    /// host-owned path ([`InjectionPlacement::EnvPointer`]). OS-agnostic
    /// (Linux and macOS). The host owns the bytes' integrity and exposure;
    /// `content` is opaque to it. Honored by all spawn modes.
    #[must_use]
    pub fn inject_env_file(
        mut self,
        env_var: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        self.file_injections.push((
            content.into(),
            InjectionPlacement::EnvPointer(env_var.into()),
        ));
        self
    }

    /// Inject `content` into the child as a read-only file ro-bound at the
    /// absolute in-sandbox `path` ([`InjectionPlacement::FixedPath`]).
    /// **Linux only** — rejected on macOS with `invalid-input`. Honored by
    /// all spawn modes.
    #[must_use]
    pub fn inject_file_at(mut self, path: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        self.file_injections
            .push((content.into(), InjectionPlacement::FixedPath(path.into())));
        self
    }

    /// Inject `content` with an explicit [`InjectionPlacement`]. Lower-level
    /// form of [`inject_env_file`](Self::inject_env_file) /
    /// [`inject_file_at`](Self::inject_file_at) for callers that compute the
    /// placement dynamically.
    #[must_use]
    pub fn inject_file(
        mut self,
        content: impl Into<Vec<u8>>,
        placement: InjectionPlacement,
    ) -> Self {
        self.file_injections.push((content.into(), placement));
        self
    }

    /// Operator-readable label surfaced in [`list`] / [`PersistentProcess::status`].
    /// Persistent-tier only. `None` (default) derives a label from `cmd`.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Keep a writable stdin pipe open after the optional `stdin` prelude so
    /// [`PersistentProcess::write_stdin`] works across invocations (REPL /
    /// `psql` / MCP-stdio children). Persistent-tier only.
    #[must_use]
    pub fn keep_stdin_open(mut self, keep: bool) -> Self {
        self.keep_stdin_open = keep;
        self
    }

    /// Per-stream ring overflow policy. Persistent-tier only.
    /// `None` (default) → [`OverflowPolicy::DropOldest`].
    #[must_use]
    pub fn overflow(mut self, policy: OverflowPolicy) -> Self {
        self.overflow = Some(policy);
        self
    }

    /// Per-stream output ring capacity in bytes (stdout and stderr each).
    /// Persistent-tier only. Host-clamped to the profile ceiling.
    #[must_use]
    pub fn log_ring_bytes(mut self, bytes: u32) -> Self {
        self.log_ring_bytes = Some(bytes);
        self
    }

    /// Wall-clock lifetime ceiling from spawn (SIGTERM → grace → SIGKILL on
    /// expiry). Persistent-tier only. Any value is clamped DOWN to the host
    /// ceiling — a guest cannot request an unbounded lifetime.
    #[must_use]
    pub fn max_lifetime(mut self, dur: std::time::Duration) -> Self {
        self.max_lifetime = Some(dur);
        self
    }

    /// Reap an untouched persistent process (no read / wait / signal / write)
    /// after this idle interval — the primary anti-leak backstop for
    /// spawn-and-forget. Persistent-tier only.
    #[must_use]
    pub fn idle_timeout(mut self, dur: std::time::Duration) -> Self {
        self.idle_timeout = Some(dur);
        self
    }

    /// After a persistent process exits, retain its id + drained log tail this
    /// long before the host auto-reaps it. Persistent-tier only.
    #[must_use]
    pub fn exit_retention(mut self, dur: std::time::Duration) -> Self {
        self.exit_retention = Some(dur);
        self
    }

    /// Per-child OS resource ceilings (applies to every tier).
    #[must_use]
    pub fn limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = Some(limits);
        self
    }

    fn into_wit(self) -> wit_process::SpawnRequest {
        let ms = |d: std::time::Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
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
            file_injections: self
                .file_injections
                .into_iter()
                .map(|(content, placement)| wit_process::FileInjection {
                    content,
                    placement: match placement {
                        InjectionPlacement::EnvPointer(v) => {
                            wit_process::InjectionPlacement::EnvPointer(v)
                        }
                        InjectionPlacement::FixedPath(p) => {
                            wit_process::InjectionPlacement::FixedPath(p)
                        }
                    },
                })
                .collect(),
            limits: self.limits.map(ResourceLimits::into_wit),
            label: self.label,
            keep_stdin_open: self.keep_stdin_open.then_some(true),
            overflow: self.overflow.map(OverflowPolicy::to_wit),
            log_ring_bytes: self.log_ring_bytes,
            max_lifetime_ms: self.max_lifetime.map(ms),
            idle_timeout_ms: self.idle_timeout.map(ms),
            exit_retention_ms: self.exit_retention.map(ms),
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

    /// Spawn a PERSISTENT background process whose lifetime is decoupled from
    /// the calling instance. Returns a [`PersistentProcess`] keyed by an
    /// opaque [`ProcessId`] that ANY later invocation of the same capsule
    /// under the same principal can [`attach`] to — unlike [`spawn_background`],
    /// it survives the pooled instance being reset between tool invocations.
    ///
    /// Gated on `host_process`. Counts against the per-principal concurrent
    /// cap (shared with `spawn_background`) and the retained-id cap.
    ///
    /// Beyond the `host_process` check, this can also fail when the
    /// invocation has no authenticated principal in scope (the owner-fallback
    /// case): a persistent id must be scoped to a real principal, so an
    /// unauthenticated path is refused with a host error rather than sharing a
    /// `default` namespace that `list`/`status_many` would enumerate.
    pub fn spawn_persistent(self) -> Result<PersistentProcess, SysError> {
        let req = self.into_wit();
        let id = wit_process::spawn_persistent(&req).map_err(host_err)?;
        Ok(PersistentProcess { id: ProcessId(id) })
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

// ============================================================
// PERSISTENT TIER
//
// A persistent process survives the pooled, stateless instance that spawned
// it (unlike the ephemeral [`Process`], whose kernel resource is reaped on
// instance reset). It is keyed by an opaque [`ProcessId`] that any later
// invocation of the same capsule+principal can [`attach`] to.
// ============================================================

/// Opaque, principal-scoped identity for a persistent process that survives
/// instance churn. Persist it (e.g. in KV) to reattach later — `Serialize` /
/// `Deserialize` let it ride in a state struct alongside a [`LogCursor`].
/// Treat as opaque — never parse or synthesize it. (A leaked id is inert
/// across the principal/capsule boundary: the host re-checks ownership on
/// every id-keyed call, so it is a handle, not a credential.)
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ProcessId(String);

impl ProcessId {
    /// The id as a string slice (e.g. to store it for later [`attach`]).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume into the owned `String`.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl core::fmt::Display for ProcessId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ProcessId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ProcessId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Lifecycle phase of a persistent process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessPhase {
    /// Spawn accepted; child not yet confirmed running.
    Starting,
    /// Child is running.
    Running,
    /// Terminated; exit info available; logs readable until released / TTL.
    Exited,
}

impl ProcessPhase {
    fn from_wit(p: wit_process::ProcessPhase) -> Self {
        match p {
            wit_process::ProcessPhase::Starting => Self::Starting,
            wit_process::ProcessPhase::Running => Self::Running,
            wit_process::ProcessPhase::Exited => Self::Exited,
        }
    }
}

/// Which stream a cursor read addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
}

impl LogStream {
    fn to_wit(self) -> wit_process::LogStream {
        match self {
            Self::Stdout => wit_process::LogStream::Stdout,
            Self::Stderr => wit_process::LogStream::Stderr,
        }
    }
}

/// Per-stream ring overflow policy for a persistent process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowPolicy {
    /// Drop oldest bytes to make room; loss surfaces as `bytes_dropped`.
    DropOldest,
    /// Stop draining the pipe when full so the child blocks on write —
    /// correct for REPL / MCP-stdio children where dropping corrupts framing.
    Backpressure,
}

impl OverflowPolicy {
    fn to_wit(self) -> wit_process::OverflowPolicy {
        match self {
            Self::DropOldest => wit_process::OverflowPolicy::DropOldest,
            Self::Backpressure => wit_process::OverflowPolicy::Backpressure,
        }
    }
}

/// Per-child OS resource ceilings. `None` per field → the principal's profile
/// default. (Host enforcement is not yet wired — see the WIT.)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Address-space / RSS ceiling.
    pub max_memory_bytes: Option<u64>,
    /// Cumulative CPU-time ceiling in whole seconds.
    pub max_cpu_secs: Option<u64>,
    /// Max concurrent child PIDs (a fork-bomb fence).
    pub max_pids: Option<u32>,
    /// Max open file descriptors.
    pub max_open_files: Option<u32>,
}

impl ResourceLimits {
    fn into_wit(self) -> wit_process::ResourceLimits {
        wit_process::ResourceLimits {
            max_memory_bytes: self.max_memory_bytes,
            max_cpu_secs: self.max_cpu_secs,
            max_pids: self.max_pids,
            max_open_files: self.max_open_files,
        }
    }
}

/// Opaque, resumable cursor into a persistent process's log stream. Use
/// [`LogCursor::start`] for the first read; pass [`LogChunk::next`] back to
/// resume exactly where you left off. Treat as opaque. `Serialize` /
/// `Deserialize` let a capsule persist the cursor (e.g. in KV) and resume
/// `read_since` from the same position in a later invocation.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LogCursor {
    token: Option<String>,
}

impl LogCursor {
    /// A cursor positioned at the oldest retained byte.
    #[must_use]
    pub fn start() -> Self {
        Self { token: None }
    }

    fn from_wit(c: wit_process::LogCursor) -> Self {
        Self { token: c.token }
    }

    fn to_wit(&self) -> wit_process::LogCursor {
        wit_process::LogCursor {
            token: self.token.clone(),
        }
    }
}

/// A non-draining slice of a persistent process's stream, addressed by cursor.
/// Multiple independent readers each keep their own cursor and observe the
/// full retained stream.
#[derive(Debug, Clone)]
pub struct LogChunk {
    /// Bytes in `[requested-cursor, next)`. Byte-faithful — non-UTF-8 safe.
    pub data: Vec<u8>,
    /// Cursor to pass to the next [`PersistentProcess::read_since`] to resume.
    pub next: LogCursor,
    /// Cumulative bytes evicted before they could be delivered through this
    /// cursor (0 unless the reader fell behind).
    pub bytes_dropped: u64,
    /// `true` once the child exited AND all retained output on this stream was
    /// delivered through this cursor — the clean EOF.
    pub drained_eof: bool,
}

impl LogChunk {
    fn from_wit(c: wit_process::LogChunk) -> Self {
        Self {
            data: c.data,
            next: LogCursor::from_wit(c.next),
            bytes_dropped: c.bytes_dropped,
            drained_eof: c.drained_eof,
        }
    }
}

/// A non-draining status snapshot of one persistent process. Repeated reads
/// never consume log bytes and never mutate process state.
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    /// Reattach key, stable across invocations / instances.
    pub id: ProcessId,
    /// Operator label (or one derived from `cmd`).
    pub label: String,
    /// cmd + args, as the capsule requested it.
    pub command: String,
    /// OS PID while running; `None` once reaped. Advisory only.
    pub os_pid: Option<u32>,
    /// Lifecycle phase.
    pub phase: ProcessPhase,
    /// Present once exited.
    pub exit: Option<ExitInfo>,
    /// Uptime since spawn.
    pub age: std::time::Duration,
    /// Time since the last operation touched this process (drives idle reap).
    pub idle: std::time::Duration,
    /// Bytes currently buffered and drainable (stdout + stderr).
    pub buffered_bytes: u64,
    /// Cumulative bytes evicted from the rings since spawn.
    pub bytes_dropped: u64,
    /// Whether stdin is still open for `write_stdin`.
    pub stdin_open: bool,
    /// Cumulative CPU time consumed. `None` until the host populates it.
    pub cpu: Option<std::time::Duration>,
    /// Peak resident memory. `None` until the host populates it.
    pub mem_bytes_peak: Option<u64>,
}

impl ProcessInfo {
    fn from_wit(i: wit_process::ProcessInfo) -> Self {
        Self {
            id: ProcessId(i.id),
            label: i.label,
            command: i.command,
            os_pid: i.os_pid,
            phase: ProcessPhase::from_wit(i.phase),
            exit: i.exit.map(ExitInfo::from_wit),
            age: std::time::Duration::from_millis(i.age_ms),
            idle: std::time::Duration::from_millis(i.idle_ms),
            buffered_bytes: i.buffered_bytes,
            bytes_dropped: i.bytes_dropped,
            stdin_open: i.stdin_open,
            cpu: i.cpu_ms.map(std::time::Duration::from_millis),
            mem_bytes_peak: i.mem_bytes_peak,
        }
    }
}

/// A handle to a PERSISTENT background process, keyed by its [`ProcessId`].
///
/// Unlike [`Process`], dropping this handle does NOT reap the underlying
/// process — it is a detached view. The process is reaped only by [`stop`](Self::stop),
/// [`release`](Self::release), or the host's idle / max-lifetime /
/// exit-retention TTLs. Obtain one from [`Command::spawn_persistent`] or
/// [`attach`].
#[derive(Debug, Clone)]
pub struct PersistentProcess {
    id: ProcessId,
}

impl PersistentProcess {
    /// The process id — persist it (e.g. in KV) to reattach from a later
    /// invocation via [`attach`].
    #[must_use]
    pub fn id(&self) -> &ProcessId {
        &self.id
    }

    /// Non-draining status snapshot.
    pub fn status(&self) -> Result<ProcessInfo, SysError> {
        wit_process::status(self.id.as_str())
            .map(ProcessInfo::from_wit)
            .map_err(host_err)
    }

    /// Drain newly-buffered stdout/stderr since the previous read, and report
    /// whether the process is still running. Drains the single shared ring —
    /// for independent multi-reader or byte-faithful reads use [`read_since`](Self::read_since).
    pub fn read_logs(&self) -> Result<Logs, SysError> {
        let r = wit_process::read_logs(self.id.as_str()).map_err(host_err)?;
        Ok(Logs {
            stdout: r.stdout,
            stderr: r.stderr,
            running: r.running,
            exit: r.exit.map(ExitInfo::from_wit),
        })
    }

    /// Non-draining, cursor-addressed, byte-faithful read of one stream. Start
    /// with [`LogCursor::start`]; pass [`LogChunk::next`] back to resume.
    pub fn read_since(
        &self,
        stream: LogStream,
        cursor: &LogCursor,
        max_bytes: u32,
    ) -> Result<LogChunk, SysError> {
        wit_process::read_since(
            self.id.as_str(),
            stream.to_wit(),
            &cursor.to_wit(),
            max_bytes,
        )
        .map(LogChunk::from_wit)
        .map_err(host_err)
    }

    /// Write to stdin. Requires the process was spawned with
    /// [`Command::keep_stdin_open`]. Returns bytes written.
    pub fn write_stdin(&self, data: &[u8]) -> Result<u32, SysError> {
        wit_process::write_stdin(self.id.as_str(), data).map_err(host_err)
    }

    /// Close stdin; the child observes EOF on read.
    pub fn close_stdin(&self) -> Result<(), SysError> {
        wit_process::close_stdin(self.id.as_str()).map_err(host_err)
    }

    /// Send a fire-and-forget signal.
    pub fn signal(&self, sig: Signal) -> Result<(), SysError> {
        wit_process::signal(self.id.as_str(), sig.to_wit()).map_err(host_err)
    }

    /// Wait up to `timeout` for the process to exit. Bounded by design — an
    /// unbounded wait would pin the pooled instance. Does NOT reap.
    pub fn wait(&self, timeout: std::time::Duration) -> Result<ExitInfo, SysError> {
        let ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        wit_process::wait(self.id.as_str(), ms)
            .map(ExitInfo::from_wit)
            .map_err(host_err)
    }

    /// Graceful terminal stop: SIGTERM, wait up to `grace`, then SIGKILL, and
    /// REMOVE the id (frees the slot). `grace` of `None` uses the host default.
    /// Consumes the handle. To keep a child's last output, drain it with
    /// [`read_since`](Self::read_since) BEFORE calling `stop`.
    pub fn stop(self, grace: Option<std::time::Duration>) -> Result<ExitInfo, SysError> {
        let ms = grace.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        wit_process::stop(self.id.as_str(), ms)
            .map(ExitInfo::from_wit)
            .map_err(host_err)
    }

    /// Drop the host's retention of an ALREADY-EXITED process — frees the slot
    /// and discards the buffered tail. Errors if still running (use
    /// [`stop`](Self::stop) for that). Consumes the handle.
    pub fn release(self) -> Result<(), SysError> {
        wit_process::release_process(self.id.as_str()).map_err(host_err)
    }
}

/// Reattach to a persistent process by id — e.g. one saved in KV across tool
/// invocations. This just wraps the id; the first id-keyed call validates
/// ownership, so an id that is unknown / not yours / reaped surfaces
/// `no-such-process` on use rather than here.
#[must_use]
pub fn attach(id: impl Into<ProcessId>) -> PersistentProcess {
    PersistentProcess { id: id.into() }
}

/// List the calling capsule + principal's persistent processes, optionally
/// filtered by a label substring. Empty is normal (post-reap recovery signal).
pub fn list(label_filter: Option<&str>) -> Result<Vec<ProcessInfo>, SysError> {
    wit_process::list_processes(label_filter)
        .map(|v| v.into_iter().map(ProcessInfo::from_wit).collect())
        .map_err(host_err)
}

/// Batch status for many ids in one host call. Unknown / unowned ids are
/// simply absent from the result.
pub fn status_many(ids: &[ProcessId]) -> Result<Vec<ProcessInfo>, SysError> {
    let raw: Vec<String> = ids.iter().map(|i| i.0.clone()).collect();
    wit_process::status_many(&raw)
        .map(|v| v.into_iter().map(ProcessInfo::from_wit).collect())
        .map_err(host_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_builders_accumulate_in_order() {
        let cmd = Command::new("agent")
            .inject_env_file("CLAUDE_CODE_MANAGED_SETTINGS_PATH", b"{}".to_vec())
            .inject_file_at("/etc/codex/requirements.toml", b"policy".to_vec())
            .inject_file(
                b"x".to_vec(),
                InjectionPlacement::EnvPointer("GEMINI".into()),
            );

        assert_eq!(cmd.file_injections.len(), 3);
        assert!(matches!(
            cmd.file_injections[0].1,
            InjectionPlacement::EnvPointer(ref v) if v == "CLAUDE_CODE_MANAGED_SETTINGS_PATH"
        ));
        assert_eq!(cmd.file_injections[0].0, b"{}");
        assert!(matches!(
            cmd.file_injections[1].1,
            InjectionPlacement::FixedPath(ref p) if p == "/etc/codex/requirements.toml"
        ));
        assert!(matches!(
            cmd.file_injections[2].1,
            InjectionPlacement::EnvPointer(ref v) if v == "GEMINI"
        ));
    }

    #[test]
    fn no_injection_by_default() {
        assert!(Command::new("agent").file_injections.is_empty());
    }
}
