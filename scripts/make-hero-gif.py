"""Turn vhs PNG frames into the README hero GIF.

Usage: python make_gif.py FRAMES_DIR OUT.gif [--fps 50] [--max-idle 1.6] [--hold 3]

vhs writes one PNG per frame. Runs of identical frames (the screen did not
change: the model thinking, a test run) are kept but capped at --max-idle
seconds, so the GIF never sits on a still screen for long. The last frame is
held for --hold seconds. ffmpeg then encodes with a generated palette.
"""

import argparse
import hashlib
import pathlib
import subprocess
import sys


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("frames")
    ap.add_argument("out")
    ap.add_argument("--fps", type=float, default=50.0, help="vhs capture framerate")
    ap.add_argument("--max-idle", type=float, default=1.6)
    ap.add_argument("--hold", type=float, default=3.0)
    a = ap.parse_args()

    frames = sorted(pathlib.Path(a.frames).glob("*.png"))
    if not frames:
        print("no frames", file=sys.stderr)
        return 1
    step = 1.0 / a.fps
    runs = []  # [path, seconds]
    last_hash = None
    for f in frames:
        h = hashlib.md5(f.read_bytes()).hexdigest()
        if h == last_hash:
            runs[-1][1] += step
        else:
            runs.append([f, step])
            last_hash = h
    total_in = len(frames) * step
    for r in runs:
        r[1] = min(r[1], a.max_idle)
    runs[-1][1] = a.hold
    total_out = sum(r[1] for r in runs)

    listing = pathlib.Path(a.out).with_suffix(".concat.txt")
    with open(listing, "w", encoding="utf-8") as fh:
        for path, secs in runs:
            fh.write(f"file '{path.resolve().as_posix()}'\nduration {secs:.4f}\n")
        fh.write(f"file '{runs[-1][0].resolve().as_posix()}'\n")  # concat needs the last file repeated

    vf = "fps=25,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle"
    cmd = ["ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", str(listing), "-vf", vf, a.out]
    subprocess.run(cmd, check=True)
    size = pathlib.Path(a.out).stat().st_size
    print(f"{len(frames)} frames, {len(runs)} distinct; {total_in:.1f} s captured -> {total_out:.1f} s; {size / 1e6:.2f} MB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
