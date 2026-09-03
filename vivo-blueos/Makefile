.PHONY: init sync status build run stop

init:
	repo init -u git@github.com:KMiri-rs/vivoblueos-manifests.git -b kmiri -m manifest.xml && repo sync

sync:
	repo sync

status:
	repo status

# Run this after Dockerfile is changed.
build:
	docker compose build

run:
	@docker compose ps --status running -q vivo-blueos | grep -q . \
	  || docker compose up -d
	docker compose exec vivo-blueos bash

stop:
	docker compose stop
