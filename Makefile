# codexbar-bars — install, uninstall, test.
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
BIN_NAMES     := cb-bars-fetch cb-bars-tmux-bar cb-bars-zellij-bar cb-bars-zellij-pipe

.PHONY: help install install-bin install-sketchybar install-all uninstall test lint clean

help: ## Show this help.
	@awk 'BEGIN{FS=":.*##"}/^[a-zA-Z_-]+:.*##/{printf "  \033[36m%-20s\033[0m %s\n",$$1,$$2}' $(MAKEFILE_LIST)

install: install-bin ## Symlink shared scripts into the user's standard bin path.
	@printf '\nInstalled shared cb-bars scripts into $(BIN_DIR).\n'
	@printf 'SketchyBar is opt-in: run `make install-sketchybar`, then source "$$ITEM_DIR/cb_bars.sh".\n'

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
		"$(REPO)/sketchybar/items/cb_bars.sh:$(SBAR_ITEMS)/cb_bars.sh" \
		"$(REPO)/sketchybar/plugins/cb_bars.sh:$(SBAR_PLUGINS)/cb_bars.sh"; do \
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
		"$(REPO)/sketchybar/items/cb_bars.sh:$(SBAR_ITEMS)/cb_bars.sh" \
		"$(REPO)/sketchybar/plugins/cb_bars.sh:$(SBAR_PLUGINS)/cb_bars.sh"; do \
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

lint: ## Run shellcheck if available.
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck -x \
			"$(REPO)/bin/cb-bars-fetch" \
			"$(REPO)/bin/cb-bars-tmux-bar" \
			"$(REPO)/bin/cb-bars-zellij-bar" \
			"$(REPO)/bin/cb-bars-zellij-pipe" \
			"$(REPO)/lib/common.sh" \
			"$(REPO)/lib/strip.sh" \
			"$(REPO)/sketchybar/items/cb_bars.sh" \
			"$(REPO)/sketchybar/plugins/cb_bars.sh" \
			"$(REPO)/test/render_test.sh"; \
	else \
		printf 'shellcheck not installed; skipping\n'; \
	fi

clean: ## Remove the user-cache directory used by this repo.
	@rm -rf "$${XDG_CACHE_HOME:-$$HOME/.cache}/codexbar-bars"
	@printf 'cleared %s\n' "$${XDG_CACHE_HOME:-$$HOME/.cache}/codexbar-bars"
