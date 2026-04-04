dash -enabled off

handle SIGUSR1 noprint nostop pass
handle SIGCHLD nostop noprint pass

# set non-stop on
# set target-async on

set schedule-multiple on
set detach-on-fork off
# set detach-on-fork on

# set follow-exec-mode new
# set follow-exec-mode same

# set follow-fork-mode child

set debuginfod enabled off

source hi.py
hi

