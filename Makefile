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
DATA_DIR      ?= $(PREFIX)/share/showy-quota
SKETCHYBAR    ?= $(HOME)/.config/sketchybar
ZELLIJ_PLUGINS ?= $(HOME)/.config/zellij/plugins
SBAR_ITEMS    ?= $(SKETCHYBAR)/items
SBAR_PLUGINS  ?= $(SKETCHYBAR)/plugins
FORCE         ?= 0
FORCE_PLUGIN_REMOVE ?= 0
CARGO         ?= cargo
MAKE_COMMAND  ?= $(MAKE)
RUSTC         ?= rustc

# Runtime dependency floors. `jq` and `bash` are hard requirements, so `doctor`
# fails below them. ImageMagick is optional (SketchyBar provider icons only) and
# is only advised on, because a missing or older `magick` costs icons, not
# correctness. 7.1.1 is the line below which the SVG/coder hardening that this
# repo's icon pipeline relies on is absent.
BASH_MIN      := 4
JQ_MIN        := 1.6
IMAGEMAGICK_MIN := 7.1.1

# Kept in step with the pinned installs in .github/workflows/{ci,release}.yml so
# the local gate and CI audit with the same tool. Renovate manages all four
# sites through the cargo-audit custom manager in renovate.json.
CARGO_AUDIT_VERSION := 0.22.2
PLUGIN_TARGET_ADD := true
ifeq ($(shell command -v rustup >/dev/null 2>&1 && echo yes),yes)
CARGO         := rustup run stable cargo
RUSTC         := $(shell rustup which --toolchain stable rustc 2>/dev/null || printf 'rustc')
PLUGIN_TARGET_ADD := rustup target add --toolchain stable wasm32-wasip1
endif

REPO          := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
BIN_NAMES     := showy-quota-fetch showy-quota-state showy-quota showy-quota-tmux-bar showy-quota-zellij-bar showy-quota-zellij-pipe
COPY_BIN_NAMES := $(BIN_NAMES) showy-quota-render
PLUGIN_CRATE  := showy-quota-zellij
PLUGIN_WASM   := $(REPO)/target/wasm32-wasip1/release/showy-quota-zellij.wasm
PLUGIN_TARGET := $(ZELLIJ_PLUGINS)/showy-quota-zellij.wasm
RENDER_CRATE  := showy-quota-zellij-core
RENDER_BIN    := $(REPO)/target/release/showy-quota-render
RENDER_TARGET := $(BIN_DIR)/showy-quota-render

.PHONY: help doctor check-deps diagnose install install-bin install-copy install-copy-sketchybar install-sketchybar plugin render-bin install-plugin grant-zellij-permissions install-all uninstall test lint ci-gates hooks clean

help: ## Show this help.
	@awk 'BEGIN{FS=":.*##"}/^[a-zA-Z_-]+:.*##/{printf "  \033[36m%-20s\033[0m %s\n",$$1,$$2}' $(MAKEFILE_LIST)

install: doctor install-bin ## Check prerequisites, then symlink shared scripts.
	@printf '\nInstalled shared showy-quota scripts into $(BIN_DIR).\n'
	@printf 'Nothing is wired to a bar yet. Run one of:\n'
	@printf '  make install-sketchybar    # then `source "$$ITEM_DIR/showy_quota.sh"`\n'
	@printf '  make install-plugin        # then paste adapters/zellij/layout-pane.kdl.fragment\n'
	@printf '  cat adapters/tmux/status-line.tmux.fragment\n'
	@printf '  cat adapters/zellij/layout-pane.kdl.fragment  # advanced zjstatus path also documented\n'

install-bin: render-bin
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
	@src="$(RENDER_BIN)"; \
	target="$(RENDER_TARGET)"; \
	if [ -L "$$target" ]; then \
		cur=$$(readlink "$$target"); \
		if [ "$$cur" = "$$src" ]; then \
			printf 'noop  %s -> %s (already current)\n' "$$target" "$$src"; \
		elif [ "$(FORCE)" != "1" ]; then \
			printf 'refusing to retarget %s\n  was: %s\n  now: %s\n  set FORCE=1 to adopt this symlink\n' "$$target" "$$cur" "$$src" >&2; \
			exit 1; \
		else \
			printf 'retarget %s\n  was: %s\n  now: %s\n' "$$target" "$$cur" "$$src" >&2; \
			ln -sfn "$$src" "$$target" || { printf 'ln failed: %s\n' "$$target" >&2; exit 1; }; \
			printf 'linked %s -> %s\n' "$$target" "$$src"; \
		fi; \
	elif [ -e "$$target" ]; then \
		printf 'refusing to clobber %s (not a symlink)\n' "$$target" >&2; \
		exit 1; \
	else \
		ln -sfn "$$src" "$$target" || { printf 'ln failed: %s\n' "$$target" >&2; exit 1; }; \
		printf 'linked %s -> %s\n' "$$target" "$$src"; \
	fi

install-copy: ## Copy runtime tree into DATA_DIR and link commands into BIN_DIR.
	@set -e; \
	if [ -e "$(DATA_DIR)" ] && [ ! -d "$(DATA_DIR)" ]; then \
		printf 'refusing to install into %s (not a directory)\n' "$(DATA_DIR)" >&2; \
		exit 1; \
	fi; \
	mkdir -p "$(DATA_DIR)" "$(BIN_DIR)"; \
	for path in bin lib adapters share; do \
		rm -rf "$(DATA_DIR)/$$path"; \
		cp -R "$(REPO)/$$path" "$(DATA_DIR)/$$path"; \
	done; \
	cp "$(REPO)/showy-quota.tmux" "$(DATA_DIR)/showy-quota.tmux"; \
	if [ -f "$(REPO)/bin/showy-quota-render" ]; then \
		chmod +x "$(DATA_DIR)/bin/showy-quota-render"; \
		printf 'using prebuilt %s\n' "$(DATA_DIR)/bin/showy-quota-render"; \
	elif command -v cargo >/dev/null 2>&1 || command -v rustup >/dev/null 2>&1; then \
		$(MAKE_COMMAND) --no-print-directory render-bin; \
		cp "$(RENDER_BIN)" "$(DATA_DIR)/bin/showy-quota-render"; \
		chmod +x "$(DATA_DIR)/bin/showy-quota-render"; \
		printf 'installed built %s\n' "$(DATA_DIR)/bin/showy-quota-render"; \
	else \
		rm -f "$(DATA_DIR)/bin/showy-quota-render"; \
		printf 'warning: showy-quota-render not installed; terminal strips will show "AI ?" with a hint until you run make render-bin or install a release tarball with bin/showy-quota-render\n' >&2; \
	fi; \
	for name in $(BIN_NAMES); do chmod +x "$(DATA_DIR)/bin/$$name"; done; \
	printf 'copied runtime tree to %s\n' "$(DATA_DIR)"
	@for name in $(COPY_BIN_NAMES); do \
		src="$(DATA_DIR)/bin/$$name"; \
		target="$(BIN_DIR)/$$name"; \
		if [ ! -e "$$src" ]; then \
			printf 'skip  %s (not installed)\n' "$$src" >&2; \
			continue; \
		fi; \
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

install-copy-sketchybar: install-copy ## Link SketchyBar integration from copied DATA_DIR.
	@mkdir -p "$(SBAR_ITEMS)" "$(SBAR_PLUGINS)"
	@for pair in \
		"$(DATA_DIR)/adapters/sketchybar/items/showy_quota.sh:$(SBAR_ITEMS)/showy_quota.sh" \
		"$(DATA_DIR)/adapters/sketchybar/plugins/showy_quota.sh:$(SBAR_PLUGINS)/showy_quota.sh"; do \
		src=$${pair%%:*}; target=$${pair#*:}; \
		chmod +x "$$src"; \
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

install-sketchybar:
	@mkdir -p "$(SBAR_ITEMS)" "$(SBAR_PLUGINS)"
	@for pair in \
		"$(REPO)/adapters/sketchybar/items/showy_quota.sh:$(SBAR_ITEMS)/showy_quota.sh" \
		"$(REPO)/adapters/sketchybar/plugins/showy_quota.sh:$(SBAR_PLUGINS)/showy_quota.sh"; do \
		src=$${pair%%:*}; target=$${pair#*:}; \
		chmod +x "$$src"; \
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

# A real file target, not a phony one: the suite runs whatever binary is at
# RENDER_BIN, so a stale artefact from an older checkout silently tests old
# code. That is not hypothetical — it is what made a renderer failure
# reproduce in CI but not locally. Listing the crate sources as prerequisites
# lets Make skip cargo entirely when nothing changed, while a source edit
# rebuilds before any assertion runs.
RENDER_SOURCES := $(shell find $(REPO)/crates/$(RENDER_CRATE)/src -name '*.rs' 2>/dev/null) \
                  $(REPO)/crates/$(RENDER_CRATE)/Cargo.toml \
                  $(REPO)/Cargo.toml $(REPO)/Cargo.lock

$(RENDER_BIN): $(RENDER_SOURCES)
	@RUSTC="$(RUSTC)" $(CARGO) build --release -p $(RENDER_CRATE)
	@printf 'built %s\n' "$(RENDER_BIN)"

render-bin: $(RENDER_BIN) ## Build the native terminal strip renderer.

plugin: ## Build the standalone Zellij WASM plugin.
	@$(PLUGIN_TARGET_ADD) >/dev/null
	@RUSTC="$(RUSTC)" $(CARGO) build --release --target wasm32-wasip1 -p $(PLUGIN_CRATE)
	@printf 'built %s\n' "$(PLUGIN_WASM)"

install-plugin: plugin ## Install the standalone Zellij WASM plugin (pre-grants Zellij permissions).
	@mkdir -p "$(ZELLIJ_PLUGINS)"
	@checksum="$(PLUGIN_WASM).sha256"; \
	if [ -f "$$checksum" ]; then \
		printf 'verifying %s\n' "$$checksum"; \
		(cd "$$(dirname "$(PLUGIN_WASM)")" && shasum -a 256 -c "$$(basename "$$checksum")"); \
	fi
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
	@ZELLIJ_PLUGINS="$(ZELLIJ_PLUGINS)" "$(REPO)/bin/showy-quota" --grant-zellij "$(PLUGIN)"

install-all: install-bin install-sketchybar install-plugin ## Install shared scripts and every optional integration.
uninstall: ## Remove symlinks and copied DATA_DIR that this Makefile created.
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
	@target="$(RENDER_TARGET)"; \
	src="$(RENDER_BIN)"; \
	if [ -L "$$target" ]; then \
		cur=$$(readlink "$$target"); \
		if [ "$$cur" = "$$src" ]; then \
			rm -f "$$target"; printf 'removed %s\n' "$$target"; \
		else \
			printf 'skip %s (points to %s)\n' "$$target" "$$cur" >&2; \
		fi; \
	fi
	@for pair in \
		"$(REPO)/adapters/sketchybar/items/showy_quota.sh:$(SBAR_ITEMS)/showy_quota.sh" \
		"$(REPO)/adapters/sketchybar/plugins/showy_quota.sh:$(SBAR_PLUGINS)/showy_quota.sh"; do \
		src=$${pair%%:*}; target=$${pair#*:}; \
		if [ -L "$$target" ]; then \
			cur=$$(readlink "$$target"); \
			if [ "$$cur" = "$$src" ]; then \
				rm -f "$$target"; printf 'removed %s\n' "$$target"; \
			else \
				printf 'skip %s (points to %s)\n' "$$target" "$$cur" >&2; \
			fi; \
		fi; \
	done
	@for name in $(COPY_BIN_NAMES); do \
		src="$(DATA_DIR)/bin/$$name"; \
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
		"$(DATA_DIR)/adapters/sketchybar/items/showy_quota.sh:$(SBAR_ITEMS)/showy_quota.sh" \
		"$(DATA_DIR)/adapters/sketchybar/plugins/showy_quota.sh:$(SBAR_PLUGINS)/showy_quota.sh"; do \
		src=$${pair%%:*}; target=$${pair#*:}; \
		if [ -L "$$target" ]; then \
			cur=$$(readlink "$$target"); \
			if [ "$$cur" = "$$src" ]; then \
				rm -f "$$target"; printf 'removed %s\n' "$$target"; \
			else \
				printf 'skip %s (points to %s)\n' "$$target" "$$cur" >&2; \
			fi; \
		fi; \
	done
	@data_dir="$(DATA_DIR)"; \
	home_dir="$${HOME:-$(HOME)}"; \
	case "$$data_dir" in ""|"/"|"/."|"$$home_dir"|"$$home_dir"/) \
		printf 'refusing to remove unsafe DATA_DIR: %s\n' "$$data_dir" >&2; \
		exit 1; \
		;; \
	esac; \
	if [ -d "$$data_dir" ]; then \
		rm -rf "$$data_dir"; \
		printf 'removed %s\n' "$$data_dir"; \
	fi
	@if [ -f "$(PLUGIN_TARGET)" ]; then \
		if [ "$(FORCE_PLUGIN_REMOVE)" = "1" ] || { [ -f "$(PLUGIN_WASM)" ] && cmp -s "$(PLUGIN_WASM)" "$(PLUGIN_TARGET)"; }; then \
			rm -f "$(PLUGIN_TARGET)"; printf 'removed %s\n' "$(PLUGIN_TARGET)"; \
		else \
			printf 'skip %s (does not match current build artifact)\n' "$(PLUGIN_TARGET)" >&2; \
		fi; \
	fi

test: $(RENDER_BIN) ## Run the smoke-test suite against fixtures (no live codexbar).
	@$(REPO)/test/render_test.sh

check-deps: ## Verify bash and jq meet the documented version floors.
	@bash -c '(( BASH_VERSINFO[0] >= $(BASH_MIN) ))' || { \
		printf 'showy-quota: bash %s+ required. macOS /bin/bash is 3.2; install Homebrew bash.\n' "$(BASH_MIN)" >&2; exit 1; }
	@command -v jq >/dev/null || { \
		printf 'showy-quota: jq %s+ is required (brew install jq / apt-get install jq).\n' "$(JQ_MIN)" >&2; exit 1; }
	@jq_raw=$$(jq --version 2>/dev/null); \
	jq_ver=$${jq_raw#jq-}; jq_ver=$${jq_ver#jq }; \
	jq_major=$${jq_ver%%.*}; jq_rest=$${jq_ver#*.}; jq_minor=$${jq_rest%%.*}; \
	case "$$jq_major" in ''|*[!0-9]*) jq_major=0 ;; esac; \
	case "$$jq_minor" in ''|*[!0-9]*) jq_minor=0 ;; esac; \
	min="$(JQ_MIN)"; \
	min_major=$${min%%.*}; min_rest=$${min#*.}; min_minor=$${min_rest%%.*}; \
	case "$$min_major" in ''|*[!0-9]*) min_major=0 ;; esac; \
	case "$$min_minor" in ''|*[!0-9]*) min_minor=0 ;; esac; \
	if [ "$$jq_major" -lt "$$min_major" ] || { [ "$$jq_major" -eq "$$min_major" ] && [ "$$jq_minor" -lt "$$min_minor" ]; }; then \
		printf 'showy-quota: jq %s+ required; found "%s".\n' "$(JQ_MIN)" "$$jq_raw" >&2; \
		exit 1; \
	fi; \
	printf 'deps: bash %s, jq %s — ok (floors: bash %s, jq %s)\n' \
		"$$(bash -c 'printf "%s" "$${BASH_VERSION:-unknown}"')" "$$jq_ver" "$(BASH_MIN)" "$(JQ_MIN)"

doctor: check-deps ## Check runtime prerequisites without touching the system.
	@if [ -x "$(RENDER_BIN)" ]; then \
		printf 'doctor: render binary found: %s\n' "$(RENDER_BIN)"; \
	elif command -v showy-quota-render >/dev/null 2>&1; then \
		printf 'doctor: render binary found on PATH: %s\n' "$$(command -v showy-quota-render)"; \
	else \
		printf 'doctor: render binary missing; run make render-bin\n' >&2; \
	fi
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
	for tool in curl flock shellcheck magick rsvg-convert tmux zellij sketchybar; do \
		if command -v "$$tool" >/dev/null 2>&1; then \
			printf 'doctor: optional %-10s found: %s\n' "$$tool" "$$(command -v "$$tool")"; \
		else \
			printf 'doctor: optional %-10s missing (only needed for related integration/features)\n' "$$tool"; \
		fi; \
	done
	@if command -v magick >/dev/null 2>&1; then \
		im_raw=$$(magick -version 2>/dev/null | head -n1); \
		im_ver=$${im_raw#*ImageMagick }; im_ver=$${im_ver%% *}; im_ver=$${im_ver%-*}; \
		printf 'doctor: ImageMagick %s (floor %s, SketchyBar icons only)\n' "$$im_ver" "$(IMAGEMAGICK_MIN)"; \
		im_major=$${im_ver%%.*}; im_rest=$${im_ver#*.}; im_minor=$${im_rest%%.*}; im_patch=$${im_rest#*.}; im_patch=$${im_patch%%.*}; \
		case "$$im_major" in ''|*[!0-9]*) im_major=0 ;; esac; \
		case "$$im_minor" in ''|*[!0-9]*) im_minor=0 ;; esac; \
		case "$$im_patch" in ''|*[!0-9]*) im_patch=0 ;; esac; \
		imin="$(IMAGEMAGICK_MIN)"; \
		imin_major=$${imin%%.*}; imin_rest=$${imin#*.}; imin_minor=$${imin_rest%%.*}; imin_patch=$${imin_rest#*.}; imin_patch=$${imin_patch%%.*}; \
		case "$$imin_major" in ''|*[!0-9]*) imin_major=0 ;; esac; \
		case "$$imin_minor" in ''|*[!0-9]*) imin_minor=0 ;; esac; \
		case "$$imin_patch" in ''|*[!0-9]*) imin_patch=0 ;; esac; \
		if [ "$$(printf '%d%03d%03d' "$$im_major" "$$im_minor" "$$im_patch")" -lt "$$(printf '%d%03d%03d' "$$imin_major" "$$imin_minor" "$$imin_patch")" ]; then \
			printf 'doctor: ImageMagick is below %s. It renders third-party provider SVGs, so prefer an updated build.\n' "$(IMAGEMAGICK_MIN)" >&2; \
		fi; \
	fi
	@if command -v codexbar >/dev/null 2>&1; then \
		printf 'doctor: %s\n' "$$(codexbar --version 2>/dev/null || printf 'CodexBar version unknown')"; \
	fi

diagnose: ## Print runtime state useful for bug reports.
	@$(REPO)/bin/showy-quota --diagnose
	@if command -v jq >/dev/null 2>&1; then \
		"$(REPO)/bin/showy-quota-state" --json 2>/dev/null \
			| jq -r '.providerMetrics[]? | select(.error != null) | "provider errors: \(.provider): \(.error.kind): \(.error.message)"' 2>/dev/null \
			|| true; \
	fi

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

ci-gates: ## Run every CI gate locally; run before tagging a release.
	@printf '\n== ci-gates 1/9: make check-deps ==\n'
	@$(MAKE_COMMAND) --no-print-directory check-deps
	@printf '\n== ci-gates 2/9: make lint ==\n'
	@$(MAKE_COMMAND) --no-print-directory lint
	@printf '\n== ci-gates 3/9: make test ==\n'
	@$(MAKE_COMMAND) --no-print-directory test
	@printf '\n== ci-gates 4/9: cargo fmt --all -- --check ==\n'
	@$(CARGO) fmt --all -- --check
	@printf '\n== ci-gates 5/9: cargo clippy --workspace --all-targets -- -D warnings ==\n'
	@$(CARGO) clippy --workspace --all-targets -- -D warnings
	@printf '\n== ci-gates 6/9: cargo test --workspace ==\n'
	@$(CARGO) test --workspace
	@printf '\n== ci-gates 7/9: cargo audit ==\n'
	@command -v cargo-audit >/dev/null 2>&1 || { \
		printf 'cargo-audit not installed; run: cargo install cargo-audit --locked --version %s\n' "$(CARGO_AUDIT_VERSION)" >&2; \
		exit 1; \
	}
	@$(CARGO) audit
	@printf '\n== ci-gates 8/9: make plugin ==\n'
	@$(MAKE_COMMAND) --no-print-directory plugin
	@printf '\n== ci-gates 9/9: check plugin exports ==\n'

hooks: ## Install the git pre-commit hook (rustfmt check via .githooks).
	@git config core.hooksPath .githooks
	@printf 'Installed git hooks: core.hooksPath -> .githooks\n'


clean: ## Remove the user-cache directory used by this repo.
	@rm -rf "$${XDG_CACHE_HOME:-$$HOME/.cache}/showy-quota"
	@printf 'cleared %s\n' "$${XDG_CACHE_HOME:-$$HOME/.cache}/showy-quota"
