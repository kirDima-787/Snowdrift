"""Packs the app icon at the sizes Windows asks for into assets/Snowdrift.ico, as PNG-compressed images.

Usage: python3 scripts/make-ico.py  (run from windows/, needs macOS sips)
"""
import os
import struct
import subprocess
import tempfile

SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
SOURCE = "../site/assets/icon-512.png"
TARGET = "assets/Snowdrift.ico"

images = []
with tempfile.TemporaryDirectory() as folder:
    for size in SIZES:
        path = os.path.join(folder, f"{size}.png")
        subprocess.run(["sips", "-z", str(size), str(size), SOURCE, "--out", path], check=True, capture_output=True)
        with open(path, "rb") as file:
            images.append((size, file.read()))

offset = 6 + 16 * len(images)
directory = struct.pack("<HHH", 0, 1, len(images))
for size, data in images:
    edge = 0 if size >= 256 else size
    directory += struct.pack("<BBBBHHII", edge, edge, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
with open(TARGET, "wb") as file:
    file.write(directory + b"".join(data for _, data in images))
print(f"{TARGET}: {', '.join(str(size) for size, _ in images)}")
