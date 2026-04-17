#!/bin/bash

set -eoux pipefail

podman build -t gdb tool/gdb

# Map current local directory to /KMiri in the container.
# Allow to disable ASLR (address space randomization) by GDB.
podman run -d -v .:/KMiri \
  --cap-add=SYS_PTRACE --security-opt seccomp=unconfined \
  --replace --name kmiri localhost/gdb sleep infinity

podman exec -it kmiri fish
