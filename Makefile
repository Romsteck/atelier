.PHONY: all atelier web web-deps runner runner-deps deploy deploy-local deploy-remote deploy-app logs clean test help

# Atelier et ses sources vivent sur Medion (/home/romain/atelier), édité via
# code-server@romain (127.0.0.1:8081). `make deploy` build EN PLACE sur Medion
# et installe localement dans /opt/atelier (plus de cross-build/rsync distant).
# Le fallback `deploy-remote` (build local + rsync/SSH) reste pour un lancement
# hors Medion.

MEDION      ?= romain@10.0.0.254
# Build et runtime co-localisés sur Medion → healthcheck en loopback.
ATELIER_API ?= http://127.0.0.1:4100

ATELIER_BIN_LOCAL   := target/release/atelier
WEB_DIST_LOCAL      := web/dist
# App-side logging SDK. Standalone crate (not a workspace member) consumed by
# the apps via an ABSOLUTE path-dep `/opt/atelier/crates/atelier-logging-shipper`
# (files, home, wallet, myfrigo). Must be (re)copied there after every change,
# wherever the source lives.
SHIPPER_CRATE_LOCAL := crates/atelier-logging-shipper
# Runner Node (Claude Agent SDK) — shim qui pilote l'agent et stream du NDJSON.
# node_modules embarque le binaire natif linux-x64 du SDK → on le ship tel quel.
RUNNER_LOCAL        := runner
RUNNER_SDK_NATIVE   := runner/node_modules/@anthropic-ai/claude-agent-sdk-linux-x64

PREFIX       ?= /opt/atelier
BIN_DST      := $(PREFIX)/bin/atelier
WEB_DIST_DST := $(PREFIX)/web/dist
SHIPPER_DST  := $(PREFIX)/crates/atelier-logging-shipper
RUNNER_DST   := $(PREFIX)/runner

IS_MEDION := $(shell [ "$$(uname -n)" = medion ] && echo yes || echo no)

help:
	@echo "Targets:"
	@echo "  atelier            cargo build --release -p atelier (local)"
	@echo "  web                npm ci (si besoin) + build frontend (web/dist)"
	@echo "  deploy             build + install dans /opt/atelier + restart atelier.service"
	@echo "                     (en place sur Medion, sinon fallback rsync/SSH)"
	@echo "  deploy-app SLUG=x  build app x + restart via API (cf. scripts/deploy-app.sh)"
	@echo "  logs               tail journalctl atelier (local sur Medion, sinon SSH)"
	@echo "  test               cargo test --workspace"
	@echo "  clean              cargo clean"
	@echo ""
	@echo "Variables (override on command line):"
	@echo "  MEDION       (default: $(MEDION))   — cible SSH du fallback deploy-remote"
	@echo "  ATELIER_API  (default: $(ATELIER_API))"
	@echo "  PREFIX       (default: $(PREFIX))"
	@echo "  IS_MEDION    (auto: $(IS_MEDION))"

all: atelier web runner

atelier:
	cargo build --release -p atelier

# node_modules est gitignoré (non versionné) → install des deps avant le build.
# `npm ci` est reproductible (piloté par package-lock.json) ; web/.npmrc porte
# legacy-peer-deps=true (conflit eslint v10 / eslint-plugin-react). On (ré)installe
# seulement si node_modules manque ou si le lockfile est plus récent.
web-deps:
	cd web && { [ -d node_modules ] && [ node_modules -nt package-lock.json ] || npm ci; }

web: web-deps
	cd web && CI=1 npm run build

# Runner Node : npm ci (reproductible). JAMAIS --omit=optional → le binaire natif
# linux-x64 du SDK est une optional-dep ; sans lui le runner échoue au runtime.
runner-deps:
	cd runner && { [ -d node_modules ] && [ node_modules -nt package-lock.json ] || npm ci --omit=dev; }

runner: runner-deps
	@test -f runner/src/runner.js || { echo "error: runner/src/runner.js missing — aborting" >&2; exit 1; }
	@test -d $(RUNNER_SDK_NATIVE) || { echo "error: $(RUNNER_SDK_NATIVE) missing (npm ci --omit=optional?) — aborting" >&2; exit 1; }

deploy:
ifeq ($(IS_MEDION),yes)
	@$(MAKE) deploy-local
else
	@$(MAKE) deploy-remote
endif

# Build en place sur Medion + install locale (sudo) dans /opt/atelier.
deploy-local: atelier web runner
	@test -x $(ATELIER_BIN_LOCAL) || { echo "error: $(ATELIER_BIN_LOCAL) missing — build failed?" >&2; exit 1; }
	@test -s $(WEB_DIST_LOCAL)/index.html || { echo "error: $(WEB_DIST_LOCAL)/index.html missing/empty — aborting (a --delete rsync would wipe prod web)" >&2; exit 1; }
	@test -f $(SHIPPER_CRATE_LOCAL)/Cargo.toml || { echo "error: $(SHIPPER_CRATE_LOCAL)/Cargo.toml missing — aborting" >&2; exit 1; }
	@test -d $(RUNNER_SDK_NATIVE) || { echo "error: $(RUNNER_SDK_NATIVE) missing — aborting (a --delete rsync would wipe prod runner)" >&2; exit 1; }
	sudo install -d -o root -g root -m 0755 $(PREFIX)/bin $(PREFIX)/web $(PREFIX)/crates $(RUNNER_DST)
	@echo "→ install atelier binary (atomic: .new + rename)"
	sudo install -o root -g root -m 0755 $(ATELIER_BIN_LOCAL) $(BIN_DST).new
	sudo mv -f $(BIN_DST).new $(BIN_DST)
	@echo "→ sync web/dist → $(WEB_DIST_DST)"
	sudo rsync -a --delete $(WEB_DIST_LOCAL)/ $(WEB_DIST_DST)/
	@echo "→ sync shipper crate → $(SHIPPER_DST) (path-dep absolu de 4 apps)"
	sudo rsync -a --delete --exclude=target --exclude=Cargo.lock $(SHIPPER_CRATE_LOCAL)/ $(SHIPPER_DST)/
	@echo "→ sync runner → $(RUNNER_DST) (Agent SDK Node, lu/exécuté par hr-studio)"
	sudo rsync -a --delete $(RUNNER_LOCAL)/src/ $(RUNNER_DST)/src/
	sudo rsync -a --delete $(RUNNER_LOCAL)/node_modules/ $(RUNNER_DST)/node_modules/
	sudo rsync -a $(RUNNER_LOCAL)/package.json $(RUNNER_LOCAL)/package-lock.json $(RUNNER_LOCAL)/.npmrc $(RUNNER_DST)/
	@echo "→ restart atelier.service"
	sudo systemctl restart atelier.service
	@echo "→ healthcheck (poll $(ATELIER_API)/api/health)"
	@for i in $$(seq 1 15); do \
	  if curl -fsS $(ATELIER_API)/api/health >/dev/null 2>&1; then \
	    echo "  atelier healthy after $${i}s"; exit 0; \
	  fi; \
	  sleep 1; \
	done; \
	echo "error: atelier healthcheck failed after 15s" >&2; \
	sudo journalctl -u atelier -n 30 --no-pager; \
	exit 1

# Fallback legacy : build local puis rsync/SSH vers Medion (lancement hors Medion).
deploy-remote: atelier web runner
	@test -x $(ATELIER_BIN_LOCAL) || { echo "error: $(ATELIER_BIN_LOCAL) missing — build failed?" >&2; exit 1; }
	@test -s $(WEB_DIST_LOCAL)/index.html || { echo "error: $(WEB_DIST_LOCAL)/index.html missing/empty — aborting" >&2; exit 1; }
	@test -f $(SHIPPER_CRATE_LOCAL)/Cargo.toml || { echo "error: $(SHIPPER_CRATE_LOCAL)/Cargo.toml missing — aborting" >&2; exit 1; }
	@test -d $(RUNNER_SDK_NATIVE) || { echo "error: $(RUNNER_SDK_NATIVE) missing — aborting" >&2; exit 1; }
	@echo "→ rsync atelier binary + web/dist to $(MEDION)"
	rsync -a --rsync-path='sudo rsync' $(ATELIER_BIN_LOCAL) $(MEDION):$(BIN_DST).new
	rsync -a --rsync-path='sudo rsync' --delete $(WEB_DIST_LOCAL)/ $(MEDION):$(WEB_DIST_DST)/
	ssh $(MEDION) 'sudo mkdir -p $(SHIPPER_DST)'
	rsync -a --rsync-path='sudo rsync' --delete --exclude=target --exclude=Cargo.lock \
	  $(SHIPPER_CRATE_LOCAL)/ $(MEDION):$(SHIPPER_DST)/
	@echo "→ rsync runner (Agent SDK Node, node_modules inclus) to $(MEDION)"
	ssh $(MEDION) 'sudo mkdir -p $(RUNNER_DST)'
	rsync -a --rsync-path='sudo rsync' --delete $(RUNNER_LOCAL)/src/ $(MEDION):$(RUNNER_DST)/src/
	rsync -a --rsync-path='sudo rsync' --delete $(RUNNER_LOCAL)/node_modules/ $(MEDION):$(RUNNER_DST)/node_modules/
	rsync -a --rsync-path='sudo rsync' $(RUNNER_LOCAL)/package.json $(RUNNER_LOCAL)/package-lock.json $(RUNNER_LOCAL)/.npmrc $(MEDION):$(RUNNER_DST)/
	@echo "→ atomic swap + restart atelier.service on $(MEDION)"
	ssh $(MEDION) 'sudo install -o root -g root -m 0755 $(BIN_DST).new $(BIN_DST) && sudo rm -f $(BIN_DST).new && sudo systemctl restart atelier.service'
	@echo "→ healthcheck (poll http://10.0.0.254:4100/api/health)"
	@for i in $$(seq 1 15); do \
	  if curl -fsS http://10.0.0.254:4100/api/health >/dev/null 2>&1; then \
	    echo "  atelier healthy after $${i}s"; exit 0; \
	  fi; \
	  sleep 1; \
	done; \
	echo "error: atelier healthcheck failed after 15s" >&2; \
	ssh $(MEDION) 'sudo journalctl -u atelier -n 30 --no-pager'; \
	exit 1

deploy-app:
	@if [ -z "$(SLUG)" ]; then echo "error: SLUG=<x> required (e.g. make deploy-app SLUG=files)" >&2; exit 1; fi
	bash scripts/deploy-app.sh $(SLUG)

logs:
ifeq ($(IS_MEDION),yes)
	sudo journalctl -u atelier -f
else
	ssh $(MEDION) 'sudo journalctl -u atelier -f'
endif

test:
	cargo test --workspace

clean:
	cargo clean
