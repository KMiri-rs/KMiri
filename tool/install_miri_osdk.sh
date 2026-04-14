#!/bin/bash

set -eoux pipefail

# Install miri.
cd /KMiri/kmiri
cargo clean
./miri install --debug --features=tracing

cd /KMiri/asterinas
make install_osdk
