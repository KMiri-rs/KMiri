#!/bin/bash

set -eoux pipefail

cargo install --path . --debug --force

export LD_LIBRARY_PATH=$(rustc --print=sysroot)/lib
export LOG_FILE=$PWD/cargo-kmiri-helper.log
export DIR_ANALYSIS=$PWD/analysis

rm "$LOG_FILE"
touch "$LOG_FILE"
cargo clean

cargo kmiri-helper #-vvv
