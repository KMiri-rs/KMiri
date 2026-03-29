dash -enabled off

handle SIGUSR1 noprint nostop pass

set debuginfod enabled off
set detach-on-fork off
set follow-fork-mode child
set follow-exec-mode new
set schedule-multiple on

b miri::main
# src/bin/miri.rs:754
b miri::eval::create_ecx

source hi.py
hi

