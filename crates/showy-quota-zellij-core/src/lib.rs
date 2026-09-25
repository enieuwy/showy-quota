pub mod cache;
pub mod codexbar;
pub mod config;
pub mod metrics;
pub mod palette;
pub mod pick;
pub mod prompt;
pub(crate) mod providers;
pub mod render;
pub(crate) mod reset;
pub mod sketchybar;
pub mod sketchybar_frame;
pub mod sketchybar_notch;
pub mod sketchybar_ring;
pub mod template;

pub use codexbar::{
    is_errored, parse_provider_config_payload, parse_usage_payload, parse_usage_payload_indexed,
    payload_has_renderable_provider, provider_ids_from_records, valid_provider_id,
    ProviderConfigError, ProviderRecord,
};
pub use config::RenderConfig;
pub use metrics::{emit_provider_metrics, emit_provider_metrics_with_visible, MetricsWithVisible};
pub use palette::Severity;
pub use pick::{emit_pick, PickOptions};
pub use prompt::{emit_formatted_prompt_segment, emit_prompt_segment, PromptOptions};
pub use render::{
    emit_rows, render_rows, render_tmux, render_vertical, render_zellij, Freshness, OutputFormat,
    RenderError, RenderOptions, RenderedRow,
};
pub use sketchybar::{sketchybar_rows, SketchybarOptions, SketchybarRow, SketchybarRows};
pub use template::{Template, TemplateScope, DEFAULT_PROMPT_FORMAT};
