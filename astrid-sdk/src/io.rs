//! Heterogeneous readiness polling for capsule host resources.
//!
//! A [`Pollable`] borrows its source resource host-side, so it must be dropped
//! before the listener, stream, subscription, or process handle that created
//! it. Keeping both values in the same scope naturally enforces that order.

use astrid_sys::astrid::io::poll as wit_poll;

use crate::{SysError, host_err};

/// Maximum number of handles accepted by one [`poll`] call.
pub const MAX_POLLABLES_PER_CALL: usize = 256;

/// An opaque readiness signal created by another SDK resource.
#[derive(Debug)]
pub struct Pollable {
    pub(crate) inner: wit_poll::Pollable,
}

impl Pollable {
    pub(crate) fn new(inner: wit_poll::Pollable) -> Self {
        Self { inner }
    }

    /// Return immediately with the current readiness state.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.inner.ready()
    }

    /// Block until this signal is ready or the capsule is unloading.
    pub fn block(&self) -> Result<(), SysError> {
        self.inner.block().map_err(host_err)
    }
}

/// Block until one or more signals are ready and return their input indices.
///
/// The input must contain between 1 and [`MAX_POLLABLES_PER_CALL`] handles.
/// Returned indices are sorted and unique.
pub fn poll(pollables: &[&Pollable]) -> Result<Vec<usize>, SysError> {
    if pollables.is_empty() {
        return Err(SysError::ApiError(
            "poll requires at least one pollable".to_string(),
        ));
    }
    if pollables.len() > MAX_POLLABLES_PER_CALL {
        return Err(SysError::ApiError(format!(
            "poll accepts at most {MAX_POLLABLES_PER_CALL} pollables"
        )));
    }

    let raw: Vec<&wit_poll::Pollable> = pollables.iter().map(|pollable| &pollable.inner).collect();
    wit_poll::poll(&raw)
        .map(|indices| indices.into_iter().map(|index| index as usize).collect())
        .map_err(host_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_rejects_empty_input_before_calling_the_host() {
        let error = poll(&[]).expect_err("empty poll set should fail");
        assert!(error.to_string().contains("at least one"));
    }
}
