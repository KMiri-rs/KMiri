.PHONY: init sync status build run stop gen check

# ====== repo management ======

init:
	repo init -u git@github.com:KMiri-rs/vivoblueos-manifests.git -b kmiri -m manifest.xml && repo sync

sync:
	repo sync

status:
	repo status

# ====== docker management ======

# Run this after Dockerfile is changed.
build:
	docker compose build

run:
	@docker compose ps --status running -q vivo-blueos | grep -q . \
	  || docker compose up -d
	docker compose exec vivo-blueos bash

stop:
	docker compose stop

# ====== kernel management ======

BOARD := qemu_riscv64
BUILD_MODE := debug
OUT_DIR := out/${BOARD}.${BUILD_MODE}

gen:
	gn gen ${OUT_DIR} --args='build_type="${BUILD_MODE}" board="${BOARD}"'

check: gen
	ninja -C ${OUT_DIR} check_all
