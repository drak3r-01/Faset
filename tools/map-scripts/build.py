#!/usr/bin/env python3
"""Regenerates the four shipped maps of `assets/maps` (lot 6.8).

Run in the dev container from the repository root:

    docker compose run --rm dev python3 tools/map-scripts/build.py

The maps are plain RON files afterwards (the editor and `map-tool` open
them like any other); this script is only how they were authored. Every
script builds one half of its map and lets `Map.sym` emit the half-turn copy.
Placement helpers (`Map.find_spots`) pick spawn points that clear every solid.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import map_cove, map_harbor, map_hollow, map_mesa  # noqa: E402

OUT = os.path.join(HERE, "..", "..", "assets", "maps")

for name, module in (
    ("driftwood-cove", map_cove),
    ("red-mesa", map_mesa),
    ("pine-hollow", map_hollow),
    ("rust-harbor", map_harbor),
):
    path = os.path.join(OUT, name + ".ron")
    module.build().save(path)
    print("wrote", path)
