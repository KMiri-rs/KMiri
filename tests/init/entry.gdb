set environment OSDK_LOCAL_DEV=1
file cargo
set args osdk miri test
source ../../tool/gdb/miri.gdb
