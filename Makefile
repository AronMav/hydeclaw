-include .deploy.env
PI_HOST   ?= user@your-server
PI_DIR    := ~/hydeclaw
TARGET    := aarch64-unknown-linux-gnu
BIN       := target/$(TARGET)/release/hydeclaw-core
AUTH      ?= $(shell cat .auth-token 2>/dev/null || echo "MISSING_AUTH_TOKEN")

.PHONY: check test build build-arm64 ui release deploy-binary deploy-ui deploy deploy-docker doctor clean

# ── Development ──────────────────────────────────────────────────────────────

check:
	cargo check --all-targets

test:
	cargo test

lint:
	cargo clippy --all-targets -- -D warnings

# ── Build ────────────────────────────────────────────────────────────────────

build:
	cargo build --release

build-arm64:
	cargo zigbuild --release --target $(TARGET) -p hydeclaw-core -p hydeclaw-watchdog -p hydeclaw-memory-worker

ui:
	cd ui && npm run build

release:
	bash release.sh --all

# ── Deploy to Pi ─────────────────────────────────────────────────────────────

deploy-binary: build-arm64
	@for CRATE in hydeclaw-core hydeclaw-watchdog hydeclaw-memory-worker; do \
		BIN=target/$(TARGET)/release/$$CRATE; \
		if [ -f "$$BIN" ]; then \
			scp $$BIN $(PI_HOST):$(PI_DIR)/$${CRATE}-aarch64; \
			echo "  deployed $$CRATE"; \
		fi; \
	done
	ssh $(PI_HOST) "chmod +x $(PI_DIR)/hydeclaw-*-aarch64; for SVC in hydeclaw-core hydeclaw-watchdog hydeclaw-memory-worker; do systemctl --user is-enabled \$$SVC 2>/dev/null && systemctl --user restart \$$SVC && echo \"  restarted \$$SVC\" || true; done"

deploy-ui: ui
	ssh $(PI_HOST) "rm -rf $(PI_DIR)/ui/out"
	cd ui && tar cf - out | ssh $(PI_HOST) "mkdir -p $(PI_DIR)/ui && cd $(PI_DIR)/ui && tar xf -"

deploy-migrations:
	scp -r migrations/ $(PI_HOST):$(PI_DIR)/migrations/

deploy-docker:
	@echo "Syncing docker/ source to Pi (excludes workspace files)..."
	rsync -av --delete \
		--exclude '__pycache__' --exclude '*.pyc' --exclude 'node_modules' \
		docker/ $(PI_HOST):$(PI_DIR)/docker/
	ssh $(PI_HOST) "cd $(PI_DIR)/docker && docker compose up -d --build"

deploy: deploy-binary deploy-ui deploy-migrations deploy-docker
	@echo "Full deploy complete. Checking health..."
	@sleep 5
	@ssh $(PI_HOST) "curl -sf -H 'Authorization: Bearer $(AUTH)' http://localhost:18789/api/doctor | python3 -m json.tool"

# ── Remote ───────────────────────────────────────────────────────────────────

doctor:
	@ssh $(PI_HOST) "curl -sf -H 'Authorization: Bearer $(AUTH)' http://localhost:18789/api/doctor | python3 -m json.tool"

logs:
	ssh $(PI_HOST) "journalctl --user -u hydeclaw-core -f --no-pager"

restart:
	ssh $(PI_HOST) "systemctl --user restart hydeclaw-core"

status:
	ssh $(PI_HOST) "systemctl --user status hydeclaw-core --no-pager"

# ── Cleanup ──────────────────────────────────────────────────────────────────

clean:
	cargo clean
	rm -rf ui/out ui/.next
