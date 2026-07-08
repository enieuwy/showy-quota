pub mod cache;
pub mod codexbar;
pub mod config;
pub mod metrics;
pub mod palette;
pub mod prompt;
pub mod render;
pub(crate) mod reset;

pub use codexbar::{
    is_errored, parse_provider_config_payload, parse_usage_payload,
    payload_has_renderable_provider, provider_ids_from_records, valid_provider_id,
    ProviderConfigError, ProviderRecord,
};
pub use config::RenderConfig;
pub use metrics::emit_provider_metrics;
pub use prompt::{emit_prompt_segment, PromptOptions};
pub use render::{render_tmux, render_zellij, OutputFormat, RenderError, RenderOptions};
