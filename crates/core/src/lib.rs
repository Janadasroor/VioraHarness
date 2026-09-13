pub mod context;
#[path = "loop/mod.rs"]
pub mod loop_mod;
pub mod mcp;
pub mod mode;
pub mod observe;
pub mod permissions;
pub mod provider;
pub mod runloop;
pub mod sandbox;
pub mod session;
pub mod skills;
pub mod subagent;
pub mod tools;

pub use loop_mod::AgentLoop;
pub use provider::{provider_for_model, ChatMessage, ChatRequest, Provider, ProviderEvent};
pub use tools::registry::{ToolDef, ToolRegistry};

/// Serializes unit tests that mutate process-global state (env vars, cwd).
/// Rust runs tests in threads; without this, env-flipping tests race.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));
