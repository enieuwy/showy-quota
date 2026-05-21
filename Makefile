# showy-bar — install, uninstall, test.
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
SBAR_ITEMS    ?= $(SKETCHYBAR)/items
SBAR_PLUGINS  ?= $(SKETCHYBAR)/plugins
FORCE         ?= 0

REPO          := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
BIN_NAMES     := showy-bar-fetch showy-bar-state showy-bar showy-bar-tmux-bar showy-bar-zellij-bar showy-bar-zellij-pipe showy-bar-zellij-kick showy-bar-zellij-new-tab

.PHONY: help doctor diagnose install install-bin install-sketchybar install-all uninstall test lint clean

help: ## Show this help.
	@awk 'BEGIN{FS=":.*##"}/^[a-zA-Z_-]+:.*##/{printf "  \033[36m%-20s\033[0m %s\n",$$1,$$2}' $(MAKEFILE_LIST)

install: doctor install-bin ## Check prerequisites, then symlink shared scripts.
	@printf '\nInstalled shared showy-bar scripts into $(BIN_DIR).\n'
	@printf 'Nothing is wired to a bar yet. Run one of:\n'
	@printf '  make install-sketchybar      # then `source "$$ITEM_DIR/showy_bar.sh"`\n'
	@printf '  cat tmux/status-line.tmux.fragment\n'
	@printf '  cat zellij/layout-pane.kdl.fragment\n'

install-bin:
	@mkdir -p "$(BIN_DIR)"
	@for name in $(BIN_NAMES); do \
		src="$(REPO)/bin/$$name"; \
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
		"$(REPO)/sketchybar/items/showy_bar.sh:$(SBAR_ITEMS)/showy_bar.sh" \
		"$(REPO)/sketchybar/plugins/showy_bar.sh:$(SBAR_PLUGINS)/showy_bar.sh"; do \
		src=$${pair%%:*}; target=$${pair##*:}; \
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
			printf 'refusing to clobber %s\n' "$$target" >&2; exit 1; \
		fi; \
		ln -sfn "$$src" "$$target" || { printf 'ln failed: %s\n' "$$target" >&2; exit 1; }; \
		printf 'linked %s\n' "$$target"; \
	done


install-all: install-bin install-sketchybar ## Install shared scripts and every optional integration.
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
		"$(REPO)/sketchybar/items/showy_bar.sh:$(SBAR_ITEMS)/showy_bar.sh" \
		"$(REPO)/sketchybar/plugins/showy_bar.sh:$(SBAR_PLUGINS)/showy_bar.sh"; do \
		src=$${pair%%:*}; target=$${pair##*:}; \
		if [ -L "$$target" ]; then \
			cur=$$(readlink "$$target"); \
			if [ "$$cur" = "$$src" ]; then \
				rm -f "$$target"; printf 'removed %s\n' "$$target"; \
			else \
				printf 'skip %s (points to %s)\n' "$$target" "$$cur" >&2; \
			fi; \
		fi; \
	done

test: ## Run the smoke-test suite against fixtures (no live codexbar).
	@$(REPO)/test/render_test.sh

doctor: ## Check runtime prerequisites without touching the system.
	@bash -c '(( BASH_VERSINFO[0] >= 4 ))' || { \
		printf 'showy-bar: bash 4+ required. macOS /bin/bash is 3.2; install Homebrew bash.\n' >&2; exit 1; }
	@command -v jq >/dev/null || { \
		printf 'showy-bar: jq is required (brew install jq / apt-get install jq).\n' >&2; exit 1; }
	@serve_url="$${SHOWY_BAR_CODEXBAR_SERVE_URL:-http://127.0.0.1:8080}"; \
	if command -v codexbar >/dev/null; then \
		source_desc="codexbar $$(command -v codexbar)"; \
	elif command -v curl >/dev/null && curl --fail --silent --max-time "$${SHOWY_BAR_CODEXBAR_SERVE_TIMEOUT_SECONDS:-0.5}" "$${serve_url%/}/usage" >/dev/null; then \
		source_desc="codexbar serve $${serve_url%/}/usage"; \
	else \
		printf 'showy-bar: CodexBar data source required.\n' >&2; \
		printf '  preferred: codexbar serve at %s/usage (requires curl)\n' "$${serve_url%/}" >&2; \
		printf '  fallback:  codexbar CLI on PATH\n' >&2; exit 1; \
	fi; \
	printf 'doctor: bash %s, jq %s, %s — ok\n' \
		"$$(bash --version | head -n1 | awk '{print $$4}')" \
		"$$(jq --version)" \
		"$${source_desc}"

diagnose: ## Print runtime state useful for bug reports.
	@$(REPO)/bin/showy-bar --diagnose

lint: ## Run shellcheck if available.
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck -x \
			"$(REPO)/bin/showy-bar-fetch" \
			"$(REPO)/bin/showy-bar-state" \
			"$(REPO)/bin/showy-bar" \
			"$(REPO)/bin/showy-bar-tmux-bar" \
			"$(REPO)/bin/showy-bar-zellij-bar" \
			"$(REPO)/bin/showy-bar-zellij-pipe" \
			"$(REPO)/bin/showy-bar-zellij-kick" \
			"$(REPO)/bin/showy-bar-zellij-new-tab" \
			"$(REPO)/lib/common.sh" \
			"$(REPO)/lib/strip.sh" \
			"$(REPO)/sketchybar/items/showy_bar.sh" \
			"$(REPO)/sketchybar/plugins/showy_bar.sh" \
			"$(REPO)/test/render_test.sh"; \
	else \
		printf 'shellcheck not installed; skipping\n'; \
	fi


clean: ## Remove the user-cache directory used by this repo.
	@rm -rf "$${XDG_CACHE_HOME:-$$HOME/.cache}/showy-bar"
	@printf 'cleared %s\n' "$${XDG_CACHE_HOME:-$$HOME/.cache}/showy-bar"
