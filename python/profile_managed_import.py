"""Linux/glibc diagnostic of initial import through the normal Python API.

Usage: python -I python/profile_managed_import.py NEW_ROOT SOURCE_DIRECTORY
No readers or queries are created. Allocator trims are diagnostic only.
"""

import ctypes
import gc
import importlib.metadata
import json
import sys
from pathlib import Path
from time import perf_counter

import ontoenv
from ontoenv import OntoEnv


def memory(phase):
    result = {"phase": phase}
    for filename, wanted in [
        ("/proc/self/status", ("VmRSS:", "VmHWM:")),
        ("/proc/self/smaps_rollup", ("Rss:", "Pss:")),
    ]:
        for line in Path(filename).read_text().splitlines():
            if line.startswith(wanted):
                name, value, _ = line.split()
                result[name.rstrip(":")] = int(value)
    return result


def main():
    native_path = Path(ontoenv._native.__file__).resolve()
    if native_path.is_relative_to(Path(__file__).resolve().parents[1]):
        raise RuntimeError("Use python -I to profile the installed wheel, not the local extension")
    root = Path(sys.argv[1]).resolve()
    sources = Path(sys.argv[2]).resolve(strict=True)
    root.mkdir()
    libc = ctypes.CDLL(None)
    libc.malloc_trim.argtypes = [ctypes.c_size_t]
    libc.malloc_trim.restype = ctypes.c_int
    phases = [memory("before_connect")]
    writer = OntoEnv.connect(
        root, offline=True, strict=False, search_directories=[str(sources)]
    )
    phases.append(memory("after_connect"))
    started = perf_counter()
    writer.update()
    update_ms = (perf_counter() - started) * 1000
    phases.append(memory("after_update"))
    assert len(writer.get_ontology_names()) == 100
    libc.malloc_trim(0)
    phases.append(memory("live_environment_after_diagnostic_trim"))
    writer.close()
    del writer
    gc.collect()
    phases.append(memory("after_close"))
    libc.malloc_trim(0)
    phases.append(memory("after_diagnostic_trim"))
    print(json.dumps({
        "python": sys.version,
        "package_version": importlib.metadata.version("ontoenv"),
        "native_extension_path": str(native_path),
        "update_ms": update_ms,
        "phases_kib": phases,
    }, indent=2))


if __name__ == "__main__":
    main()
