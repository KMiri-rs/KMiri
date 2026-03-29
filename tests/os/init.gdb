# This GDB supports auto-downloading debuginfo from the following URLs:
# https://debuginfod.ubuntu.com
# Enable debuginfod for this session? (y or [n])
set debuginfod enabled on

# stop at exec syscall (like invoking cargo)
# catch exec

# 发生 fork 时，不要放掉任何一个进程
# set detach-on-fork off
# set schedule-multiple on

# 默认留在父进程（Cargo）
# set follow-fork-mode parent
# jump to child process
#set follow-fork-mode child

# 
# set follow-exec-mode new

# 忽略 SIGUSR1 信号
handle SIGUSR1 noprint nostop pass

