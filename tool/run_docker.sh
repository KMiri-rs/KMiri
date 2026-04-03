#!/bin/bash

set -eoux pipefail

podman build -t gdb tool/gdb
podman run -d -v .:/KMiri --replace --name kmiri localhost/gdb sleep infinity
podman exec -it kmiri fish
