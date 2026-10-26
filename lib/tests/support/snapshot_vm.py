"""Cut power to disposable QEMU guests at OntoEnv publication boundaries.

Requires a Linux kernel with built-in ext4/virtio, static busybox, cpio,
mkfs.ext4, QEMU and a Linux ontoenv unit-test executable. No mounts or root
privileges on the host; no host directory, network or device is exposed to guests.
All images/logs are retained in a new directory below --work.
"""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import tempfile
import time


PHASES = ["ObjectReady", "ManifestWritten", "ManifestSynced", "ManifestDurable",
          "CurrentWritten", "CurrentSynced", "CurrentReplaced", "Durable", "Acknowledged"]

INIT = r"""#!/bin/busybox sh
export PATH=/bin
mount -t devtmpfs devtmpfs /dev
exec </dev/console >/dev/console 2>&1
mount -t proc proc /proc
mount -t sysfs sysfs /sys
for arg in $(cat /proc/cmdline); do
  case "$arg" in
    ontoenv_mode=*) export ONTOENV_VM_MODE=${arg#*=} ;;
    ontoenv_phase=*) export ONTOENV_VM_PHASE=${arg#*=} ;;
    ontoenv_expected=*) export ONTOENV_VM_EXPECTED=${arg#*=} ;;
  esac
done
mkdir -p /data
options=rw
if [ "$ONTOENV_VM_MODE" = verify ]; then options=ro; fi
mount -t ext4 -o "$options" /dev/vda /data || { echo ONTOENV_VM_MOUNT_FAILED; poweroff -f; }
/probe --ignored --exact snapshots::tests::durability_vm_worker --nocapture
status=$?
if [ "$ONTOENV_VM_MODE" = seed ]; then sync; fi
echo ONTOENV_VM_EXIT $status
poweroff -f
"""


def initrd(probe, destination):
    root = destination / "root"
    root.mkdir()
    for name in ["bin", "dev", "proc", "sys", "data", "tmp"]:
        (root / name).mkdir()
    shutil.copy2(shutil.which("busybox"), root / "bin/busybox")
    for applet in ["sh", "mount", "mkdir", "cat", "sync", "poweroff"]:
        (root / "bin" / applet).symlink_to("busybox")
    shutil.copy2(probe, root / "probe")
    libraries = subprocess.check_output(["ldd", str(probe)], text=True)
    for library in set(re.findall(r"(/[^\s()]+)", libraries)):
        target = root / library.lstrip("/")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(Path(library).resolve(), target)
    (root / "init").write_text(INIT)
    (root / "init").chmod(0o755)
    names = ["."] + [str(p.relative_to(root)) for p in root.rglob("*")]
    archive = subprocess.run(["cpio", "--quiet", "-o", "-H", "newc", "--null"],
                             input=("\0".join(names) + "\0").encode(), cwd=root,
                             stdout=subprocess.PIPE, check=True).stdout
    image = destination / "initrd.gz"
    image.write_bytes(gzip.compress(archive, mtime=0))
    return image


def boot(kernel, ramdisk, disk, mode, phase, expected, log, timeout):
    command = ["qemu-system-x86_64", "-no-user-config", "-nodefaults", "-display", "none",
               "-serial", "stdio", "-monitor", "none", "-nic", "none", "-no-reboot",
               "-accel", "tcg,thread=multi", "-cpu", "max", "-smp", "2", "-m", "512",
               "-kernel", str(kernel), "-initrd", str(ramdisk),
               "-append", f"console=ttyS0 panic=1 rdinit=/init ontoenv_mode={mode} "
                          f"ontoenv_phase={phase} ontoenv_expected={expected}",
               "-drive", f"if=none,id=data,file={disk},format=raw,cache=none",
               "-device", "virtio-blk-pci,drive=data", "-device", "virtio-rng-pci"]
    started = time.monotonic()
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    output = bytearray()
    cut = False
    try:
        while time.monotonic() - started < timeout:
            for key, _ in selector.select(timeout=0.25):
                data = os.read(key.fd, 65536)
                if not data:
                    selector.unregister(key.fileobj)
                    continue
                output.extend(data)
                if mode == "update" and f"ONTOENV_VM_CUT {phase}\r\n".encode() in output:
                    process.send_signal(signal.SIGKILL)
                    cut = True
            if process.poll() is not None and not selector.get_map():
                break
        else:
            raise TimeoutError(f"Guest timed out: {log}")
        if mode == "update":
            assert cut, f"Guest did not reach crash point: {log}"
        else:
            assert b"ONTOENV_VM_EXIT 0" in output, f"Guest failed: {log}"
        if mode == "seed":
            assert b"ONTOENV_VM_SEEDED" in output, f"No baseline: {log}"
        if mode == "verify":
            match = re.search(rb"ONTOENV_VM_VERIFIED ([12])", output)
            assert match, f"No verified version: {log}"
            return int(match.group(1))
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=10)
        selector.close()
        log.write_bytes(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--cycles", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=90)
    args = parser.parse_args()
    assert args.cycles > 0
    args.work.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="snapshot-vm-", dir=args.work)).resolve()
    print(f"Artifacts: {work}", flush=True)
    kernel, probe = args.kernel.resolve(), args.probe.resolve()
    ramdisk = initrd(probe, work)
    baseline = work / "baseline.img"
    with baseline.open("wb") as handle:
        handle.truncate(64 * 1024 * 1024)
    subprocess.run(["mkfs.ext4", "-q", "-F", str(baseline)], check=True)
    boot(kernel, ramdisk, baseline, "seed", "none", "none", work / "seed.log", args.timeout)
    print("Durable baseline created", flush=True)
    results = {"kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
               "probe_sha256": hashlib.sha256(probe.read_bytes()).hexdigest(),
               "qemu": subprocess.check_output(["qemu-system-x86_64", "--version"], text=True).splitlines()[0],
               "filesystem": "ext4 (mkfs defaults), guest journal recovery, application verification on read-only mount",
               "disk": "64 MiB raw image, virtio-blk, QEMU cache=none, no host mounts or network",
               "scope": "Guest kernel/page-cache loss. Host/device volatile caches are not power-cycled.",
               "runs": []}
    for cycle in range(args.cycles):
        for phase in PHASES:
            name = f"{cycle + 1}-{phase}"
            disk = work / f"{name}.img"
            shutil.copyfile(baseline, disk)
            boot(kernel, ramdisk, disk, "update", phase, "none", work / f"{name}-cut.log", args.timeout)
            expected = "new" if phase in ["Durable", "Acknowledged"] else "either" if phase == "CurrentReplaced" else "old"
            generation = boot(kernel, ramdisk, disk, "verify", phase, expected,
                              work / f"{name}-verify.log", args.timeout)
            results["runs"].append({"cycle": cycle + 1, "phase": phase, "expected": expected,
                                    "generation": generation, "verified": True})
            (work / "results.json").write_text(json.dumps(results, indent=2) + "\n")
            print(f"{name}: expected {expected}, verified coherent generation {generation}", flush=True)
    print(f"Passed {len(results['runs'])} VM cuts/reboots; results: {work / 'results.json'}", flush=True)


if __name__ == "__main__":
    main()
