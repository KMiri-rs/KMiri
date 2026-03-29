import gdb

class MiriCommand(gdb.Command):
    """
    Automates GDB to reach the 'miri' process.
    Optimized to prevent GDB segmentation faults in multi-process environments.
    """

    def __init__(self):
        super(MiriCommand, self).__init__("miri", gdb.COMMAND_USER)
        self.target_name = "miri"
        self.hooked = False

    def is_target(self, filename):
        return filename and self.target_name in filename.lower()

    def stop_handler(self, event):
        """Handle stop events (triggered by catch exec)"""
        if not self.hooked:
            return

        inf = gdb.selected_inferior()
        filename = inf.progspace.filename

        if self.is_target(filename):
            print(f"\n[OK] Found Miri: {filename}")
            self.cleanup()
        else:
            # Not miri, keep going.
            # We use post_event to let GDB finish its current event cycle.
            print(f"[Search] Skipping: {filename}")
            gdb.post_event(self.safe_continue)

    def exit_handler(self, event):
        """Handle process exit and return to parent"""
        if not self.hooked:
            return

        exited_inf = event.inferior
        print(f"[Info] Process {exited_inf.num} exited.")

        # Return to Inferior 1 (the main cargo/osdk process)
        parent_inf = None
        for inf in gdb.inferiors():
            print(f"[Debug] {inf.num}")
            if inf.num == 1 and inf.is_valid():
                parent_inf = inf
                break

        if parent_inf and parent_inf.pid != 0:
            # Switch to the parent and continue searching
            try:
                # Select the first available thread in the parent
                threads = list(parent_inf.threads())
                if threads:
                    threads[0].switch()
                    print(f"[>] Back to parent (Inferior {parent_inf.num}).")
                    gdb.post_event(self.safe_continue)
            except gdb.error:
                pass

        # Cleanup: Remove the exited inferior slot to prevent GDB memory bloat/crash
        gdb.post_event(lambda: gdb.execute(f"remove-inferiors {exited_inf.num}", to_string=True))

    def safe_continue(self):
        """Executes continue only if the session is still active"""
        try:
            if self.hooked:
                gdb.execute("continue")
        except gdb.error:
            pass

    def cleanup(self):
        """Disconnect all hooks"""
        if self.hooked:
            gdb.events.stop.disconnect(self.stop_handler)
            gdb.events.exited.disconnect(self.exit_handler)
            self.hooked = False
            # Optional: gdb.execute("delete checkpoints")
            print("[*] Miri capture hooks uninstalled.")

    def invoke(self, arg, from_tty):
        inf = gdb.selected_inferior()
        if not inf.progspace or not inf.progspace.filename:
            print("[Error] Load a file first.")
            return

        if self.hooked:
            print("Already running.")
            return

        # 1. Stability settings
        # gdb.execute("set confirmation off")
        gdb.execute("set follow-fork-mode child")
        gdb.execute("set detach-on-fork off")
        # Disable printing inferior exit/start messages to reduce internal stress
        gdb.execute("set print inferior-events off")
        gdb.execute("set print thread-events off")

        # 2. Setup catchpoint
        gdb.execute("catch exec")

        # 3. Hook events
        gdb.events.stop.connect(self.stop_handler)
        gdb.events.exited.connect(self.exit_handler)
        self.hooked = True

        print(f"[*] Searching for '{self.target_name}' recursively...")

        if inf.pid == 0:
            gdb.execute("run")
        else:
            gdb.execute("continue")

MiriCommand()
