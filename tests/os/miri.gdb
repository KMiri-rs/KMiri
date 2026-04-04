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

# Register the `hi` command.
source miri.py
miri

