#!/bin/bash

set -eoux pipefail

cargo install --path . --debug --force

# the driver needs fresh build to work
cargo clean
cargo kmiri-helper #-vvv
