"""Terminate owned commands without leaving child processes behind."""

import os
import signal


def stop_process(process):
    if process.poll() is not None:
        return
    if os.name == "nt":
        import psutil

        parent = psutil.Process(process.pid)
        children = parent.children(recursive=True)
        for child in reversed(children):
            try:
                child.kill()
            except psutil.NoSuchProcess:
                pass
        process.kill()
        psutil.wait_procs(children, timeout=5)
    elif os.getpgid(process.pid) == process.pid:
        os.killpg(process.pid, signal.SIGKILL)
    else:
        process.kill()
    process.wait(timeout=10)
