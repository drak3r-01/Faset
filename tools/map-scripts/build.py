#!/usr/bin/env python3
"""Regenerates the shipped maps of `assets/maps` (lots 6.8 and 6.13).

Run in the dev container from the repository root:

    docker compose run --rm dev python3 tools/map-scripts/build.py [name...]

The maps are plain RON files afterwards (the editor and `map-tool` open
them like any other); this script is only how they were authored. Every
script builds one half of its map and lets the `Map.sym` mode emit the
mirrored or half-turn copy. Placement helpers (`Map.find_spots`) pick spawn
points that clear every solid. With names, only those maps are written.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import map_cove, map_harbor, map_hollow, map_mesa  # noqa: E402
import map_mossgate, map_greyfold, map_nightjar  # noqa: E402

OUT = os.path.join(HERE, "..", "..", "assets", "maps")

MAPS = (
    ("driftwood-cove", map_cove),
    ("red-mesa", map_mesa),
    ("pine-hollow", map_hollow),
    ("rust-harbor", map_harbor),
    ("mossgate", map_mossgate),
    ("greyfold-quarry", map_greyfold),
    ("nightjar-market", map_nightjar),
)

if __name__ == "__main__":
    wanted = set(sys.argv[1:])
    for name, module in MAPS:
        if wanted and name not in wanted:
            continue
        path = os.path.join(OUT, name + ".ron")
        module.build().save(path)
        print("wrote", path)
