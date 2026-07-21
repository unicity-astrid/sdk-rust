//! Principal-scoped execution of signed core-Wasm workers.
//!
//! This is an experimental, pre-1.0 surface. A capsule may open only worker
//! objects declared as `type = "compute-worker"` in its signed package. Astrid
//! owns admission, scheduling, cancellation, metering, and accounting; the
//! capsule owns the worker algorithm and the bytes in shared memory.

use super::*;

/// Whether a group may run workers concurrently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// One worker and FIFO dispatch for reproducible execution.
    Deterministic,
    /// The admitted worker count may execute concurrently.
    Parallel,
}

impl ExecutionMode {
    fn to_wit(self) -> wit_compute::ExecutionMode {
        match self {
            Self::Deterministic => wit_compute::ExecutionMode::Deterministic,
            Self::Parallel => wit_compute::ExecutionMode::Parallel,
        }
    }

    fn from_wit(value: wit_compute::ExecutionMode) -> Self {
        match value {
            wit_compute::ExecutionMode::Deterministic => Self::Deterministic,
            wit_compute::ExecutionMode::Parallel => Self::Parallel,
        }
    }
}

/// How Astrid resolves the requested worker count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parallelism {
    /// Use useful host parallelism subject to effective policy and reservations.
    Auto,
    /// Admit exactly this many workers or fail.
    Exact(u32),
    /// Admit at least one and no more than this many workers.
    AtMost(u32),
}

impl Parallelism {
    fn to_wit(self) -> wit_compute::Parallelism {
        match self {
            Self::Auto => wit_compute::Parallelism::Auto,
            Self::Exact(count) => wit_compute::Parallelism::Exact(count),
            Self::AtMost(count) => wit_compute::Parallelism::AtMost(count),
        }
    }
}

/// Parameters for opening a compute group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRequest {
    /// Signed `compute-worker` component id from the capsule manifest.
    pub worker: String,
    /// Deterministic or concurrent execution.
    pub mode: ExecutionMode,
    /// Requested worker count.
    pub parallelism: Parallelism,
    /// Initial shared-memory size in 64-KiB pages.
    pub initial_memory_pages: u32,
    /// Maximum shared-memory size in 64-KiB pages. Zero requests automatic
    /// admission from the host's current principal and process-wide capacity.
    pub maximum_memory_pages: u32,
}

impl GroupRequest {
    /// Construct a request. A nonzero maximum is explicit; zero delegates the
    /// maximum to host admission and can be selected more clearly with
    /// [`Self::auto_memory`].
    pub fn new(
        worker: impl Into<String>,
        initial_memory_pages: u32,
        maximum_memory_pages: u32,
    ) -> Self {
        Self {
            worker: worker.into(),
            mode: ExecutionMode::Deterministic,
            parallelism: Parallelism::Exact(1),
            initial_memory_pages,
            maximum_memory_pages,
        }
    }

    /// Ask the host to resolve the effective shared-memory maximum.
    ///
    /// The host intersects its process-wide pool, current reservations, the
    /// verified principal's policy, and the signed worker declaration. Read the
    /// admitted value from [`ComputeGroup::info`] after opening the group.
    #[must_use]
    pub fn auto_memory(mut self) -> Self {
        self.maximum_memory_pages = 0;
        self
    }

    /// Permit concurrent execution and choose a worker-count policy.
    #[must_use]
    pub fn parallel(mut self, parallelism: Parallelism) -> Self {
        self.mode = ExecutionMode::Parallel;
        self.parallelism = parallelism;
        self
    }

    /// Force the reproducible one-worker execution mode.
    #[must_use]
    pub fn deterministic(mut self) -> Self {
        self.mode = ExecutionMode::Deterministic;
        self.parallelism = Parallelism::Exact(1);
        self
    }

    fn to_wit(&self) -> wit_compute::GroupRequest {
        wit_compute::GroupRequest {
            worker: self.worker.clone(),
            mode: self.mode.to_wit(),
            parallelism: self.parallelism.to_wit(),
            initial_memory_pages: self.initial_memory_pages,
            maximum_memory_pages: self.maximum_memory_pages,
        }
    }
}

/// Capsule-defined range and metadata for one worker invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkDescriptor {
    /// Byte offset in shared memory. The first 64 bytes are host-reserved.
    pub offset: u64,
    /// Byte length beginning at `offset`.
    pub length: u64,
    /// Opaque capsule-defined value passed to the worker.
    pub tag: u64,
    /// Optional exact worker index.
    pub worker_index: Option<u32>,
    /// Optional per-job fuel self-limit.
    pub fuel: Option<u64>,
}

impl WorkDescriptor {
    /// Construct a descriptor for the next available worker.
    #[must_use]
    pub const fn new(offset: u64, length: u64, tag: u64) -> Self {
        Self {
            offset,
            length,
            tag,
            worker_index: None,
            fuel: None,
        }
    }

    /// Pin this invocation to an exact worker instance.
    #[must_use]
    pub const fn on_worker(mut self, worker_index: u32) -> Self {
        self.worker_index = Some(worker_index);
        self
    }

    /// Apply a per-job fuel self-limit without widening operator policy.
    #[must_use]
    pub const fn with_fuel(mut self, fuel: u64) -> Self {
        self.fuel = Some(fuel);
        self
    }

    fn to_wit(self) -> wit_compute::WorkDescriptor {
        wit_compute::WorkDescriptor {
            offset: self.offset,
            length: self.length,
            tag: self.tag,
            worker_index: self.worker_index,
            fuel: self.fuel,
        }
    }
}

/// Lifecycle state of a submitted invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

impl JobState {
    fn from_wit(value: wit_compute::JobState) -> Self {
        match value {
            wit_compute::JobState::Queued => Self::Queued,
            wit_compute::JobState::Running => Self::Running,
            wit_compute::JobState::Completed => Self::Completed,
            wit_compute::JobState::Cancelled => Self::Cancelled,
            wit_compute::JobState::Failed => Self::Failed,
        }
    }
}

/// Non-blocking job snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobStatus {
    pub state: JobState,
    pub worker_index: Option<u32>,
}

impl JobStatus {
    fn from_wit(value: wit_compute::JobStatus) -> Self {
        Self {
            state: JobState::from_wit(value.state),
            worker_index: value.worker_index,
        }
    }
}

/// Terminal worker result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobResult {
    pub state: JobState,
    pub worker_index: u32,
    pub worker_status: i32,
    pub fuel_consumed: u64,
    pub elapsed_ns: u64,
}

impl JobResult {
    fn from_wit(value: wit_compute::JobResult) -> Self {
        Self {
            state: JobState::from_wit(value.state),
            worker_index: value.worker_index,
            worker_status: value.worker_status,
            fuel_consumed: value.fuel_consumed,
            elapsed_ns: value.elapsed_ns,
        }
    }
}

/// Host-stamped group accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accounting {
    pub workers_reserved: u32,
    pub memory_bytes_current: u64,
    pub memory_bytes_peak: u64,
    pub jobs_submitted: u64,
    pub jobs_completed: u64,
    pub jobs_cancelled: u64,
    pub jobs_failed: u64,
    pub fuel_consumed: u64,
}

impl Accounting {
    fn from_wit(value: wit_compute::Accounting) -> Self {
        Self {
            workers_reserved: value.workers_reserved,
            memory_bytes_current: value.memory_bytes_current,
            memory_bytes_peak: value.memory_bytes_peak,
            jobs_submitted: value.jobs_submitted,
            jobs_completed: value.jobs_completed,
            jobs_cancelled: value.jobs_cancelled,
            jobs_failed: value.jobs_failed,
            fuel_consumed: value.fuel_consumed,
        }
    }
}

/// Configuration and live counters for an open group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    pub worker: String,
    pub mode: ExecutionMode,
    pub parallelism: u32,
    pub memory_pages: u32,
    pub maximum_memory_pages: u32,
    pub queued_jobs: u32,
    pub running_jobs: u32,
    pub usage: Accounting,
}

impl GroupInfo {
    fn from_wit(value: wit_compute::GroupInfo) -> Self {
        Self {
            worker: value.worker,
            mode: ExecutionMode::from_wit(value.mode),
            parallelism: value.parallelism,
            memory_pages: value.memory_pages,
            maximum_memory_pages: value.maximum_memory_pages,
            queued_jobs: value.queued_jobs,
            running_jobs: value.running_jobs,
            usage: Accounting::from_wit(value.usage),
        }
    }
}

/// One admitted set of worker instances and its shared memory.
#[derive(Debug)]
pub struct ComputeGroup {
    inner: wit_compute::ComputeGroup,
}

impl ComputeGroup {
    /// Validate and atomically admit a signed worker group.
    pub fn open(request: &GroupRequest) -> Result<Self, SysError> {
        let inner = wit_compute::open(&request.to_wit()).map_err(host_err)?;
        Ok(Self { inner })
    }

    /// Read the group's current configuration and accounting.
    pub fn info(&self) -> Result<GroupInfo, SysError> {
        self.inner.info().map(GroupInfo::from_wit).map_err(host_err)
    }

    /// Copy at most 1 MiB from shared memory.
    pub fn read(&self, offset: u64, length: u32) -> Result<Vec<u8>, SysError> {
        self.inner.read(offset, length).map_err(host_err)
    }

    /// Copy at most 1 MiB into shared memory.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<(), SysError> {
        self.inner.write(offset, data).map_err(host_err)
    }

    /// Grow shared memory and return its previous size in pages.
    pub fn grow(&self, delta_pages: u32) -> Result<u32, SysError> {
        self.inner.grow(delta_pages).map_err(host_err)
    }

    /// Queue one invocation without blocking for its result.
    pub fn submit(&self, descriptor: WorkDescriptor) -> Result<ComputeJob, SysError> {
        let inner = self.inner.submit(descriptor.to_wit()).map_err(host_err)?;
        Ok(ComputeJob { inner })
    }

    /// Cancel all queued and running work and reject new submissions.
    pub fn cancel(&self) -> Result<(), SysError> {
        self.inner.cancel().map_err(host_err)
    }
}

/// Observer for one submitted invocation.
///
/// Dropping this handle does not cancel the invocation; use [`Self::cancel`]
/// or cancel/drop the owning [`ComputeGroup`].
#[derive(Debug)]
pub struct ComputeJob {
    inner: wit_compute::Job,
}

impl ComputeJob {
    /// Read current state without blocking.
    pub fn status(&self) -> Result<JobStatus, SysError> {
        self.inner
            .status()
            .map(JobStatus::from_wit)
            .map_err(host_err)
    }

    /// Wait for a terminal result or typed worker failure.
    pub fn join(&self) -> Result<JobResult, SysError> {
        self.inner.join().map(JobResult::from_wit).map_err(host_err)
    }

    /// Request cancellation. This operation is idempotent.
    pub fn cancel(&self) -> Result<(), SysError> {
        self.inner.cancel().map_err(host_err)
    }
}

/// Convenience wrapper for [`ComputeGroup::open`].
pub fn open(request: &GroupRequest) -> Result<ComputeGroup, SysError> {
    ComputeGroup::open(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_preserve_explicit_policy() {
        let request = GroupRequest::new("linux-vcpu", 1024, 8192).parallel(Parallelism::AtMost(8));
        assert_eq!(request.mode, ExecutionMode::Parallel);
        assert_eq!(request.parallelism, Parallelism::AtMost(8));

        let automatic = GroupRequest::new("linux-vcpu", 1024, 8192).auto_memory();
        assert_eq!(automatic.initial_memory_pages, 1024);
        assert_eq!(automatic.maximum_memory_pages, 0);

        let descriptor = WorkDescriptor::new(64, 4096, 7)
            .on_worker(3)
            .with_fuel(1_000_000);
        assert_eq!(descriptor.worker_index, Some(3));
        assert_eq!(descriptor.fuel, Some(1_000_000));
    }
}
