"""Save recovery diagnostics on the host across USB disconnects/reboots."""

import argparse
import concurrent.futures
import datetime
import pathlib
import subprocess
import time
import uuid


def capture(adb, out, seconds):
    out.mkdir(parents=True, exist_ok=True)
    until = time.monotonic() + seconds
    session = max((int(p.name.split("-", 1)[0]) for p in out.iterdir()
                   if p.is_dir() and p.name.split("-", 1)[0].isdigit()), default=0)
    previous_boot = None
    with (out / "connections.log").open("a", encoding="utf-8", buffering=1) as connections:
        def note(message):
            connections.write(f"{datetime.datetime.now().isoformat()} {message}\n")

        def save(folder, name, command, timeout=8):
            with (folder / name).open("wb", buffering=0) as output:
                try:
                    result = subprocess.run(adb + ["exec-out", command], stdout=output,
                                            stderr=subprocess.STDOUT, timeout=timeout)
                    return result.returncode
                except subprocess.TimeoutExpired:
                    return -1

        while time.monotonic() < until:
            try:
                found = subprocess.run(adb + ["exec-out", "test -f /skyward/device.toml && cat /proc/sys/kernel/random/boot_id"],
                                       capture_output=True, timeout=3)
            except subprocess.TimeoutExpired:
                time.sleep(0.5)
                continue
            boot = found.stdout.decode("utf-8", errors="replace").strip()
            try:
                uuid.UUID(boot)
            except ValueError:
                time.sleep(0.5)
                continue
            if found.returncode != 0:
                time.sleep(0.5)
                continue
            session += 1
            folder = out / f"{session:03d}-{boot}"
            folder.mkdir()
            note(f"connected boot_id={boot} folder={folder.name}")
            if previous_boot is not None:
                note("boot ID changed: reboot confirmed" if boot != previous_boot
                     else "same boot ID: USB/stream reconnect only")
            previous_boot = boot
            streams = []
            files = []
            for name, command in [
                ("kernel-live.log", "test -f /skyward/device.toml && cat /dev/kmsg"),
                ("recovery-live.log", "test -f /skyward/device.toml && tail -n +1 -f /skyward/run/recovery.log"),
                ("install-live.log", "test -f /skyward/device.toml && { while [ ! -f /skyward/run/install-script.log ]; do sleep 1; done; tail -n +1 -f /skyward/run/install-script.log; }"),
                ("uptime-live.log", "test -f /skyward/device.toml && { while true; do cat /proc/sys/kernel/random/boot_id /proc/uptime; sleep 5; done; }"),
            ]:
                output = (folder / name).open("wb", buffering=0)
                files.append(output)
                streams.append(subprocess.Popen(adb + ["exec-out", command], stdout=output, stderr=subprocess.STDOUT))
            jobs = [
                ("recovery-snapshot.log", "cat /skyward/run/recovery.log"),
                ("kernel-snapshot.log", "dmesg"),
                ("state.txt", "cat /proc/sys/kernel/random/boot_id; cat /proc/uptime; sha256sum /system/bin/recovery; cat /proc/cmdline; getprop ro.boot.bootreason; getprop sys.boot.reason; ps -A; cat /proc/mounts"),
                ("pstore.log", "ls -la /sys/fs/pstore; cat /sys/fs/pstore/* /proc/last_kmsg"),
                ("device.toml", "cat /skyward/device.toml"),
                ("install-snapshot.log", "cat /skyward/run/install-script.log"),
                ("display-state.txt", "cat /sys/class/drm/card0-DSI-1/modes /sys/class/drm/card0-DSI-1/status /sys/class/drm/card0-DSI-1/enabled; cat /sys/kernel/debug/dri/0/state; for p in /sys/devices/system/cpu/cpufreq/policy*; do echo $p; cat $p/scaling_governor $p/scaling_cur_freq; done; for pid in $(pidof recovery); do for task in /proc/$pid/task/*; do echo $task; cat $task/comm $task/wchan $task/stack; done; done"),
            ]
            try:
                with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
                    futures = [pool.submit(save, folder, name, "test -f /skyward/device.toml && ( " + cmd + " )") for name, cmd in jobs]
                    # A dead stream causes a new snapshot. The boot ID
                    # distinguishes reboot from USB loss.
                    while time.monotonic() < until and all(p.poll() is None for p in streams):
                        time.sleep(0.25)
                    for future in futures:
                        future.result()
                if time.monotonic() >= until:
                    note("capture duration reached; stopping host streams")
            finally:
                for process in streams:
                    if process.poll() is None:
                        process.terminate()
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=3)
                for output in files:
                    output.close()
            note("stream ended codes=" + repr([p.returncode for p in streams]))
            time.sleep(0.5)
        note("capture finished")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--serial")
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--seconds", type=int, default=300)
    args = parser.parse_args()
    if args.seconds <= 0:
        parser.error("--seconds must be positive")
    command = [args.adb] + (["-s", args.serial] if args.serial else [])
    capture(command, args.out, args.seconds)
