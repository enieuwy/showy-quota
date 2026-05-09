# codexbar-bars — install, uninstall, test.
#
# `make install` symlinks scripts into ~/.local/bin and the SketchyBar
# pieces into ~/.config/sketchybar. It refuses to clobber non-symlinks so
# pre-existing user files are safe.
#
# All paths are overridable from the command line, e.g.
#   make install BIN_DIR=~/bin SKETCHYBAR_DIR=/opt/dotfiles/sketchybar

PREFIX        ?= $(HOME)/.local
BIN_DIR       ?= $(PREFIX)/bin
SKETCHYBAR    ?= $(HOME)/.config/sketchybar
SBAR_ITEMS    ?= $(SKETCHYBAR)/items
SBAR_PLUGINS  ?= $(SKETCHYBAR)/plugins

REPO          := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
BIN_SOURCES   := $(wildcard $(REPO)/bin/cb-bars-*)

.PHONY: help install install-bin install-sketchybar uninstall test lint clean

help: ## Show this help.
	@awk 'BEGIN{FS=":.*##"}/^[a-zA-Z_-]+:.*##/{printf "  \033[36m%-20s\033[0m %s\n",$$1,$$2}' $(MAKEFILE_LIST)

install: install-bin install-sketchybar ## Symlink scripts into the user's standard paths.
	@printf '\nInstalled. Source the SketchyBar item from your sketchybarrc:\n'
	@printf '  source "$$ITEM_DIR/cb_bars.sh"\n'
	@printf 'And reload sketchybar.\n'

install-bin:
	@mkdir -p "$(BIN_DIR)"
	@for src in $(BIN_SOURCES); do \
		name=$$(basename $$src); \
		target="$(BIN_DIR)/$$name"; \
		if [ -L "$$target" ]; then \
			cur=$$(readlink "$$target"); \
			if [ "$$cur" = "$$src" ]; then \
				printf 'noop  %s -> %s (already current)\n' "$$target" "$$src"; \
				continue; \
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
			printf 'retarget %s\n  was: %s\n  now: %s\n' "$$target" "$$cur" "$$src" >&2; \
		elif [ -e "$$target" ]; then \
			printf 'refusing to clobber %s\n' "$$target" >&2; exit 1; \
		fi; \
		ln -sfn "$$src" "$$target" || { printf 'ln failed: %s\n' "$$target" >&2; exit 1; }; \
		printf 'linked %s\n' "$$target"; \
	done

uninstall: ## Remove every symlink that this Makefile would create.
	@for src in $(BIN_SOURCES); do \
		name=$$(basename $$src); \
		target="$(BIN_DIR)/$$name"; \
		if [ -L "$$target" ]; then rm -f "$$target"; printf 'removed %s\n' "$$target"; fi; \
	done
	@for f in $(SBAR_ITEMS)/cb_bars.sh $(SBAR_PLUGINS)/cb_bars.sh; do \
		if [ -L "$$f" ]; then rm -f "$$f"; printf 'removed %s\n' "$$f"; fi; \
	done

test: ## Run the smoke-test suite against fixtures (no live codexbar).
	@$(REPO)/test/render_test.sh

lint: ## Run shellcheck if available.
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck -x \
			$(REPO)/bin/cb-bars-* \
			$(REPO)/lib/*.sh \
			$(REPO)/sketchybar/items/cb_bars.sh \
			$(REPO)/sketchybar/plugins/cb_bars.sh \
			$(REPO)/test/render_test.sh; \
	else \
		printf 'shellcheck not installed; skipping\n'; \
	fi

clean: ## Remove the user-cache directory used by this repo.
	@rm -rf "$${XDG_CACHE_HOME:-$$HOME/.cache}/codexbar-bars"
	@printf 'cleared %s\n' "$${XDG_CACHE_HOME:-$$HOME/.cache}/codexbar-bars"
