# Configuration reference

Generated from `share/config-manifest.json`.

| Environment key | KDL key | Type | Default | Scope | Description |
|---|---|---|---|---|---|
| SHOWY_QUOTA_FORCE_COLOR | — | boolean | 0 | shell, native | Force ANSI color even when the output is not a terminal. |
| SHOWY_QUOTA_SKETCHYBAR_FORCE_REDECLARE | — | boolean | 0 | shell | Recreate the items during the next SketchyBar update. |
| SHOWY_QUOTA_SKETCHYBAR_ICON_FONT_FILE | — | string |  | shell | Override the file used for provider font icons. |
| SHOWY_QUOTA_SKETCHYBAR_ROW_RADIUS | — | integer | 3 | shell, native | Set the corner radius of a native slider row. |
| SHOWY_QUOTA_DEGRADED_CLI | — | boolean | 0 | shell, native | Mark supplied data as degraded CLI fallback data. |
| SHOWY_QUOTA_SKETCHYBAR_AUTO_COMPACT | — | boolean | off | shell, native | Hide bar lanes and labels above the compact provider threshold. |
| SHOWY_QUOTA_MAX_USAGE_JSON_BYTES | — | integer | 5242880 | shell | Maximum accepted provider payload size in bytes. |
| SHOWY_QUOTA_REFRESH_SECONDS | refresh_seconds | integer | 120 | shell | Refresh seconds. |
| SHOWY_QUOTA_LOCK_WAIT_TENTHS | lock_wait_tenths | integer | 100 | shell | Lock wait tenths. |
| SHOWY_QUOTA_CACHE_DIR | cache_dir | string | ${XDG_CACHE_HOME:-${HOME}/.cache}/showy-quota | shell | Cache dir. |
| SHOWY_QUOTA_CODEXBAR_BIN | codexbar_bin | string | codexbar | shell | Codexbar bin. |
| SHOWY_QUOTA_CODEXBAR_SERVE_URL | codexbar_serve_url | string | http://127.0.0.1:8080 | shell | Codexbar serve url. |
| SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_INTERVAL_SECONDS | codexbar_serve_refresh_interval_seconds | string |  | shell | Codexbar serve refresh interval seconds. |
| SHOWY_QUOTA_CODEXBAR_SERVE_START_WAIT_TENTHS | codexbar_serve_start_wait_tenths | integer | 30 | shell | Codexbar serve start wait tenths. |
| SHOWY_QUOTA_MANAGE_SERVE | manage_serve | integer | 1 | shell | Manage serve. |
| SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS | codexbar_serve_timeout_seconds | integer | 10 | shell | Codexbar serve timeout seconds. |
| SHOWY_QUOTA_CODEXBAR_SERVE_USAGE_TIMEOUT_SECONDS | codexbar_serve_usage_timeout_seconds | integer | 30 | shell | Codexbar serve usage timeout seconds. |
| SHOWY_QUOTA_CODEXBAR_SERVE_REFRESH_SECONDS | codexbar_serve_refresh_seconds | string |  | shell | Codexbar serve refresh seconds. |
| SHOWY_QUOTA_CODEXBAR_CLI_TIMEOUT_SECONDS | codexbar_cli_timeout_seconds | integer | 20 | shell | Codexbar cli timeout seconds. |
| SHOWY_QUOTA_CODEXBAR_SERVE_FAILURES_BEFORE_RESTART | codexbar_serve_failures_before_restart | integer | 3 | shell | Codexbar serve failures before restart. |
| SHOWY_QUOTA_CODEXBAR_SERVE_FAILURES_BEFORE_CLI | codexbar_serve_failures_before_cli | string | ${SHOWY_QUOTA_CODEXBAR_SERVE_FAILURES_BEFORE_RESTART} | shell | Codexbar serve failures before cli. |
| SHOWY_QUOTA_CODEXBAR_SERVE_FAILURE_BACKOFF_SECONDS | codexbar_serve_failure_backoff_seconds | integer | 60 | shell | Codexbar serve failure backoff seconds. |
| SHOWY_QUOTA_CODEXBAR_CLI_FAILURE_BACKOFF_SECONDS | codexbar_cli_failure_backoff_seconds | string | ${SHOWY_QUOTA_REFRESH_SECONDS} | shell | Codexbar cli failure backoff seconds. |
| SHOWY_QUOTA_CODEXBAR_CONFIG_PROVIDERS_TIMEOUT_SECONDS | codexbar_config_providers_timeout_seconds | integer | 5 | shell | Codexbar config providers timeout seconds. |
| SHOWY_QUOTA_CODEXBAR_CONFIG_PROVIDERS_BACKOFF_SECONDS | codexbar_config_providers_backoff_seconds | integer | 60 | shell | Codexbar config providers backoff seconds. |
| SHOWY_QUOTA_PROVIDER_FAILURE_BACKOFF_SECONDS | provider_failure_backoff_seconds | string | ${SHOWY_QUOTA_REFRESH_SECONDS} | shell | Provider failure backoff seconds. |
| SHOWY_QUOTA_PROVIDERS | providers | csv |  | shell, native, zellij | Providers. |
| SHOWY_QUOTA_PROVIDERS_EXCLUDE | providers_exclude | csv |  | shell, native, zellij | Providers exclude. |
| SHOWY_QUOTA_PROVIDER_ORDER | provider_order | csv | ${SHOWY_QUOTA_PROVIDER_DEFAULT_ORDER} | shell, native, zellij | Provider order. |
| SHOWY_QUOTA_INCLUDE_STATUS | include_status | boolean | 1 | shell, native, zellij | Include status. |
| SHOWY_QUOTA_PALETTE_PRIMARY_GOOD | palette_primary_good | color | 25be6a | shell, native, zellij | Palette primary good. |
| SHOWY_QUOTA_PALETTE_PRIMARY_WARN | palette_primary_warn | color | f0af00 | shell, native, zellij | Palette primary warn. |
| SHOWY_QUOTA_PALETTE_PRIMARY_BAD | palette_primary_bad | color | ee5396 | shell, native, zellij | Palette primary bad. |
| SHOWY_QUOTA_PALETTE_PRIMARY_UNKNOWN | palette_primary_unknown | color | 6c7086 | shell, native, zellij | Palette primary unknown. |
| SHOWY_QUOTA_PALETTE_DIM_SCALE | palette_dim_scale | string | 0.55 | shell, native, zellij | Palette dim scale. |
| SHOWY_QUOTA_DIM_WINDOW_MINUTES | dim_window_minutes | integer | 10080 | shell, native, zellij | Dim window minutes. |
| SHOWY_QUOTA_PALETTE_BG | palette_bg | color | 161616 | shell, native, zellij | Palette bg. |
| SHOWY_QUOTA_PALETTE_SURFACE | palette_surface | color | 2a2a2a | shell, native, zellij | Palette surface. |
| SHOWY_QUOTA_PALETTE_TRACK | palette_track | color | 3a3a4a | shell, native, zellij | Palette track. |
| SHOWY_QUOTA_PALETTE_ICON_TEXT | palette_icon_text | color | f2f4f8 | shell, native, zellij | Palette icon text. |
| SHOWY_QUOTA_PALETTE_COUNTDOWN | palette_countdown | color | 7b8496 | shell, native, zellij | Palette countdown. |
| SHOWY_QUOTA_PALETTE_COUNTDOWN_WARN | palette_countdown_warn | color | ${SHOWY_QUOTA_PALETTE_PRIMARY_BAD} | shell, native, zellij | Palette countdown warn. |
| SHOWY_QUOTA_PALETTE_STALE | palette_stale | color | ${SHOWY_QUOTA_PALETTE_PRIMARY_UNKNOWN} | shell, native, zellij | Palette stale. |
| SHOWY_QUOTA_PALETTE_ELAPSED | palette_elapsed | color | be95ff | shell, native, zellij | Palette elapsed. |
| SHOWY_QUOTA_PALETTE_ELAPSED_LONG | palette_elapsed_long | color | 3ddbd9 | shell, native, zellij | Palette elapsed long. |
| SHOWY_QUOTA_STALE_GLYPH | stale_glyph | glyph | ⚠ | shell, native, zellij | Stale glyph. |
| SHOWY_QUOTA_DEGRADED_CLI_GLYPH | degraded_cli_glyph | glyph | ⚠cli | shell, native, zellij | Degraded cli glyph. |
| SHOWY_QUOTA_GOOD_MIN_REMAINING | good_min_remaining | integer | 40 | shell, native, zellij | Good min remaining. |
| SHOWY_QUOTA_WARN_MIN_REMAINING | warn_min_remaining | integer | 15 | shell, native, zellij | Warn min remaining. |
| SHOWY_QUOTA_TIME_WARN_MINUTES | time_warn_minutes | integer | 30 | shell, native, zellij | Time warn minutes. |
| SHOWY_QUOTA_CODEXBAR_RESOURCES | codexbar_resources | string | /Applications/CodexBar.app/Contents/Resources | shell | Codexbar resources. |
| SHOWY_QUOTA_SKETCHYBAR_IMAGE_CACHE | sketchybar_image_cache | string | ${SHOWY_QUOTA_CACHE_DIR}/sketchybar | shell | Sketchybar image cache. |
| SHOWY_QUOTA_SKETCHYBAR_CLICK | sketchybar_click | string | open -b com.steipete.codexbar | shell | Sketchybar click. |
| SHOWY_QUOTA_SKETCHYBAR_UPDATE_FREQ | sketchybar_update_freq | integer | 10 | shell | Sketchybar update freq. |
| SHOWY_QUOTA_PNG_BAR_W | png_bar_w | integer | 80 | shell | Png bar w. |
| SHOWY_QUOTA_PNG_BAR_H | png_bar_h | integer | 18 | shell | Png bar h. |
| SHOWY_QUOTA_SKETCHYBAR_ICON_WIDTH | sketchybar_icon_width | integer | 22 | shell | Sketchybar icon width. |
| SHOWY_QUOTA_SKETCHYBAR_ICON_PADDING_LEFT | sketchybar_icon_padding_left | integer | 5 | shell | Sketchybar icon padding left. |
| SHOWY_QUOTA_SKETCHYBAR_ICON_SCALE | sketchybar_icon_scale | string | 0.28 | shell | Sketchybar icon scale. |
| SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_MODE | sketchybar_provider_icon_mode | string | svg | shell | Sketchybar provider icon mode. |
| SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT | sketchybar_provider_icon_font | string | sketchybar-app-font:Regular:14.0 | shell | Sketchybar provider icon font. |
| SHOWY_QUOTA_SKETCHYBAR_PROVIDER_ICON_FONT_PADDING_RIGHT | sketchybar_provider_icon_font_padding_right | integer | 2 | shell | Sketchybar provider icon font padding right. |
| SHOWY_QUOTA_SKETCHYBAR_BAR_WIDTH | sketchybar_bar_width | string | $((SHOWY_QUOTA_PNG_BAR_W + 3)) | shell | Sketchybar bar width. |
| SHOWY_QUOTA_SKETCHYBAR_LABEL_WIDTH | sketchybar_label_width | integer | 32 | shell | Sketchybar label width. |
| SHOWY_QUOTA_SKETCHYBAR_PLACEMENT | sketchybar_placement | string | left | shell | Sketchybar placement. |
| SHOWY_QUOTA_SKETCHYBAR_BODY | sketchybar_body | string | rows | shell | Sketchybar body. |
| SHOWY_QUOTA_SKETCHYBAR_POPUP | sketchybar_popup | string | click | shell | Sketchybar popup. |
| SHOWY_QUOTA_SKETCHYBAR_NOTCH_MARGIN | sketchybar_notch_margin | integer | 4 | shell | Sketchybar notch margin. |
| SHOWY_QUOTA_SKETCHYBAR_COMPACT_PROVIDER_COUNT | sketchybar_compact_provider_count | integer | 5 | shell | Sketchybar compact provider count. |
| SHOWY_QUOTA_SKETCHYBAR_PILL_RADIUS | sketchybar_pill_radius | integer | 14 | shell | Sketchybar pill radius. |
| SHOWY_QUOTA_SKETCHYBAR_PILL_HEIGHT | sketchybar_pill_height | integer | 28 | shell | Sketchybar pill height. |
| SHOWY_QUOTA_SKETCHYBAR_PILL_COLOR | sketchybar_pill_color | string | 0xcc24273a | shell | Sketchybar pill color. |
| SHOWY_QUOTA_ZELLIJ_WIDGET | zellij_widget | string | pipe_showy_quota | shell | Zellij widget. |
| SHOWY_QUOTA_ZELLIJ_PIPE_NAME | zellij_pipe_name | string | showy-quota | shell | Zellij pipe name. |
| SHOWY_QUOTA_ZELLIJ_PIPE_INTERVAL | zellij_pipe_interval | integer | 10 | shell | Zellij pipe interval. |
| SHOWY_QUOTA_ZELLIJ_PIPE_TIMEOUT_TENTHS | zellij_pipe_timeout_tenths | integer | 20 | shell | Zellij pipe timeout tenths. |
| SHOWY_QUOTA_ZELLIJ_BAR_WIDTH | bar_width | integer | 12 | shell, native, zellij | Zellij bar width. |
| SHOWY_QUOTA_TERMINAL_BAR_MODE | terminal_bar_mode | string | auto | shell, native, zellij | Terminal bar mode. |
| SHOWY_QUOTA_PROVIDER_MODES | provider_modes | provider_modes | gemini=mono3,cursor=mono3 | shell, native, zellij | Provider modes. |
| SHOWY_QUOTA_MONO_COLOR_MODE | mono_color_mode | string | lowest | shell, native, zellij | Mono color mode. |
| SHOWY_QUOTA_MONO_MARKERS | mono_markers | csv | primary | shell, native, zellij | Mono markers. |
| SHOWY_QUOTA_PROVIDER_THRESHOLDS | provider_thresholds | thresholds |  | shell, native, zellij | Provider thresholds. |
| SHOWY_QUOTA_WINDOW_THRESHOLDS | window_thresholds | thresholds |  | shell, native, zellij | Window thresholds. |
| SHOWY_QUOTA_WINDOWS | windows | csv | primary,secondary,tertiary | shell, native, zellij | Windows. |
| SHOWY_QUOTA_WINDOW_MODE | window_mode | string | all | shell, native, zellij | Window mode. |
| SHOWY_QUOTA_COMPACT_PROVIDER_COUNT | compact_provider_count | integer | 0 | shell, native, zellij | Compact provider count. |
| SHOWY_QUOTA_WIDTH_BUDGET | width_budget | integer | 0 | shell, native, zellij | Width budget. |
| SHOWY_QUOTA_COMPACT_ORDER | compact_order | string | ordered | shell, native, zellij | Compact order. |
| SHOWY_QUOTA_ZELLIJ_BIN | zellij_bin | string | zellij | shell | Zellij bin. |
| SHOWY_QUOTA_ZELLIJ_PLUGIN | zellij_plugin | string |  | shell | Zellij plugin. |
| SHOWY_QUOTA_USAGE_FILE | usage_file | string | ${SHOWY_QUOTA_CACHE_DIR}/usage.json | shell | Usage file. |
| SHOWY_QUOTA_USAGE_STAMP | usage_stamp | string | ${SHOWY_QUOTA_CACHE_DIR}/usage.json.updated-at | shell | Usage stamp. |
| SHOWY_QUOTA_CODEXBAR_SERVE_PID_FILE | codexbar_serve_pid_file | string | ${SHOWY_QUOTA_CACHE_DIR}/codexbar-serve.pid | shell | Codexbar serve pid file. |
| SHOWY_QUOTA_USAGE_LOCK | usage_lock | string | ${SHOWY_QUOTA_CACHE_DIR}/usage.lock | shell | Usage lock. |
| SHOWY_QUOTA_CODEXBAR_SERVE_FAILURE_STAMP | codexbar_serve_failure_stamp | string | ${SHOWY_QUOTA_CACHE_DIR}/serve-failed-at | shell | Codexbar serve failure stamp. |
| SHOWY_QUOTA_CODEXBAR_SERVE_FAILURE_COUNT_FILE | codexbar_serve_failure_count_file | string | ${SHOWY_QUOTA_CACHE_DIR}/serve-failed-count | shell | Codexbar serve failure count file. |
| SHOWY_QUOTA_CODEXBAR_CLI_FAILURE_STAMP | codexbar_cli_failure_stamp | string | ${SHOWY_QUOTA_CACHE_DIR}/cli-failed-at | shell | Codexbar cli failure stamp. |
| SHOWY_QUOTA_CODEXBAR_CONFIG_PROVIDERS_FAILURE_STAMP | codexbar_config_providers_failure_stamp | string | ${SHOWY_QUOTA_CACHE_DIR}/config-providers-failed-at | shell | Codexbar config providers failure stamp. |
| SHOWY_QUOTA_PROVIDER_FAILURE_DIR | provider_failure_dir | string | ${SHOWY_QUOTA_CACHE_DIR}/provider-failures | shell | Provider failure dir. |
| SHOWY_QUOTA_FRESHNESS | freshness | string | off | native, zellij | Freshness. |
| SHOWY_QUOTA_SEVERITY_GLYPHS | severity_glyphs | boolean | False | native, zellij | Severity glyphs. |
| SHOWY_QUOTA_PALETTE_DIM_GOOD | palette_dim_good | color | None | native, zellij | Palette dim good. |
| SHOWY_QUOTA_PALETTE_DIM_WARN | palette_dim_warn | color | None | native, zellij | Palette dim warn. |
| SHOWY_QUOTA_PALETTE_DIM_BAD | palette_dim_bad | color | None | native, zellij | Palette dim bad. |
| SHOWY_QUOTA_PALETTE_DIM_UNKNOWN | palette_dim_unknown | color | None | native, zellij | Palette dim unknown. |
| SHOWY_QUOTA_ERROR_GLYPH | error_glyph | glyph | ⚠ | native, zellij | Error glyph. |
| SHOWY_QUOTA_RESET_DESCRIPTION_TIMEZONE_OFFSET | reset_description_timezone_offset | timezone | None | native, zellij | Reset description timezone offset minutes. |
| SHOWY_QUOTA_THEME | theme | string | default | native, zellij | Theme. |
| SHOWY_QUOTA_TMUX_BAR_WIDTH | tmux_bar_width | integer | None | native, zellij | Tmux bar width. |
| SHOWY_QUOTA_VERTICAL_BAR_WIDTH | vertical_bar_width | integer | 16 | native, zellij | Vertical bar width. |
| SHOWY_QUOTA_VERTICAL_SORT | vertical_sort | string | provider | native, zellij | Vertical sort. |
| SHOWY_QUOTA_VERTICAL_RESET_CLOCK | vertical_reset_clock | boolean | True | native, zellij | Vertical reset clock. |
| SHOWY_QUOTA_CAP_LEFT | cap_left | glyph |  | native, zellij | Cap left. |
| SHOWY_QUOTA_CAP_RIGHT | cap_right | glyph |  | native, zellij | Cap right. |
| SHOWY_QUOTA_RENDER_BIN | — | string |  | shell | Render bin. |
| SHOWY_QUOTA_FETCH_BIN | — | string |  | shell | Fetch bin. |
| SHOWY_QUOTA_FIXTURE | — | string |  | shell | Fixture. |
| SHOWY_QUOTA_ZELLIJ_PERMISSIONS_FILE | — | string |  | shell | Zellij permissions file. |
| SHOWY_QUOTA_NO_CONFIG | — | string |  | shell | No config. |
| SHOWY_QUOTA_NOW_EPOCH | — | integer | None | shell | Override the clock for deterministic fixture rendering. |
