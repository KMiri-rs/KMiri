from dataclasses import dataclass, field
from typing import Dict
from pprint import pp
import gdb

@dataclass
class ProcessStatus:
    exited: bool
    filename: str
    parent: int | None = None

class Hi(gdb.Command):
    def __init__(self):
        super().__init__("hi", gdb.COMMAND_USER)
        # inferior number as the key and parent
        self.child_to_parent: Dict[int, ProcessStatus] = {}
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
        # Cleanup: Remove the exited inferior slot to prevent GDB memory bloat/crash
        # gdb.post_event(lambda: gdb.execute(f"remove-inferiors {child}; inferior {parent}"))
        gdb.post_event(lambda: self.exit_to_another_inferior())

    def exit_to_another_inferior(self):
        self.update_inferiors()

        child = gdb.selected_inferior().num
        target = val if (val := self.inferior_to_be_returned(child)) else self.newest_alive_inferior()
        if not target:
            print(f"[No target inferior to jump back for child {child}]")
            return

        print(f"back to {target}\n{pp(self.child_to_parent, sort_dicts=True)}")
        gdb.execute(f"inferior {target}")

    def update_inferiors(self):
        present = set()
        for inf in gdb.inferiors():
            num = inf.num
            present.add(num)
            item = self.child_to_parent.get(num)
            exited = not inf.is_valid() or inf.pid == 0
            if item:
                item.exited = exited
            else:
                self.child_to_parent[num] = ProcessStatus(exited, filename=filename(inf))
                # print(f"{parent} => {child} is outdated and removed for child {num}")

        remove = []
        for num in self.child_to_parent:
            if num not in present: remove.append(num)
        for num in remove:
            del self.child_to_parent[num]

    def inferior_to_be_returned(self, num: int) -> int | None:
        child = self.child_to_parent.get(num)
        if child is None: return None

        parentNum = child.parent
        parent = self.child_to_parent.get(parentNum)
        if parent is None: return None
        return self.inferior_to_be_returned(parentNum) if parent.exited else parentNum

    def newest_alive_inferior(self) -> int | None:
        """Choose the largest alive inferior."""
        alive = []
        for num, status in self.child_to_parent.items():
            if not status.exited: alive.append(num)
        return max(alive) if alive else None

    def new_inferior_handler(self, event):
        printInferior(f"new_inferior_handler")
        parent = gdb.selected_inferior()
        child = event.inferior
        print(f"[New Inferior] {parent.num} => {child.num}")
        self.child_to_parent[child.num] = ProcessStatus(exited=False, parent=parent.num, filename=filename(parent))

    def new_progspace_handler(self, event):
        printInferior(f"new_progspace_handler")
        print(f"[new_progspace] progspace={event.progspace.filename}")

    def new_objfile_handler(self, event):
        printInferior(f"new_objfile_handler")
        print(f"[new_objfile] objfile={event.new_objfile.filename}")

def filename(inf) -> str:
    return inf.progspace.filename

def printInferior(s):
    inf = gdb.selected_inferior()
    num = inf.num
    pid = inf.pid
    print(f"[inferior={num} pid={pid}] [{s}] {filename(inf)}")

Hi()
