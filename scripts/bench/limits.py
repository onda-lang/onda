"""Sequential child execution with persisted logs and enforced resource limits."""
from dataclasses import dataclass
import json
import os
from pathlib import Path
import resource
import signal
import subprocess
import uuid


@dataclass(frozen=True)
class Limits:
    cpu: int
    memory_mib: int = 2048
    timeout_seconds: int = 30

    def run(self, command, prefix: Path, *, virtual_memory=False, env=None):
        command = [str(x) for x in command]
        unit = None
        if virtual_memory:
            # V8 reserves a large address space. Bound committed memory with a cgroup,
            # rather than applying an address-space limit that prevents V8 startup.
            unit = f"onda-bench-{uuid.uuid4().hex}"
            command = ["systemd-run", "--user", "--quiet", "--wait", "--pipe", "--collect",
                       "--service-type=exec", f"--unit={unit}",
                       "-p", f"MemoryMax={self.memory_mib}M", "-p", "MemorySwapMax=0",
                       "-p", "TasksMax=64", "-p", f"RuntimeMaxSec={self.timeout_seconds}s",
                       "-p", f"LimitCPU={self.timeout_seconds}", "-p", "LimitCORE=0",
                       "-p", f"CPUAffinity={self.cpu}", *command]
        prefix.with_suffix(".command.json").write_text(json.dumps(command, indent=2) + "\n")

        def prepare():
            if hasattr(os, "sched_setaffinity"):
                os.sched_setaffinity(0, {self.cpu})
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
            resource.setrlimit(resource.RLIMIT_CPU, (self.timeout_seconds, self.timeout_seconds))
            if not virtual_memory:
                size = self.memory_mib * 1024 * 1024
                resource.setrlimit(resource.RLIMIT_AS, (size, size))

        with prefix.with_suffix(".stdout.log").open("w") as stdout, prefix.with_suffix(".stderr.log").open("w") as stderr:
            process = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=env,
                                       start_new_session=True, preexec_fn=prepare)
            try:
                status = process.wait(timeout=self.timeout_seconds + (5 if unit else 0))
            except BaseException:
                if unit:
                    try:
                        subprocess.run(["systemctl", "--user", "stop", unit], timeout=5,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
                    except (OSError, subprocess.TimeoutExpired):
                        # RuntimeMaxSec still applies if the control connection fails.
                        pass
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
                raise
        if status:
            raise RuntimeError(f"child exited with status {status}; see {prefix.with_suffix('.stderr.log')}")
        return prefix.with_suffix(".stdout.log").read_text()
