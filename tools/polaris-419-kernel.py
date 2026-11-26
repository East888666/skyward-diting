#!/usr/bin/env python3
"""Extract the tested Polaris 4.19 config or replace its Image, retaining DTBs.

This helper supports the known 31d0b8099432 recovery kernel only. The kernel
patch must be built separately; passing a stock kernel is not a fix.
"""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import zlib

STOCK_KERNEL_SHA256 = '5c319f0d109543da064a6405c11e9894dd5742c7d4582b5f45af1cf3a9ece93c'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def split(data):
    if data[:8] != b'ANDROID!':
        raise ValueError('not an Android boot image')
    u32 = lambda off: struct.unpack_from('<I', data, off)[0]
    page, ksize, rsize = u32(36), u32(8), u32(16)
    if u32(40) != 1 or u32(24) != 0 or u32(1632) != 0:
        raise ValueError('expected v1 without second stage or separate DTBO')
    if page < 2048 or page > 65536 or page & (page - 1):
        raise ValueError('invalid page size')
    rat = page + (ksize + page - 1) // page * page
    if rat + rsize > len(data):
        raise ValueError('truncated image')
    return page, data[page:page + ksize], data[rat:rat + rsize]


def unzip(kernel):
    decoder = zlib.decompressobj(16 + zlib.MAX_WBITS)
    raw = decoder.decompress(kernel) + decoder.flush()
    if not decoder.eof:
        raise ValueError('truncated gzip kernel')
    return raw, decoder.unused_data


def config(raw):
    start = raw.find(b'IKCFG_ST')
    end = raw.find(b'IKCFG_ED', start + 8)
    if start < 0 or end < 0:
        raise ValueError('embedded kernel config missing')
    return gzip.decompress(raw[start + 8:end])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['extract-config', 'replace'])
    parser.add_argument('--stock', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--kernel', type=Path, help='newly built Image.gz')
    parser.add_argument('--packer', type=Path, default=Path('target/release/skyward-pack'))
    args = parser.parse_args()
    if args.out.exists():
        parser.error('--out already exists; choose a new output path')
    original = args.stock.read_bytes()
    page, old_kernel, ramdisk = split(original)
    if sha(old_kernel) != STOCK_KERNEL_SHA256:
        parser.error('stock kernel does not match the tested 4.19 base')
    raw, tail = unzip(old_kernel)
    if args.action == 'extract-config':
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_bytes(config(raw))
        print(args.out)
        return
    if args.kernel is None:
        parser.error('replace needs --kernel Image.gz')
    at, count = 0, 0
    while at < len(tail):
        magic, size = struct.unpack_from('>II', tail, at)
        if magic != 0xd00dfeed or size < 40 or at + size > len(tail):
            raise ValueError('unrecognized appended device tree data')
        at += size
        count += 1
    if count != 14:
        raise ValueError('expected 14 original DTBs')
    new_gzip = args.kernel.read_bytes()
    new_raw, extra = unzip(new_gzip)
    if extra or b'4.19.325-cip130-st14-perf-skyward-legacyice' not in new_raw:
        raise ValueError('expected patched Image.gz without appended data')
    cfg = config(new_raw).decode().splitlines()
    for option in ['CONFIG_QTI_CRYPTO_TZ_LEGACY_ICE=y', 'CONFIG_ENABLE_LEGACY_PFK=y']:
        if option not in cfg:
            raise ValueError('required kernel setting missing: ' + option)
    kernel = new_gzip + tail
    draft = bytearray(original[:page])
    struct.pack_into('<I', draft, 8, len(kernel))
    draft += kernel
    draft += bytes((-len(draft)) % page)
    draft += ramdisk
    draft += bytes((-len(draft)) % page)
    if len(draft) > 67108864:
        raise ValueError('image exceeds the 64 MiB recovery partition')
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='polaris-419-') as temporary:
        draft_path = Path(temporary) / 'draft.img'
        draft_path.write_bytes(draft)
        subprocess.run([str(args.packer.resolve()), 'boot', '--kernel', str(draft_path),
                        '--ramdisk', str(args.stock), '--out', str(args.out)], check=True)
    final = args.out.read_bytes()
    _, fk, fr = split(final)
    if fk != kernel or fr != ramdisk:
        raise ValueError('repack changed kernel or ramdisk contents')
    manifest = dict(stock=str(args.stock), stock_sha256=sha(original),
                    output=str(args.out), output_sha256=sha(final),
                    output_size=len(final), kernel_sha256=sha(kernel),
                    ramdisk_sha256=sha(ramdisk), dtb_tail_sha256=sha(tail), dtbs=count)
    args.out.with_suffix(args.out.suffix + '.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(json.dumps(manifest, indent=2))


if __name__ == '__main__':
    main()
