#!/bin/bash

set -eoux pipefail

CWD=${CWD:=$PWD}

# Install miri.
cd "${CWD}"/kmiri
# cargo clean
./miri install --debug

# Install osdk.
cd "${CWD}"/asterinas
# cargo clean
OSDK_LOCAL_DEV=1 make install_osdk

# Test if gdb works.
# cd "${CWD}"/tests/os
# OSDK_LOCAL_DEV=1 cargo osdk miri test
# OSDK_LOCAL_DEV=1 rust-gdb -x miri.gdb --args cargo osdk miri test
