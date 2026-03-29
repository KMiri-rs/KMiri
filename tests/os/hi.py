import gdb

class Hi(gdb.Command):
    def __init__(self):
        super().__init__("hi", gdb.COMMAND_USER)
        # inferior number as the key and value
        self.parent_to_child = {}
        self.target_exe_file_suffix = "miri"

        gdb.events.stop.connect(self.stop_handler)
        gdb.events.exited.connect(self.exit_handler)
        gdb.events.new_inferior.connect(self.new_inferior_handler)
        # gdb.events.new_progspace.connect(self.new_progspace_handler)
        # gdb.events.new_objfile.connect(self.new_objfile_handler)

    def invoke(self, args, from_tty):
        printInferior("invoke")

        gdb.execute("catch exec")
        gdb.execute("run")

    def stop_handler(self, event):
        printInferior("stop_handler")
        if not gdb.selected_inferior().progspace.filename.endswith(self.target_exe_file_suffix):
            gdb.execute("continue")

    def exit_handler(self, event):
        printInferior("exit_handler")

        alive = []
        dead = []
        # Purge outdated inferior pairs if parent or child is invalid or has exited.
        for inf in gdb.inferiors():
            num = inf.num
            if inf.is_valid() and inf.pid != 0:
                alive.append(num)
            else:
                for parent, child in self.parent_to_child.items():
                    if parent == num or child == num:
                        # The parent or child may be dead, but the array is keys to be removed.
                        dead.append(parent)
                        print(f"{parent} => {child} is outdated and removed for child {num}")

        for key in dead:
            del self.parent_to_child[key]

        if alive == []:
            return

        child = gdb.selected_inferior().num
        target = alive[0] # child's parent may be dead, so default to the earlist alive inferior
        for key, val in self.parent_to_child.items():
            if child == val:
                target = key

        print(f"back to {target}")
        # Cleanup: Remove the exited inferior slot to prevent GDB memory bloat/crash
        # gdb.post_event(lambda: gdb.execute(f"remove-inferiors {child}; inferior {parent}"))
        gdb.post_event(lambda: gdb.execute(f"inferior {target}"))

    def new_inferior_handler(self, event):
        printInferior(f"new_inferior_handler")
        parent = gdb.selected_inferior()
        child = event.inferior
        print(f"[New Inferior] {parent.num} => {child.num}")
        self.parent_to_child[parent.num] = child.num

    def new_progspace_handler(self, event):
        printInferior(f"new_progspace_handler")
        print(f"[new_progspace] progspace={event.progspace.filename}")

    def new_objfile_handler(self, event):
        printInferior(f"new_objfile_handler")
        print(f"[new_objfile] objfile={event.new_objfile.filename}")

def printInferior(s):
    inf = gdb.selected_inferior()
    filename = inf.progspace.filename
    num = inf.num
    pid = inf.pid
    print(f"[inferior={num} pid={pid}] [{s}] {filename}")

Hi()
