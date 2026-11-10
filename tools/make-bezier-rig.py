"""Generates assets/rigs/ella_bezier/ from assets/rigs/ella/: identical except that every segment of every bone
timeline (rotate / translate / scale) gets an ease-in-out bezier curve instead of linear interpolation.

Ella's animations have no bezier curves at all, so this rig measures what evaluating beziers costs (scene
`ella_random_bezier` vs `ella_random`). The work per frame is otherwise identical: same bones, keys, attachments.

Spine 4.x JSON: a key's `curve` describes the segment to the next key, as 4 absolute numbers per value component
(cx1, cy1, cx2, cy2: x in seconds, y in the timeline's units), x component first. Omitted fields take their
defaults (time 0, rotate/translate value 0, scale 1).

Run from anywhere: python tools/make-bezier-rig.py, then tools/make-spine43-assets.sh.
"""

import json
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "assets/rigs/ella"
DST = ROOT / "assets/rigs/ella_bezier"
NAME = "Ella_bezier"

# Component fields and their defaults per bone timeline type
COMPONENTS = {
    "rotate": [("value", 0.0)],
    "translate": [("x", 0.0), ("y", 0.0)],
    "scale": [("x", 1.0), ("y", 1.0)],
}


def ease(t0, v0, t1, v1):
    """Ease-in-out: horizontal tangents at both ends, control points at 1/4 and 3/4 of the duration."""
    dt = t1 - t0
    return [t0 + dt / 4, v0, t1 - dt / 4, v1]


def bezierize(keys, components):
    for key, nxt in zip(keys, keys[1:]):
        if key.get("curve") == "stepped":
            continue
        t0, t1 = key.get("time", 0.0), nxt.get("time", 0.0)
        curve = []
        for field, default in components:
            curve += ease(t0, key.get(field, default), t1, nxt.get(field, default))
        key["curve"] = curve
    return len(keys) - 1 if keys else 0


def main():
    data = json.loads((SRC / "Ella.json").read_text(encoding="utf-8"))
    segments = 0
    for animation in data["animations"].values():
        for timelines in animation.get("bones", {}).values():
            for kind, keys in timelines.items():
                if kind not in COMPONENTS:
                    raise SystemExit(f"unexpected bone timeline type {kind!r}")
                segments += bezierize(keys, COMPONENTS[kind])

    if DST.exists():
        shutil.rmtree(DST)
    DST.mkdir(parents=True)
    with open(DST / f"{NAME}.json", "w", encoding="utf-8", newline="\n") as f:
        json.dump(data, f, ensure_ascii=False, separators=(",", ":"))
    atlas = (SRC / "Ella.atlas").read_text(encoding="utf-8")
    assert atlas.startswith("Ella.png"), "atlas page name changed"
    with open(DST / f"{NAME}.atlas", "w", encoding="utf-8", newline="") as f:
        f.write(f"{NAME}.png" + atlas[len("Ella.png"):])
    shutil.copyfile(SRC / "Ella.png", DST / f"{NAME}.png")
    print(f"{DST}: {segments} bezier segments")


if __name__ == "__main__":
    main()
