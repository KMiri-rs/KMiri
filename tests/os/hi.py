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

class Hi(gdb.Command):
    def __init__(self):
        super().__init__("hi", gdb.COMMAND_USER)
        # inferior number as the key and parent
        self.child_to_parent: Dict[int, ProcessStatus] = {}

        gdb.events.stop.connect(self.stop_handler)
        gdb.events.exited.connect(self.exit_handler)
        # gdb.events.new_inferior.connect(self.new_inferior_handler)
        gdb.events.selected_context.connect(self.on_selected_context)
        # gdb.events.before_prompt.connect(self.on_before_prompt)
        # gdb.events.new_progspace.connect(self.new_progspace_handler)
        # gdb.events.new_objfile.connect(self.new_objfile_handler)

    def invoke(self, arg, from_tty):
        printInferior("invoke")

        if not arg:
            gdb.execute("catch exec")
            gdb.execute("run")
            return

        if arg == "disconnect":
            gdb.events.stop.disconnect(self.stop_handler)
            gdb.events.exited.disconnect(self.exit_handler)
            # gdb.events.new_inferior.disconnect(self.new_inferior_handler)
            gdb.events.selected_context.disconnect(self.on_selected_context)

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
        # self.run_continue()
        gdb.post_event(lambda: self.run_continue())
        # cmdline = CmdLine.new(gdb.selected_inferior().pid)
        # if cmdline.is_miri_interested("os_osdk_bin"):
        #     print(f"😎 Reached miri: {pp(cmdline)}")
        #     return 
        # gdb.execute("continue")

    def on_before_prompt(self):
        printInferior("on_before_prompt")
        self.run_continue()

    def exit_handler(self, event):
        printInferior("exit_handler")
        # Cleanup: Remove the exited inferior slot to prevent GDB memory bloat/crash
        # current = event.inferior.num
        # gdb.execute(f"remove-inferiors {current}")
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
        # if not cmdline.is_miri():
        # gdb.post_event(lambda: gdb.execute("continue"))

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

    def new_inferior_handler(self, event):
        printInferior(f"new_inferior_handler")
        parent = gdb.selected_inferior()
        child = event.inferior
        print(f"[New Inferior] {parent.num} => {child.num}")
        self.child_to_parent[child.num] = ProcessStatus(
            exited=False, parent=parent.num, cmdline=CmdLine.new(parent.pid)
        )
        gdb.post_event(lambda: self.run_continue())

    def new_progspace_handler(self, event):
        printInferior(f"new_progspace_handler")
        print(f"[new_progspace] progspace={event.progspace.filename}")

    def new_objfile_handler(self, event):
        printInferior(f"new_objfile_handler")
        print(f"[new_objfile] objfile={event.new_objfile.filename}")

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
        # /home/gh-zjp-CN/.rustup/toolchains/nightly-2025-12-06-x86_64-unknown-linux-gnu/bin/miri --sysroot /home/gh-zjp-CN/.cache/miri --crate-name os_
        # osdk_bin --edition=2024 src/main.rs --diagnostic-width=160 --crate-type bin --emit=dep-info,link -C embed-bitcode=no -C debuginfo=2 --check-cf
        # g cfg(docsrs,test) --check-cfg cfg(feature, values()) -C metadata=1763759f804e0bb2 -C extra-filename=-abbb8042da03efe8 --out-dir /home/gh-zjp-
        # CN/KMiri/tests/os/target/miri/x86_64-unknown-linux-gnu/debug/deps --target x86_64-unknown-linux-gnu -C incremental=/home/gh-zjp-CN/KMiri/tests
        # /os/target/miri/x86_64-unknown-linux-gnu/debug/incremental -L dependency=/home/gh-zjp-CN/KMiri/tests/os/target/miri/x86_64-unknown-linux-gnu/d
        # ebug/deps -L dependency=/home/gh-zjp-CN/KMiri/tests/os/target/miri/debug/deps --extern noprelude:alloc=/home/gh-zjp-CN/KMiri/tests/os/target/m
        # iri/x86_64-unknown-linux-gnu/debug/deps/liballoc-f7053cd07531c1b0.rlib --extern noprelude:compiler_builtins=/home/gh-zjp-CN/KMiri/tests/os/tar
        # get/miri/x86_64-unknown-linux-gnu/debug/deps/libcompiler_builtins-ba64b2db9d19c20b.rlib --extern noprelude:core=/home/gh-zjp-CN/KMiri/tests/os
        # /target/miri/x86_64-unknown-linux-gnu/debug/deps/libcore-7e921fa91c0ac277.rlib --extern os=/home/gh-zjp-CN/KMiri/tests/os/target/miri/x86_64-u
        # nknown-linux-gnu/debug/deps/libos-fc0464f9eb396199.rlib --extern osdk_frame_allocator=/home/gh-zjp-CN/KMiri/tests/os/target/miri/x86_64-unknow
        # n-linux-gnu/debug/deps/libosdk_frame_allocator-b6e49ebd5aa7114f.rlib --extern osdk_heap_allocator=/home/gh-zjp-CN/KMiri/tests/os/target/miri/x
        # 86_64-unknown-linux-gnu/debug/deps/libosdk_heap_allocator-b57cfd8c86913aa3.rlib --extern osdk_test_kernel=/home/gh-zjp-CN/KMiri/tests/os/targe
        # t/miri/x86_64-unknown-linux-gnu/debug/deps/libosdk_test_kernel-37678a83309cf538.rlib --extern ostd=/home/gh-zjp-CN/KMiri/tests/os/target/miri/
        # x86_64-unknown-linux-gnu/debug/deps/libostd-314795e0586344e8.rlib -Z unstable-options --cfg ktest -C link-arg=-Tmiri.ld -C relocation-model=st
        # atic -C relro-level=off -C force-unwind-tables=yes --check-cfg cfg(ktest) -C no-redzone=y -C target-feature=+ermsb --
        return self.is_miri() and crate in self.cmdline

    def is_miri(self) -> bool:
        return self.exe.endswith("/miri")

    def is_cargo_miri(self) -> bool:
        return self.exe.endswith("/cargo-miri")

Hi()
