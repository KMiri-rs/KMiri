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
        fname = filename(gdb.selected_inferior())
        if not is_miri(fname):
            gdb.execute("continue")
        else:
            print(f"😎 Reached miri: {fname}")

    def exit_handler(self, event):
        printInferior("exit_handler")
        # Cleanup: Remove the exited inferior slot to prevent GDB memory bloat/crash
        # gdb.post_event(lambda: gdb.execute(f"remove-inferiors {child}; inferior {parent}"))
        gdb.post_event(lambda: self.exit_to_another_inferior())

    def exit_to_another_inferior(self):
        self.update_inferiors()

        child = gdb.selected_inferior().num
        target = None
        if (val := self.inferior_to_be_returned(child)):
            target = val 
        elif (val := self.miri_inferior()):
            target = val
        else:
            target = self.newest_alive_inferior()
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

    def miri_inferior(self) -> int | None:
        """Return the living miri or cargo-miri inferior number"""
        miri = []
        cargo_miri = []
        for num, status in self.child_to_parent.items():
            if status.exited:
                continue
            if is_cargo_miri(status.filename):
                cargo_miri.append(num)
                continue
            if is_miri(status.filename):
                miri.append(num)
        print(f"miri={miri}\tcargo-miri={cargo_miri}")
        if miri:
            return max(miri)
        else:
            return num if cargo_miri and (num := max(cargo_miri)) else None

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
    return fname if (fname := inf.progspace.filename) else "Unknown"

def printInferior(s):
    inf = gdb.selected_inferior()
    num = inf.num
    pid = inf.pid
    print(f"[inferior={num} pid={pid}] [{s}] {filename(inf)}")


def is_miri(fname: str) -> bool:
    return fname.endswith("miri")

def is_cargo_miri(fname: str) -> bool:
    return fname.endswith("cargo-miri")

# cat /proc/2850975/cmdline | xargs -0
# `/home/gh-zjp-CN/.rustup/toolchains/nightly-2025-12-06-x86_64-unknown-linux-gnu/bin/cargo-miri runner /home/gh-zjp-CN/KMiri/tests/os/target/miri/x86_64-unknown-linux-gnu/debug/os-osdk-bin`

Hi()
