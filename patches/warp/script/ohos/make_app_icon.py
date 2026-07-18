#!/usr/bin/env python3
"""Generate app_icon.png for OHOS HAP (flat PNG, same approach as Zed)."""
import struct, zlib, os

SRC = "/storage/Users/currentUser/workspace/warp-winit/app/channels/dev/icon/no-padding/512x512.png"
HAP_ENTRY = "/storage/Users/currentUser/workspace/warp-winit/app/src/platform/ohos/hap/entry/src/main/resources/base/media"
HAP_APPSCOPE = "/storage/Users/currentUser/workspace/warp-winit/app/src/platform/ohos/hap/AppScope/resources/base/media"

def read_png(path):
    with open(path, 'rb') as f:
        f.read(8)
        chunks = []
        while True:
            length = struct.unpack('>I', f.read(4))[0]
            ctype = f.read(4)
            data = f.read(length)
            f.read(4)
            chunks.append((ctype, data))
            if ctype == b'IEND':
                break
    for ct, data in chunks:
        if ct == b'IHDR':
            w, h, bd, ct = struct.unpack('>IIBB', data[:10])
            break
    raw = b''
    for ct, data in chunks:
        if ct == b'IDAT':
            raw += data
    return w, h, zlib.decompress(raw)

def write_png(path, w, h, pixels):
    def chunk(ctype, data):
        c = ctype + data
        return struct.pack('>I', len(data)) + c + struct.pack('>I', zlib.crc32(c) & 0xFFFFFFFF)
    header = chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0))
    raw = b''
    for y in range(h):
        raw += b'\x00' + pixels[y * w * 4:(y + 1) * w * 4]
    data = chunk(b'IDAT', zlib.compress(raw))
    end = chunk(b'IEND', b'')
    with open(path, 'wb') as f:
        f.write(b'\x89PNG\r\n\x1a\n')
        f.write(header + data + end)

sw, sh, pixels = read_png(SRC)
out = bytearray(256 * 256 * 4)
for ny in range(256):
    for nx in range(256):
        r = g = b = a = 0
        for dy in range(2):
            for dx in range(2):
                si = ((ny * 2 + dy) * 512 + (nx * 2 + dx)) * 4
                r += pixels[si]
                g += pixels[si+1]
                b += pixels[si+2]
                a += pixels[si+3]
        ni = (ny * 256 + nx) * 4
        out[ni:ni+4] = bytes([r // 4, g // 4, b // 4, a // 4])

app_icon = bytes(out)
for d in [HAP_APPSCOPE, HAP_ENTRY]:
    write_png(os.path.join(d, "app_icon.png"), 256, 256, app_icon)
    # remove old unused files
    for old in ["layered_image.json", "startIcon.png"]:
        p = os.path.join(d, old)
        if os.path.exists(p):
            os.remove(p)

print("Done!")
for d in [HAP_APPSCOPE, HAP_ENTRY]:
    p = os.path.join(d, "app_icon.png")
    print(f"  {p}  ({os.path.getsize(p)}B)")
