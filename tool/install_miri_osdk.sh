#!/bin/bash

set -eoux pipefail

# Install miri.
cd /KMiri/kmiri
cargo clean
./miri install --debug --features=tracing

# Install osdk.
cd /KMiri/asterinas
cargo clean
OSDK_LOCAL_DEV=1 make install_osdk

# Test if gdb works.
cd /KMiri/tests/os
OSDK_LOCAL_DEV=1 cargo osdk miri test
OSDK_LOCAL_DEV=1 rust-gdb -x miri.gdb --args cargo osdk miri test
