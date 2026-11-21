#!/usr/bin/env python3
"""Run the `#[ktest]`s of one crate under KMiri and record one result per test.

    ktest-sweep.py CRATE_DIR OUT_DIR [TEST ...]

Runs `cargo osdk miri [TEST ...]` in CRATE_DIR (all tests if none are given).
KMiri stops the whole kernel at the first Undefined Behavior or unsupported
operation, while natively only that test would fail. So when the kernel dies
inside a test, or a test makes no progress for PER_TEST seconds, that test is
recorded and the sweep starts a new kernel with the tests after it, in the
order the runner listed them (`KTEST_LIST` lines).

Results go to OUT_DIR/ledger.tsv (name, status, run, detail), with status one of
ok, FAILED (the test panicked), ABORT (the kernel died in the test) and TIMEOUT.
Each kernel's output is OUT_DIR/runNNN.log. The environment (PATH with
cargo-osdk, MIRIFLAGS, OSDK_LOCAL_DEV, ...) is passed through unchanged.
"""
import os, re, signal, subprocess, sys, time

PER_TEST = int(os.environ.get("PER_TEST", "300"))
BUILD_TIMEOUT = int(os.environ.get("BUILD_TIMEOUT", "1500"))
ANSI = re.compile(r"\x1b\[[0-9;]*m")
TEST_LINE = re.compile(r"^test (\S+) \.\.\.(.*)$", re.M)


def run(crate_dir, out_dir, names, idx):
    """Runs one kernel. Returns (log path, 'done' or 'stuck', exit code)."""
    log = os.path.join(out_dir, f"run{idx:03d}.log")
    with open(log, "wb") as f:
        p = subprocess.Popen(["cargo", "osdk", "miri"] + names, cwd=crate_dir, stdout=f,
                             stderr=subprocess.STDOUT, start_new_session=True)
        last, last_t, state = None, time.time(), "done"
        while p.poll() is None:
            time.sleep(3)
            started = TEST_LINE.findall(read(log))
            mark = (len(started), started[-1][0] if started else None)
            if mark != last:
                last, last_t = mark, time.time()
            if time.time() - last_t > (PER_TEST if started else BUILD_TIMEOUT):
                os.killpg(p.pid, signal.SIGKILL)
                p.wait()
                state = "stuck"
                break
    return log, state, p.returncode


def read(log):
    return ANSI.sub("", open(log, "rb").read().decode("utf-8", "replace"))


def parse(text):
    """Returns (listed tests, [(test, ok|FAILED|INCOMPLETE)], first error line)."""
    listed = re.findall(r"^KTEST_LIST (\S+)", text, re.M)
    results = []
    for m in TEST_LINE.finditer(text):
        tail = m.group(2)
        status = "ok" if re.search(r"\bok\s*$", tail) else "FAILED" if "FAILED" in tail else None
        if status is None:  # The test printed something; its result is on a later line.
            rest = text[m.end():]
            nxt = TEST_LINE.search(rest)
            found = re.search(r"^ ?(ok|FAILED)\s*$", rest[:nxt.start()] if nxt else rest, re.M)
            status = found.group(1) if found else "INCOMPLETE"
        results.append((m.group(1), status))
    err = re.search(r"^(error: .*|.*Undefined Behavior: .*|thread 'rustc'.* panicked at .*)$",
                    text, re.M)
    return listed, results, err.group(1)[:230] if err else ""


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    crate_dir, out_dir, names = sys.argv[1], sys.argv[2], sys.argv[3:]
    os.makedirs(out_dir, exist_ok=True)
    ledger = open(os.path.join(out_dir, "ledger.tsv"), "a")

    def record(name, status, idx, detail=""):
        ledger.write(f"{name}\t{status}\trun{idx:03d}\t{detail}\n")
        ledger.flush()
        print(f"{status:9s} {name}  {detail[:150]}", flush=True)

    order, idx = None, 0
    while True:
        log, state, rc = run(crate_dir, out_dir, names, idx)
        text = read(log)
        listed, results, err = parse(text)
        if order is None:
            if not listed:
                sys.exit(f"no KTEST_LIST in {log}: {err}")
            # Like the native runner's whitelist, a name selects the tests whose path
            # ends with it, compared by whole `::` segments.
            order = [n for n in listed
                     if not names or any(n == w or n.endswith("::" + w) for w in names)]
        panics = dict(re.findall(r"^KTEST_PANIC (\S+) :: (.*)$", text, re.M))
        for name, status in results:
            if status != "INCOMPLETE":
                record(name, status, idx, panics.get(name, ""))
        incomplete = [n for n, s in results if s == "INCOMPLETE"]
        if incomplete:
            detail = f"no progress for {PER_TEST}s" if state == "stuck" else err
            record(incomplete[-1], "TIMEOUT" if state == "stuck" else "ABORT", idx, detail)
        elif state != "done" or (rc != 0 and "power off: " not in text):
            record("<runner>", "RUNNER-" + state, idx, f"rc={rc} {err}")
        if not results or results[-1][0] not in order:
            break
        names = order[order.index(results[-1][0]) + 1:]
        if not names:
            break
        idx += 1


if __name__ == "__main__":
    main()
