//! Minimal test capsule for verifying the SDK compiles as a Component Model
//! binary targeting `wasm32-wasip2`.
//!
//! This capsule exercises the `#[capsule]` macro, a tool, an interceptor,
//! install/upgrade lifecycle hooks, and several host imports (KV, IPC, log).
//!
//! Build:
//!   cargo build -p test-capsule --target wasm32-wasip2
//!
//! The output at `target/wasm32-wasip2/debug/test_capsule.wasm` should be a
//! valid Component Model binary loadable by the wasmtime kernel.

use astrid_sdk::prelude::*;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct TestCapsule {
    counter: u64,
}

#[capsule]
impl TestCapsule {
    /// A simple tool that increments a counter and returns the new value.
    #[astrid::tool("increment")]
    #[astrid::mutable]
    fn increment(&mut self, _args: serde_json::Value) -> Result<serde_json::Value, SysError> {
        self.counter = self.counter.wrapping_add(1);
        let _ = log::info(&format!("counter incremented to {}", self.counter));
        Ok(serde_json::json!({ "counter": self.counter }))
    }

    /// A read-only tool that returns the current counter.
    #[astrid::tool("get_counter")]
    fn get_counter(&self, _args: serde_json::Value) -> Result<serde_json::Value, SysError> {
        Ok(serde_json::json!({ "counter": self.counter }))
    }

    /// An interceptor that passes through.
    #[astrid::interceptor("test.v1.event")]
    fn handle_event(&self, _payload: serde_json::Value) -> Result<serde_json::Value, SysError> {
        Ok(serde_json::json!({ "handled": true }))
    }

    /// Lifecycle: first-time install.
    #[astrid::install]
    fn install(&self) -> Result<(), SysError> {
        let _ = log::info("test-capsule installed");
        Ok(())
    }

    /// Lifecycle: upgrade from previous version.
    #[astrid::upgrade]
    fn upgrade(&self, prev_version: &str) -> Result<(), SysError> {
        let _ = log::info(&format!("test-capsule upgraded from {prev_version}"));
        Ok(())
    }
}
