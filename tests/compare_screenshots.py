"""Compare two screenshot directories (tests/screenshots.py output) pixel by pixel.

    python3 tests/compare_screenshots.py <dir-a> <dir-b> [diff-dir]

Prints, per image, the share of pixels that differ (after a small tolerance) and writes a diff image when a directory
is given. Exit code 1 when any image differs by more than 2% (timestamps and task ids are expected to differ a little).
"""
import os
import sys

from PIL import Image, ImageChops

a, b = sys.argv[1], sys.argv[2]
diff_dir = sys.argv[3] if len(sys.argv) > 3 else ""
if diff_dir:
    os.makedirs(diff_dir, exist_ok=True)
worst = 0.0
for name in sorted(os.listdir(a)):
    if not name.endswith(".png") or not os.path.exists(os.path.join(b, name)):
        print(f"{name:24s} missing in {b}" if name.endswith(".png") else "", end="\n" if name.endswith(".png") else "")
        continue
    ia = Image.open(os.path.join(a, name)).convert("RGB")
    ib = Image.open(os.path.join(b, name)).convert("RGB")
    if ia.size != ib.size:
        print(f"{name:24s} size differs {ia.size} vs {ib.size}")
        worst = max(worst, 1.0)
        continue
    d = ImageChops.difference(ia, ib).convert("L").point(lambda v: 255 if v > 24 else 0)
    changed = sum(1 for v in d.getdata() if v)
    share = changed / (ia.width * ia.height)
    worst = max(worst, share)
    print(f"{name:24s} {share * 100:6.2f}% pixels differ")
    if diff_dir and changed:
        d.save(os.path.join(diff_dir, name))
print(f"worst {worst * 100:.2f}%")
sys.exit(1 if worst > 0.02 else 0)
