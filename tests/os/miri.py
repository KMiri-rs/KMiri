from __future__ import annotations
from dataclasses import dataclass, field
from typing import Dict
from pprint import pp
import textwrap
import gdb

@dataclass
class ProcessStatus:
    exited: bool
    cmdline: CmdLine
    parent: int | None = None

CMD = "miri"
ARG_RUN = "run"
ARG_DISCONNECT = "disconnect"
ARG_SET_BREAKPOINTS = "set-breakpoints"

class Miri(gdb.Command):
    def __init__(self):
        # The command name is `miri`.
        super().__init__(CMD, gdb.COMMAND_USER)
        # inferior number as the key and parent
        self.child_to_parent: Dict[int, ProcessStatus] = {}

        # Register event callbacks.
        gdb.events.stop.connect(self.stop_handler)
        gdb.events.exited.connect(self.exit_handler)
        gdb.events.selected_context.connect(self.on_selected_context)

    def invoke(self, arg, from_tty):
        # printInferior("invoke")

        # The entry point.
        if not arg:
            gdb.execute("catch exec")
            gdb.execute("run")
            return

        # Unregister event callbacks.
        if arg == ARG_DISCONNECT:
            gdb.events.stop.disconnect(self.stop_handler)
            gdb.events.exited.disconnect(self.exit_handler)
            gdb.events.selected_context.disconnect(self.on_selected_context)
            return
        
        if arg == ARG_SET_BREAKPOINTS:
            gdb.execute("break miri::main")
            gdb.execute("break miri::eval::create_ecx")
            return

        if arg == ARG_RUN:
            gdb.execute(f"{CMD} {ARG_DISCONNECT}")
            gdb.execute(f"{CMD} {ARG_SET_BREAKPOINTS}")

    def complete(self, text, word):
        word = word or ""
        candidates = [ARG_RUN, ARG_DISCONNECT, ARG_SET_BREAKPOINTS]
        return [c for c in candidates if c.startswith(word)]

    def run_continue(self):
        cmdline = CmdLine.new(gdb.selected_inferior().pid)
        if cmdline.is_miri_interested("os_osdk_bin"):
            print(f"😎 Reached miri: {pp(cmdline)}")
            return 
        try:
            gdb.execute("continue")
        except gdb.error as e:
            print(f"[continue error] {e}")

    def stop_handler(self, event):
        printInferior("stop_handler")
        # Execute `continue` in the stop event callback doesn't work.
        # We need to run `continue` in post_event.
        gdb.post_event(lambda: self.run_continue())

    def exit_handler(self, event):
        printInferior("exit_handler")
        # We don't need to remove-inferiors, because follow-exec-mode defaults 
        # to same, meaning dead process will be overridden or cleaned up during exec by GDB.
        # But we do need to return to a parent or proper process.
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
        
        cmdline = self.child_to_parent[target].cmdline
        print(f"back to {target}")
        # pp(self.child_to_parent, sort_dicts=True)
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
                self.child_to_parent[num] = ProcessStatus(exited, cmdline=CmdLine.new(inf.pid))

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
            if status.cmdline.is_cargo_miri():
                cargo_miri.append(num)
                continue
            if status.cmdline.is_miri():
                miri.append(num)
        print(f"miri={miri}\tcargo-miri={cargo_miri}")
        if miri:
            return max(miri)
        else:
            return num if cargo_miri and (num := max(cargo_miri)) else None

    def on_selected_context(self, event):
        print(f"[on_selected_context] current inferior: {event.inferior.num}, thread: {event.thread.num} frame: {event.frame.name}")
        self.run_continue()

def filename(inf) -> str:
    return fname if (fname := inf.progspace.filename) else "Unknown"

def printInferior(s):
    inf = gdb.selected_inferior()
    num = inf.num
    pid = inf.pid
    print(f"[inferior={num} pid={pid}] [{s}] {filename(inf)}")

# cat /proc/2850975/cmdline | xargs -0
# `/home/gh-zjp-CN/.rustup/toolchains/nightly-2025-12-06-x86_64-unknown-linux-gnu/bin/cargo-miri runner /home/gh-zjp-CN/KMiri/tests/os/target/miri/x86_64-unknown-linux-gnu/debug/os-osdk-bin`
@dataclass
class CmdLine:
    cmdline: str
    exe: str

    @classmethod
    def new(cls, pid: int) -> CmdLine | None:
        if pid == 0:
            print("Process not started")
            return None
        
        cmdline = ""
        # 直接读系统文件，不需要 GDB 暂停程序
        try:
            with open(f"/proc/{pid}/cmdline", "r") as f:
                # 替换 null 字符为空格
                cmdline = f.read().replace('\x00', ' ')
        except Exception as e:
            print(e)
            return None

        return cls(cmdline, exe=cmdline.split(" ")[0].strip())

    def is_miri_interested(self, crate: str) -> bool:
        # /home/gh-zjp-CN/.rustup/toolchains/nightly-2025-12-06-x86_64-unknown-linux-gnu/bin/miri --sysroot /home/gh-zjp-CN/.cache/miri
        # --crate-name os_osdk_bin ...
        return self.is_miri() and crate in self.cmdline

    def is_miri(self) -> bool:
        return self.exe.endswith("/miri")

    def is_cargo_miri(self) -> bool:
        return self.exe.endswith("/cargo-miri")

# Register the command.
Miri()
