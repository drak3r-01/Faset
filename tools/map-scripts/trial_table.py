#!/usr/bin/env python3
"""Bot trials of the shipped maps, summarised as a Markdown table (lot 6.13).

Run in the dev container from the repository root, after building map-tool:

    docker compose run --rm dev python3 tools/map-scripts/trial_table.py \
        /cache/target/release/map-tool [name...]

Each map is played with seeds 1, 2 and 3 (`map-tool trial`, team mode, 16
bots, 5 minutes). Columns: kills, kills of team A and B, the gap between
them (relative to the larger), deaths within 6 m of a spawn, the mean
engagement distance, and the dead zones: 4 m cells with a walkable floor
that no bot entered, counted only inside the playable area of maps that
declare `play_area(x, z)` (roofs, cliffs and scenery outside it do not
count), as a share of the visited plus unvisited cells of that area.
"""
import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from build import MAPS, OUT  # noqa: E402

SEEDS = (1, 2, 3)


def trial(tool, path, seed):
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "report.json")
        subprocess.run([tool, "trial", path, "--seed", str(seed), "--out", out], check=True,
                       stdout=subprocess.DEVNULL)
        with open(out) as fh:
            return json.load(fh)


def dead_zones(report, inside):
    n = 0
    for z in report["unvisited_zones"]:
        cx = (z["min"][0] + z["max"][0]) / 2
        cz = (z["min"][1] + z["max"][1]) / 2
        if inside is None or inside(cx, cz):
            n += 1
    return n


def area_cells(inside, cell=4.0):
    n = 0
    for i in range(-40, 40):
        for j in range(-40, 40):
            if inside((i + 0.5) * cell, (j + 0.5) * cell):
                n += 1
    return n


def main():
    tool = sys.argv[1]
    wanted = set(sys.argv[2:])
    print("| Map | Seed | Kills | A / B | Gap | Spawn deaths | Engagement | Dead zones |")
    print("|---|---|---|---|---|---|---|---|")
    for name, module in MAPS:
        if wanted and name not in wanted:
            continue
        inside = getattr(module, "play_area", None)
        cells = area_cells(inside) if inside else None
        path = os.path.join(OUT, name + ".ron")
        for seed in SEEDS:
            r = trial(tool, path, seed)
            a, b = r["team_balance"][0]["kills"], r["team_balance"][1]["kills"]
            gap = abs(a - b) / max(a, b, 1)
            dz = dead_zones(r, inside)
            dz_txt = f"{dz} ({100 * dz / cells:.0f} %)" if cells else f"{dz} (all cells)"
            print(f"| {name} | {seed} | {r['total_kills']} | {a} / {b} | {100 * gap:.0f} % | "
                  f"{100 * r['spawn_deaths']['rate']:.1f} % | {r['mean_engagement_distance_m']:.1f} m | {dz_txt} |",
                  flush=True)


if __name__ == "__main__":
    main()
