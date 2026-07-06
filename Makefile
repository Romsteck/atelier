.PHONY: all atelier web web-deps runner runner-deps deploy deploy-local deploy-remote deploy-app fix-app-perms logs clean test help

# Atelier et ses sources vivent sur Medion (/home/romain/atelier), édité via
# code-server@romain (127.0.0.1:8081). `make deploy` build EN PLACE sur Medion
# et installe localement dans /opt/atelier (plus de cross-build/rsync distant).
# Le fallback `deploy-remote` (build local + rsync/SSH) reste pour un lancement
# hors Medion.

MEDION      ?= romain@10.0.0.254
# Build et runtime co-localisés sur Medion → healthcheck en loopback.
ATELIER_API ?= http://127.0.0.1:4100

ATELIER_BIN_LOCAL   := target/release/atelier
# dv-{slug} typed-client generator. Shipped to /opt/atelier/bin so it's on the
# host PATH for humans (+ legacy `hr-dv-codegen` symlink); the primary regen
# path is the in-process `dv_regen_client` MCP tool, which needs no binary.
CODEGEN_BIN_LOCAL   := target/release/atelier-dv-codegen
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
CODEGEN_BIN_DST := $(PREFIX)/bin/atelier-dv-codegen
WEB_DIST_DST := $(PREFIX)/web/dist
SHIPPER_DST  := $(PREFIX)/crates/atelier-logging-shipper
RUNNER_DST   := $(PREFIX)/runner

IS_MEDION := $(shell [ "$$(uname -n)" = medion ] && echo yes || echo no)

help:
	@echo "Targets:"
	@echo "  atelier            cargo build --release -p atelier (local)"
	@echo "  web                npm ci (si besoin) + 2 builds Vite : homepage (dist) + Studio (dist/studio)"
	@echo "  deploy             build + install dans /opt/atelier + restart atelier.service"
	@echo "                     (en place sur Medion, sinon fallback rsync/SSH)"
	@echo "  deploy-app SLUG=x  build app x + restart via API (cf. scripts/deploy-app.sh)"
	@echo "  fix-app-perms      one-time: re-normalise l'ownership des arbres apps (hr-studio)"
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
	cargo build --release -p atelier -p atelier-dv-codegen

# node_modules est gitignoré (non versionné) → install des deps avant le build.
# `npm ci` est reproductible (piloté par package-lock.json) ; web/.npmrc porte
# legacy-peer-deps=true (conflit eslint v10 / eslint-plugin-react). On (ré)installe
# seulement si node_modules manque ou si le lockfile est plus récent.
web-deps:
	cd web && { [ -d node_modules ] && [ node_modules -nt package-lock.json ] || npm ci; }

# Deux builds Vite SÉPARÉS partageant web/src/ : la homepage (base /, → dist/) puis
# le Studio (base /studio/, → dist/studio/). ORDRE IMPÉRATIF : la homepage d'abord
# (son emptyOutDir vide dist/, donc dist/studio/ aussi), le Studio ensuite.
web: web-deps
	cd web && CI=1 npm run build && CI=1 npm run build:studio

# Runner Node : npm ci (reproductible). JAMAIS --omit=optional → le binaire natif
# linux-x64 du SDK est une optional-dep ; sans lui le runner échoue au runtime.
runner-deps:
	cd runner && { [ -d node_modules ] && [ node_modules -nt package-lock.json ] || npm ci --omit=dev; }

runner: runner-deps
	@test -f runner/src/runner.js || { echo "error: runner/src/runner.js missing — aborting" >&2; exit 1; }
	@test -f runner/src/scan.js || { echo "error: runner/src/scan.js missing (surveillance scan runner) — aborting" >&2; exit 1; }
	@test -d $(RUNNER_SDK_NATIVE) || { echo "error: $(RUNNER_SDK_NATIVE) missing — le binaire natif est une optional-dep : relancer 'npm ci --omit=dev' SANS --omit=optional" >&2; exit 1; }

deploy:
ifeq ($(IS_MEDION),yes)
	@$(MAKE) deploy-local
else
	@$(MAKE) deploy-remote
endif

# Gardes pré-vol partagées deploy-local/deploy-remote : un rsync --delete sur un
# artefact absent effacerait la prod correspondante (web, /studio, runner).
define PREFLIGHT
	@test -x $(ATELIER_BIN_LOCAL) || { echo "error: $(ATELIER_BIN_LOCAL) missing — build failed?" >&2; exit 1; }
	@test -x $(CODEGEN_BIN_LOCAL) || { echo "error: $(CODEGEN_BIN_LOCAL) missing — build failed? (cargo build -p atelier-dv-codegen)" >&2; exit 1; }
	@test -s $(WEB_DIST_LOCAL)/index.html || { echo "error: $(WEB_DIST_LOCAL)/index.html missing/empty — aborting (a --delete rsync would wipe prod web)" >&2; exit 1; }
	@test -s $(WEB_DIST_LOCAL)/studio/studio.html || { echo "error: $(WEB_DIST_LOCAL)/studio/studio.html missing/empty — studio build absent (a --delete rsync would wipe prod /studio)" >&2; exit 1; }
	@test -f $(SHIPPER_CRATE_LOCAL)/Cargo.toml || { echo "error: $(SHIPPER_CRATE_LOCAL)/Cargo.toml missing — aborting" >&2; exit 1; }
	@test -d $(RUNNER_SDK_NATIVE) || { echo "error: $(RUNNER_SDK_NATIVE) missing — aborting (a --delete rsync would wipe prod runner)" >&2; exit 1; }
endef

# Healthcheck partagé : $(call HEALTHCHECK,<base-url>,<commande logs en cas d'échec>)
define HEALTHCHECK
	@echo "→ healthcheck (poll $(1)/api/health)"
	@for i in $$(seq 1 15); do \
	  if curl -fsS $(1)/api/health >/dev/null 2>&1; then \
	    echo "  atelier healthy after $${i}s"; exit 0; \
	  fi; \
	  sleep 1; \
	done; \
	echo "error: atelier healthcheck failed after 15s" >&2; \
	$(2); \
	exit 1
endef

# Build en place sur Medion + install locale (sudo) dans /opt/atelier.
deploy-local: atelier web runner
	$(PREFLIGHT)
	sudo install -d -o root -g root -m 0755 $(PREFIX)/bin $(PREFIX)/web $(PREFIX)/crates $(RUNNER_DST)
	@echo "→ install atelier binary (atomic: .new + rename)"
	sudo install -o root -g root -m 0755 $(ATELIER_BIN_LOCAL) $(BIN_DST).new
	sudo mv -f $(BIN_DST).new $(BIN_DST)
	@echo "→ install atelier-dv-codegen (+ legacy hr-dv-codegen symlink)"
	sudo install -o root -g root -m 0755 $(CODEGEN_BIN_LOCAL) $(CODEGEN_BIN_DST).new
	sudo mv -f $(CODEGEN_BIN_DST).new $(CODEGEN_BIN_DST)
	sudo ln -sfn atelier-dv-codegen $(PREFIX)/bin/hr-dv-codegen
	@echo "→ sync web/dist → $(WEB_DIST_DST)"
	sudo rsync -a --delete $(WEB_DIST_LOCAL)/ $(WEB_DIST_DST)/
	@echo "→ sync shipper crate → $(SHIPPER_DST) (path-dep absolu de 4 apps)"
	sudo rsync -a --delete --exclude=target --exclude=Cargo.lock $(SHIPPER_CRATE_LOCAL)/ $(SHIPPER_DST)/
	@echo "→ sync runner → $(RUNNER_DST) (Agent SDK Node, lu/exécuté par hr-studio)"
	sudo rsync -a --delete $(RUNNER_LOCAL)/src/ $(RUNNER_DST)/src/
	sudo rsync -a --delete $(RUNNER_LOCAL)/node_modules/ $(RUNNER_DST)/node_modules/
	sudo rsync -a $(RUNNER_LOCAL)/package.json $(RUNNER_LOCAL)/package-lock.json $(RUNNER_LOCAL)/.npmrc $(RUNNER_DST)/
	@echo "→ sync systemd unit → /etc/systemd/system/atelier.service (le repo est la source de vérité)"
	sudo install -o root -g root -m 0644 systemd/atelier.service /etc/systemd/system/atelier.service
	sudo systemctl daemon-reload
	@echo "→ restart atelier.service"
	sudo systemctl restart atelier.service
	$(call HEALTHCHECK,$(ATELIER_API),sudo journalctl -u atelier -n 30 --no-pager)

# Fallback legacy : build local puis rsync/SSH vers Medion (lancement hors Medion).
deploy-remote: atelier web runner
	$(PREFLIGHT)
	@echo "→ rsync atelier + atelier-dv-codegen binaries + web/dist to $(MEDION)"
	rsync -a --rsync-path='sudo rsync' $(ATELIER_BIN_LOCAL) $(MEDION):$(BIN_DST).new
	rsync -a --rsync-path='sudo rsync' $(CODEGEN_BIN_LOCAL) $(MEDION):$(CODEGEN_BIN_DST).new
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
	ssh $(MEDION) 'sudo install -o root -g root -m 0755 $(BIN_DST).new $(BIN_DST) && sudo rm -f $(BIN_DST).new && sudo install -o root -g root -m 0755 $(CODEGEN_BIN_DST).new $(CODEGEN_BIN_DST) && sudo rm -f $(CODEGEN_BIN_DST).new && sudo ln -sfn atelier-dv-codegen $(PREFIX)/bin/hr-dv-codegen && sudo systemctl restart atelier.service'
	$(call HEALTHCHECK,http://10.0.0.254:4100,ssh $(MEDION) 'sudo journalctl -u atelier -n 30 --no-pager')

deploy-app:
	@if [ -z "$(SLUG)" ]; then echo "error: SLUG=<x> required (e.g. make deploy-app SLUG=files)" >&2; exit 1; fi
	bash scripts/deploy-app.sh $(SLUG)

# One-time rattrapage : re-normalise l'ownership des arbres apps sur le user de
# build unifié (hr-studio), groupe-writable + setgid. À lancer une fois après
# être passé à ATELIER_BUILD_AS_USER=hr-studio, hors build en cours (BUILD_BUSY
# ne protège pas contre le chown). IO potentiellement lourde (node_modules/target).
fix-app-perms:
	@echo "→ chown -R hr-studio:hr-studio + chmod g+rwX + setgid on /var/lib/atelier/apps"
	sudo chown -R hr-studio:hr-studio /var/lib/atelier/apps
	sudo chmod -R g+rwX /var/lib/atelier/apps
	sudo find /var/lib/atelier/apps -type d -exec chmod g+s {} +
	@echo "  done — app trees are now hr-studio-owned, group-writable, setgid"

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
