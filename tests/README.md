# Run with QEMU

`OVMF.fd` can be inaccessible due to permission on Ubuntu desktop. Or the file path differs on machines.

So set `$OVMF_FD=/path/to/OVMF.fd` before any qemu command. E.g.

```
cd os
export OVMF_FD=$PWD/../OVMF.fd
OSDK_LOCAL_DEV=1 cargo osdk run
```
