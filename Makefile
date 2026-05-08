.PHONY: all atelier web deploy deploy-web install-service logs clean test

ATELIER_BIN := /opt/atelier/bin/atelier
RELEASE_BIN := target/release/atelier
WEB_DIST := web/dist
WEB_DEST := /opt/atelier/web/dist

all: atelier web

atelier:
	cargo build --release -p atelier

web:
	cd web && npm run build

deploy-web: web
	sudo install -d -m 0755 /opt/atelier/web
	sudo rsync -aH --delete $(WEB_DIST)/ $(WEB_DEST)/

deploy: atelier deploy-web
	sudo install -d -m 0755 /opt/atelier/bin /opt/atelier/data /var/lib/atelier
	sudo install -m 0755 $(RELEASE_BIN) $(ATELIER_BIN)
	sudo systemctl restart atelier.service
	sleep 1
	curl -fsS http://127.0.0.1:4100/api/health | jq . || (sudo journalctl -u atelier -n 30 --no-pager; exit 1)

install-service:
	sudo install -m 0644 systemd/atelier.service /etc/systemd/system/atelier.service
	sudo install -m 0644 systemd/atelier-sync-docs.service /etc/systemd/system/atelier-sync-docs.service
	sudo install -m 0644 systemd/atelier-sync-docs.timer /etc/systemd/system/atelier-sync-docs.timer
	sudo install -m 0644 systemd/atelier-sync-store.service /etc/systemd/system/atelier-sync-store.service
	sudo install -m 0644 systemd/atelier-sync-store.timer /etc/systemd/system/atelier-sync-store.timer
	sudo install -m 0644 systemd/atelier-sync-git.service /etc/systemd/system/atelier-sync-git.service
	sudo install -m 0644 systemd/atelier-sync-git.timer /etc/systemd/system/atelier-sync-git.timer
	sudo install -d -m 0755 /opt/atelier/bin
	sudo install -m 0755 scripts/sync-docs.sh /opt/atelier/bin/sync-docs.sh
	sudo install -m 0755 scripts/sync-store.sh /opt/atelier/bin/sync-store.sh
	sudo install -m 0755 scripts/sync-git.sh /opt/atelier/bin/sync-git.sh
	sudo systemctl daemon-reload
	sudo systemctl enable atelier.service
	sudo systemctl enable --now atelier-sync-docs.timer
	sudo systemctl enable --now atelier-sync-store.timer
	sudo systemctl enable --now atelier-sync-git.timer

logs:
	journalctl -u atelier -f

test:
	cargo test --workspace

clean:
	cargo clean
