# showy-quota — install, uninstall, test.
#
# `make install` symlinks scripts into ~/.local/bin and the SketchyBar
# pieces into ~/.config/sketchybar. It refuses to clobber files or retarget
# existing symlinks unless FORCE=1 is set.
#
# All paths are overridable from the command line, e.g.
#   make install BIN_DIR="$HOME/bin" SKETCHYBAR=/opt/dotfiles/sketchybar

PREFIX        ?= $(HOME)/.local
BIN_DIR       ?= $(PREFIX)/bin
SKETCHYBAR    ?= $(HOME)/.config/sketchybar
ZELLIJ_PLUGINS ?= $(HOME)/.config/zellij/plugins
SBAR_ITEMS    ?= $(SKETCHYBAR)/items
SBAR_PLUGINS  ?= $(SKETCHYBAR)/plugins
FORCE         ?= 0
CARGO         ?= cargo
RUSTC         ?= rustc
PLUGIN_TARGET_ADD := true
ifeq ($(shell command -v rustup >/dev/null 2>&1 && echo yes),yes)
CARGO         := rustup run stable cargo
RUSTC         := $(shell rustup which --toolchain stable rustc 2>/dev/null || printf 'rustc')
PLUGIN_TARGET_ADD := rustup target add --toolchain stable wasm32-wasip1
endif

REPO          := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
BIN_NAMES     := showy-quota-fetch showy-quota-state showy-quota showy-quota-tmux-bar showy-quota-zellij-bar showy-quota-zellij-pipe
PLUGIN_CRATE  := showy-quota-zellij
PLUGIN_WASM   := $(REPO)/target/wasm32-wasip1/release/showy-quota-zellij.wasm
PLUGIN_TARGET := $(ZELLIJ_PLUGINS)/showy-quota-zellij.wasm

.PHONY: help doctor diagnose install install-bin install-sketchybar plugin install-plugin grant-zellij-permissions install-all uninstall test lint clean

help: ## Show this help.
	@awk 'BEGIN{FS=":.*##"}/^[a-zA-Z_-]+:.*##/{printf "  \033[36m%-20s\033[0m %s\n",$$1,$$2}' $(MAKEFILE_LIST)

install: doctor install-bin ## Check prerequisites, then symlink shared scripts.
	@printf '\nInstalled shared showy-quota scripts into $(BIN_DIR).\n'
	@printf 'Nothing is wired to a bar yet. Run one of:\n'
	@printf '  make install-sketchybar    # then `source "$$ITEM_DIR/showy_quota.sh"`\n'
	@printf '  make install-plugin        # then paste adapters/zellij/layout-pane.kdl.fragment\n'
	@printf '  cat adapters/tmux/status-line.tmux.fragment\n'
	@printf '  cat adapters/zellij/layout-pane.kdl.fragment  # advanced zjstatus path also documented\n'

install-bin:
	@mkdir -p "$(BIN_DIR)"
	@for name in $(BIN_NAMES); do \
		src="$(REPO)/bin/$$name"; \
		chmod +x "$$src"; \
		target="$(BIN_DIR)/$$name"; \
		if [ -L "$$target" ]; then \
			cur=$$(readlink "$$target"); \
			if [ "$$cur" = "$$src" ]; then \
				printf 'noop  %s -> %s (already current)\n' "$$target" "$$src"; \
				continue; \
			fi; \
			if [ "$(FORCE)" != "1" ]; then \
				printf 'refusing to retarget %s\n  was: %s\n  now: %s\n  set FORCE=1 to adopt this symlink\n' "$$target" "$$cur" "$$src" >&2; \
				exit 1; \
			fi; \
			printf 'retarget %s\n  was: %s\n  now: %s\n' "$$target" "$$cur" "$$src" >&2; \
		elif [ -e "$$target" ]; then \
			printf 'refusing to clobber %s (not a symlink)\n' "$$target" >&2; \
			exit 1; \
		fi; \
		ln -sfn "$$src" "$$target" || { printf 'ln failed: %s\n' "$$target" >&2; exit 1; }; \
		printf 'linked %s -> %s\n' "$$target" "$$src"; \
	done

install-sketchybar:
	@mkdir -p "$(SBAR_ITEMS)" "$(SBAR_PLUGINS)"
	@for pair in \
		"$(REPO)/adapters/sketchybar/items/showy_quota.sh:$(REPO)/sketchybar/items/showy_quota.sh:$(SBAR_ITEMS)/showy_quota.sh" \
		"$(REPO)/adapters/sketchybar/plugins/showy_quota.sh:$(REPO)/sketchybar/plugins/showy_quota.sh:$(SBAR_PLUGINS)/showy_quota.sh"; do \
		src=$${pair%%:*}; rest=$${pair#*:}; legacy_src=$${rest%%:*}; target=$${rest#*:}; \
		chmod +x "$$src"; \
		if [ -L "$$target" ]; then \
			cur=$$(readlink "$$target"); \
			if [ "$$cur" = "$$src" ]; then \
				printf 'noop  %s -> %s (already current)\n' "$$target" "$$src"; \
				continue; \
			fi; \
			if [ "$$cur" = "$$legacy_src" ]; then \
				printf 'retarget legacy %s\n  was: %s\n  now: %s\n' "$$target" "$$cur" "$$src"; \
			elif [ "$(FORCE)" != "1" ]; then \
				printf 'refusing to retarget %s\n  was: %s\n  now: %s\n  set FORCE=1 to adopt this symlink\n' "$$target" "$$cur" "$$src" >&2; \
				exit 1; \
			else \
				printf 'retarget %s\n  was: %s\n  now: %s\n' "$$target" "$$cur" "$$src" >&2; \
			fi; \
		elif [ -e "$$target" ]; then \
			printf 'refusing to clobber %s\n' "$$target" >&2; exit 1; \
		fi; \
		ln -sfn "$$src" "$$target" || { printf 'ln failed: %s\n' "$$target" >&2; exit 1; }; \
		printf 'linked %s\n' "$$target"; \
	done

plugin: ## Build the standalone Zellij WASM plugin.
	@$(PLUGIN_TARGET_ADD) >/dev/null
	@RUSTC="$(RUSTC)" $(CARGO) build --release --target wasm32-wasip1 -p $(PLUGIN_CRATE)
	@printf 'built %s\n' "$(PLUGIN_WASM)"

install-plugin: plugin ## Install the standalone Zellij WASM plugin (pre-grants Zellij permissions).
	@mkdir -p "$(ZELLIJ_PLUGINS)"
	@if [ -e "$(PLUGIN_TARGET)" ] && ! cmp -s "$(PLUGIN_WASM)" "$(PLUGIN_TARGET)"; then \
		if [ "$(FORCE)" != "1" ]; then \
			printf 'refusing to clobber %s (set FORCE=1 to replace)\n' "$(PLUGIN_TARGET)" >&2; \
			exit 1; \
		fi; \
	fi
	@cp -f "$(PLUGIN_WASM)" "$(PLUGIN_TARGET)"
	@printf 'installed %s\n' "$(PLUGIN_TARGET)"
	@ZELLIJ_PLUGINS="$(ZELLIJ_PLUGINS)" "$(REPO)/bin/showy-quota" --grant-zellij "$(PLUGIN_TARGET)" || printf 'warning: could not pre-grant Zellij permissions; run `make grant-zellij-permissions` to retry\n' >&2

grant-zellij-permissions: ## Pre-grant Zellij plugin permissions (override path with PLUGIN=/abs/plugin.wasm).
	@ZELLIJ_PLUGINS="$(ZELLIJ_PLUGINS)" "$(REPO)/bin/showy-quota" --grant-zellij $(PLUGIN)

install-all: install-bin install-sketchybar install-plugin ## Install shared scripts and every optional integration.
uninstall: ## Remove symlinks that this Makefile created.
	@for name in $(BIN_NAMES); do \
		src="$(REPO)/bin/$$name"; \
		target="$(BIN_DIR)/$$name"; \
		if [ -L "$$target" ]; then \
			cur=$$(readlink "$$target"); \
			if [ "$$cur" = "$$src" ]; then \
				rm -f "$$target"; printf 'removed %s\n' "$$target"; \
			else \
				printf 'skip %s (points to %s)\n' "$$target" "$$cur" >&2; \
			fi; \
		fi; \
	done
	@for pair in \
		"$(REPO)/adapters/sketchybar/items/showy_quota.sh:$(REPO)/sketchybar/items/showy_quota.sh:$(SBAR_ITEMS)/showy_quota.sh" \
		"$(REPO)/adapters/sketchybar/plugins/showy_quota.sh:$(REPO)/sketchybar/plugins/showy_quota.sh:$(SBAR_PLUGINS)/showy_quota.sh"; do \
		src=$${pair%%:*}; rest=$${pair#*:}; legacy_src=$${rest%%:*}; target=$${rest#*:}; \
		if [ -L "$$target" ]; then \
			cur=$$(readlink "$$target"); \
			if [ "$$cur" = "$$src" ] || [ "$$cur" = "$$legacy_src" ]; then \
				rm -f "$$target"; printf 'removed %s\n' "$$target"; \
			else \
				printf 'skip %s (points to %s)\n' "$$target" "$$cur" >&2; \
			fi; \
		fi; \
	done
	@if [ -f "$(PLUGIN_TARGET)" ]; then \
		if [ -f "$(PLUGIN_WASM)" ] && cmp -s "$(PLUGIN_WASM)" "$(PLUGIN_TARGET)"; then \
			rm -f "$(PLUGIN_TARGET)"; printf 'removed %s\n' "$(PLUGIN_TARGET)"; \
		else \
			printf 'skip %s (does not match current build artifact)\n' "$(PLUGIN_TARGET)" >&2; \
		fi; \
	fi

test: ## Run the smoke-test suite against fixtures (no live codexbar).
	@$(REPO)/test/render_test.sh

doctor: ## Check runtime prerequisites without touching the system.
	@bash -c '(( BASH_VERSINFO[0] >= 4 ))' || { \
		printf 'showy-quota: bash 4+ required. macOS /bin/bash is 3.2; install Homebrew bash.\n' >&2; exit 1; }
	@command -v jq >/dev/null || { \
		printf 'showy-quota: jq is required (brew install jq / apt-get install jq).\n' >&2; exit 1; }
	@serve_url="$${SHOWY_QUOTA_CODEXBAR_SERVE_URL:-http://127.0.0.1:8080}"; \
	source_desc=""; \
	if command -v codexbar >/dev/null; then \
		source_desc="codexbar CLI $$(command -v codexbar)"; \
	elif command -v curl >/dev/null && curl --fail --silent --max-time "$${SHOWY_QUOTA_CODEXBAR_SERVE_TIMEOUT_SECONDS:-0.5}" "$${serve_url%/}/usage" >/dev/null; then \
		source_desc="codexbar serve-only $${serve_url%/}/usage"; \
	else \
		printf 'showy-quota: CodexBar data source required.\n' >&2; \
		printf '  preferred: codexbar CLI on PATH\n' >&2; \
		printf '  serve-only: set SHOWY_QUOTA_CODEXBAR_SERVE_URL to a working /usage endpoint (requires curl)\n' >&2; \
		exit 1; \
	fi; \
	printf 'doctor: bash %s, jq %s, %s — ok\n' \
		"$$(bash -c 'printf "%s" "$${BASH_VERSION:-unknown}"')" \
		"$$(jq --version)" \
		"$${source_desc}"; \
	for tool in curl flock shellcheck magick tmux zellij sketchybar; do \
		if command -v "$$tool" >/dev/null 2>&1; then \
			printf 'doctor: optional %-10s found: %s\n' "$$tool" "$$(command -v "$$tool")"; \
		else \
			printf 'doctor: optional %-10s missing (only needed for related integration/features)\n' "$$tool"; \
		fi; \
	done

diagnose: ## Print runtime state useful for bug reports.
	@$(REPO)/bin/showy-quota --diagnose

lint: ## Run shellcheck if available.
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck -x \
			"$(REPO)/bin/showy-quota-fetch" \
			"$(REPO)/bin/showy-quota-state" \
			"$(REPO)/bin/showy-quota" \
			"$(REPO)/bin/showy-quota-tmux-bar" \
			"$(REPO)/bin/showy-quota-zellij-bar" \
			"$(REPO)/bin/showy-quota-zellij-pipe" \
			"$(REPO)/showy-quota.tmux" \
			"$(REPO)/lib/common.sh" \
			"$(REPO)/lib/strip.sh" \
			"$(REPO)/adapters/sketchybar/items/showy_quota.sh" \
			"$(REPO)/adapters/sketchybar/plugins/showy_quota.sh" \
			"$(REPO)/test/render_test.sh"; \
	else \
		printf 'shellcheck not installed; skipping\n'; \
	fi


clean: ## Remove the user-cache directory used by this repo.
	@rm -rf "$${XDG_CACHE_HOME:-$$HOME/.cache}/showy-quota"
	@printf 'cleared %s\n' "$${XDG_CACHE_HOME:-$$HOME/.cache}/showy-quota"
