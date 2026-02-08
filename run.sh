#!/bin/bash

set -eoux pipefail

podman build -t kmiri .
podman run -it -v .:/KMiri --replace --name kmiri localhost/kmiri:latest fish
