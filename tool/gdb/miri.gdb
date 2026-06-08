# Disable dashboard.
dash -enabled off

# Run multiple subprocesses concurrently.
set schedule-multiple on
# Capture subprocesses.
set detach-on-fork off

# Don't download libc debug info.
set debuginfod enabled off

# Don't stop at these signals.
handle SIGUSR1 noprint nostop pass
handle SIGCHLD nostop noprint pass

# NOTE: run this script in each single test, i.e.
# KMiri/tests/xx $ OSDK_LOCAL_DEV=1 rust-gdb -x ../../tool/gdb/miri.gdb --args cargo osdk miri test
# because the code have encoded and relied on the exact relative path.
source ./../../tool/gdb/miri.py
miri run
