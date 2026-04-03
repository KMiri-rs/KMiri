#!/bin/bash

set -eoux pipefail

cd /KMiri/kmiri
./miri install --debug

cd /KMiri/asterinas
make install_osdk
