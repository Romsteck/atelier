.PHONY: all atelier web deploy install-service logs clean test

ATELIER_BIN := /opt/atelier/bin/atelier
RELEASE_BIN := target/release/atelier

all: atelier

atelier:
	cargo build --release -p atelier

web:
	@if [ -f web/package.json ]; then cd web && npm run build; else echo "no web/ yet (Phase 2+)"; fi

deploy: atelier
	sudo install -d -m 0755 /opt/atelier/bin /opt/atelier/data /var/lib/atelier
	sudo install -m 0755 $(RELEASE_BIN) $(ATELIER_BIN)
	sudo systemctl restart atelier.service
	sleep 1
	curl -fsS http://127.0.0.1:4100/api/health | jq . || (sudo journalctl -u atelier -n 30 --no-pager; exit 1)

install-service:
	sudo install -m 0644 systemd/atelier.service /etc/systemd/system/atelier.service
	sudo systemctl daemon-reload
	sudo systemctl enable atelier.service

logs:
	journalctl -u atelier -f

test:
	cargo test --workspace

clean:
	cargo clean
