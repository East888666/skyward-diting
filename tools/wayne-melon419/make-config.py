#!/usr/bin/env python3
"""Generate an experimental Wayne config for the patched Melon recovery.

This does not patch or compile a kernel, or embed the required RPMB library.
"""

import argparse
import json
from pathlib import Path
import re
import tomllib


def generate(text):
    config = tomllib.loads(text)
    if config["device"]["codename"] != "wayne":
        raise ValueError("only the inspected Wayne configuration is supported")
    if not config.get("recovery", {}).get("legacy_fbe"):
        raise ValueError("source configuration must enable legacy FBE")

    # Preserve the version properties, ownership, service readiness and groups.
    for name in ("manual_key_services", "legacy_fbe_auto"):
        text = re.sub(rf"^{name}\s*=.*\n", "", text, flags=re.M)
    text = text.replace(
        "[recovery]\n",
        "[recovery]\nmanual_key_services = true\nlegacy_fbe_auto = true\n",
        1,
    )
    start = text.index("[haptics]")
    end = text.index("\n[backup]", start)
    text = (
        text[:start]
        + '[haptics]\nclick = 0\n'
        + 'strength = { lightest = 0.5, strongest = 1.0 }\n'
        + text[end:]
    )
    text = text.replace(
        "# Verified LOS 18.1's 4.4 kernel exposes the legacy timed_output driver.\n"
        "# The Melon 4.19 kernel instead needs click=0 (FF_PERIODIC), but failed\n"
        "# to open these LOS keys even after its RPMB node alias was supplied.\n",
        "# Melon 4.19 uses its FF_PERIODIC waveform, selected by click=0.\n",
    )
    services = text.split("[[service]]")
    changed = 0
    for index in range(1, len(services)):
        block = services[index]
        service = tomllib.loads("[[service]]" + block)["service"][0]
        if service.get("path") != "/vendor/bin/qseecomd":
            continue
        if service.get("loader") != "/apex/com.android.runtime/bin/linker64":
            raise ValueError("unexpected qseecomd loader")
        libraries = ":".join(service["libraries"])
        if not re.fullmatch(r"[/A-Za-z0-9_.:-]+", libraries):
            raise ValueError("unexpected library search path")
        command = (
            "export LD_PRELOAD=/skyward/libwayne-rpmb-compat.so; "
            f"export LD_LIBRARY_PATH={libraries}; "
            "exec /apex/com.android.runtime/bin/linker64 /vendor/bin/qseecomd"
        )
        block = re.sub(
            r"^path = .*$",
            'path = "/system/bin/sh"\nargs = '
            + json.dumps(["-c", command]),
            block,
            flags=re.M,
        )
        services[index] = re.sub(
            r"^(?:loader|libraries) = .*\n", "", block, flags=re.M
        )
        changed += 1
    if changed != 1:
        raise ValueError("expected exactly one qseecomd service")
    text = "[[service]]".join(services)
    # The source header documents the verified 4.4 build, not this candidate.
    text = text[text.index("[device]"):]
    text = (
        "# EXPERIMENTAL: Wayne, patched Melon 4.19 and LOS 18.1 vendor.\n"
        "# Full file decryption has NOT been verified with this kernel patch.\n"
        "# Requires tools/wayne-419-legacy-ice.patch and the RPMB shim in ramdisk.\n"
        "# See tools/wayne-melon419/README.md; stock Melon is insufficient.\n\n"
        + text
    )
    result = tomllib.loads(text)
    assert result["recovery"]["legacy_fbe_auto"]
    assert result["recovery"]["manual_key_services"]
    assert result["haptics"]["click"] == 0
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source", type=Path,
        default=Path(__file__).resolve().parents[2] / "devices/wayne.toml",
    )
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    text = generate(args.source.read_text())
    args.out.parent.mkdir(parents=True, exist_ok=True)
    # Never replace the production config or a user's existing candidate.
    with args.out.open("x") as output:
        output.write(text)
    print(args.out)


if __name__ == "__main__":
    main()
