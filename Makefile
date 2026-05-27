.PHONY: all atelier web deploy deploy-medion deploy-app logs clean test help

# Atelier vit désormais sur Medion. Les sources des apps restent sur
# CloudMaster (édition via code-server). Le Makefile build localement et
# pousse les artefacts vers Medion.

MEDION ?= romain@10.0.0.254
ATELIER_API ?= http://10.0.0.254:4100

ATELIER_BIN_LOCAL := target/release/atelier
ATELIER_FLOWD_BIN_LOCAL := target/release/atelier-flowd
WEB_DIST_LOCAL := web/dist

help:
	@echo "Targets:"
	@echo "  atelier            cargo build --release -p atelier (local)"
	@echo "  web                build frontend (web/dist) (local)"
	@echo "  deploy             build all + push binary + web/dist to Medion + restart atelier.service"
	@echo "  deploy-app SLUG=x  build + rsync app x to Medion + restart via API"
	@echo "  logs               tail journalctl atelier on Medion"
	@echo "  test               cargo test --workspace"
	@echo "  clean              cargo clean"
	@echo ""
	@echo "Variables (override on command line):"
	@echo "  MEDION       (default: $(MEDION))"
	@echo "  ATELIER_API  (default: $(ATELIER_API))"

all: atelier web

atelier:
	cargo build --release -p atelier

web:
	cd web && npm run build

# Push binary + frontend to Medion + restart Atelier service.
deploy: deploy-medion

deploy-medion: atelier web
	@test -x $(ATELIER_BIN_LOCAL) || { echo "error: $(ATELIER_BIN_LOCAL) missing — build failed?" >&2; exit 1; }
	@test -s $(WEB_DIST_LOCAL)/index.html || { echo "error: $(WEB_DIST_LOCAL)/index.html missing/empty — aborting (a --delete rsync would wipe prod web)" >&2; exit 1; }
	@echo "→ rsync atelier binary + web/dist to Medion"
	rsync -a --rsync-path='sudo rsync' $(ATELIER_BIN_LOCAL) $(MEDION):/opt/atelier/bin/atelier.new
	rsync -a --rsync-path='sudo rsync' --delete $(WEB_DIST_LOCAL)/ $(MEDION):/opt/atelier/web/dist/
	@echo "→ atomic swap + restart atelier.service on Medion"
	ssh $(MEDION) 'sudo install -o root -g root -m 0755 /opt/atelier/bin/atelier.new /opt/atelier/bin/atelier && sudo rm /opt/atelier/bin/atelier.new && sudo systemctl restart atelier.service'
	@echo "→ healthcheck (poll /api/health)"
	@for i in $$(seq 1 15); do \
	  if curl -fsS $(ATELIER_API)/api/health >/dev/null 2>&1; then \
	    echo "  atelier healthy after $${i}s"; exit 0; \
	  fi; \
	  sleep 1; \
	done; \
	echo "error: atelier healthcheck failed after 15s" >&2; \
	ssh $(MEDION) 'sudo journalctl -u atelier -n 30 --no-pager'; \
	exit 1

# Build + rsync + restart a single app on Medion.
deploy-app:
	@if [ -z "$(SLUG)" ]; then echo "error: SLUG=<x> required (e.g. make deploy-app SLUG=files)" >&2; exit 1; fi
	bash scripts/deploy-app.sh $(SLUG)

logs:
	ssh $(MEDION) 'sudo journalctl -u atelier -f'

test:
	cargo test --workspace

clean:
	cargo clean
