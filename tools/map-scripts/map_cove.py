import math, random
from maplib import *


def facing(x, z, tx=0.0, tz=0.0):
    dx, dz = tx - x, tz - z
    return math.degrees(math.atan2(-dx, -dz)) % 360


HEADER = """// Driftwood Cove: a tropical island in a turquoise lagoon. Team map, 12
// players, Rotate180 (everything is built twice, the second copy turned a
// half turn about the centre). The sea is deadly: the island is bordered by
// the water (a KillVolume below the sea level), dark sea stacks and cliffs.
//
//   z=-30   cliff wall (basalt)             sea stacks around the cove
//           .--- wreck lobe ----.
//   pier W--|   dirt rim, grass plateau (y=1)    |--pier E
//           |   shrine on a 3-tier ziggurat (y=5)|
//           '---- lagoon (deadly) -----------------'
//
// Routes: the beach ring (flat, long sightlines along the sand), the plateau
// (ruined walls and crates for cover), the ziggurat (height, sniper nest) and
// two watchtowers (4 m platforms) overlooking the beaches.
"""


def build():
    m = Map("Driftwood Cove", "A tropical island in a turquoise lagoon: beaches, ruins and a shrine on the hill.",
            (2, 12), {"ambience": "day", "sky": "clear", "palette": "nature",
                      "weather": [(0.0, "clear")]}, HEADER, gravity=9.81)

    # ---- sea and seabed ------------------------------------------------------
    m.box(-60, -6, -60, 60, -3, 60, "sand")
    m.disc(0, 0, 38, -3, -1.5, "sand_white", n=24)          # shallow shelf, seen through the water
    m.zone(-60, -9, -60, 60, -0.5, 60, "KillVolume")         # the sea is deadly
    m.zone(-60, -8, -60, 60, -0.5, 60, "Water")              # sea level (drawn by the renderer)
    # Cliffs enclosing the cove: a ragged ring of basalt and stone, 8 to 16 m high.
    rr = random.Random(3)
    for side in (-1, 1):
        x = -58.0
        while x < 58:
            w = min(4 + 5 * rr.random(), 58 - x)
            if w < 1:
                break
            h = 8 + 6 * rr.random()
            z0, z1 = (50.0, 58.0) if side > 0 else (-58.0, -50.0)
            m.box(x, -3, z0, x + w, h, z1, "basalt" if rr.random() < 0.7 else "stone")
            m.box(x + 0.6, h, z0 + (2 if side > 0 else 0), x + w - 0.6, h + 2 + 3 * rr.random(), z1 - (0 if side > 0 else 2), "basalt")
            x += w
        z = -50.0
        while z < 50:
            w = min(4 + 5 * rr.random(), 50 - z)
            if w < 1:
                break
            h = 6 + 6 * rr.random()
            x0, x1 = (50.0, 58.0) if side > 0 else (-58.0, -50.0)
            m.box(x0, -3, z, x1, h, z + w, "basalt" if rr.random() < 0.7 else "stone")
            m.box(x0 + (2 if side > 0 else 0), h, z + 0.6, x1 - (0 if side > 0 else 2), h + 2 + 2 * rr.random(), z + w - 0.6, "basalt")
            z += w
    # Sea stacks (decorative, in the deadly water).
    r = random.Random(7)
    for i in range(14):
        a = 2 * math.pi * i / 14 + 0.2
        d = 41 + 4 * r.random()
        x, z = d * math.cos(a), d * math.sin(a) * 0.9
        h = 5 + 7 * r.random()
        w = 2.0 + 2 * r.random()
        m.box(x - w, -3, z - w, x + w, h, z + w, "basalt")
        m.box(x - w * 0.6, h, z - w * 0.6, x + w * 0.6, h + 2, z + w * 0.6, "basalt")

    # ---- island body (top at y=0) -------------------------------------------------
    m.disc(0, 0, 30, -3, 0, "sand_white", n=24)
    m.sym = True
    m.disc(26, 14, 13, -3, 0, "sand_white", n=16)
    m.disc(10, -28, 11, -3, 0, "sand_white", n=16)
    # a few tufts of dry grass on the beach (flat, same level as the sand)
    m.sym = False

    # plateau: dirt rim and grass top
    m.disc(0, 0, 19.5, 0, 0.5, "dirt", n=24)
    m.disc(0, 0, 19, 0.5, 1.0, "grass", n=24)
    m.sym = True
    # ramps up to the plateau on the four sides (the sym copy gives -X and +Z)
    m.wedge(17.5, 0, -2.5, 22, 1.0, 2.5, "NegX", "dirt")
    m.wedge(-2.5, 0, -22, 2.5, 1.0, -17.5, "PosZ", "dirt")
    m.sym = False

    # ---- ziggurat and shrine (centre, same on both sides) --------------------------
    m.box(-9, 1, -9, 9, 2.5, 9, "sandstone")
    m.box(-6, 2.5, -6, 6, 4.0, 6, "sandstone")
    m.box(-3.5, 4.0, -3.5, 3.5, 5.0, 3.5, "stone")
    m.sym = True
    m.stairs(9, 1.0, -2.5, 12.5, 2.5, 2.5, "NegX", 4, "stone")
    m.stairs(-2, 2.5, 6, 2, 4.0, 9, "NegZ", 4, "stone")
    m.stairs(3.5, 4.0, -1.5, 6, 5.0, 1.5, "NegX", 3, "stone")
    m.sym = False
    for sx in (-3, 3):
        for sz in (-3, 3):
            m.box(sx - 0.5, 5.0, sz - 0.5, sx + 0.5, 8.0, sz + 0.5, "sandstone")
    m.box(-4.5, 8.0, -4.5, 4.5, 8.5, 4.5, "wood")
    m.box(-3.5, 8.5, -3.5, 3.5, 9.0, 3.5, "palm_leaves")
    m.box(-1.0, 5.0, -3.5, 1.0, 6.0, -3.0, "sandstone")        # low parapets of the nest
    m.box(-1.0, 5.0, 3.0, 1.0, 6.0, 3.5, "sandstone")
    # temple pillars on tier 1 and 2 corners
    for sx in (-7.5, 7.5):
        for sz in (-7.5, 7.5):
            m.box(sx - 0.5, 2.5, sz - 0.5, sx + 0.5, 4.5, sz + 0.5, "mossy_stone")

    # ---- plateau cover (ruined walls, crates), one half, copied ------------------------
    m.sym = True
    m.box(12.5, 1, 6.5, 17, 2.0, 7.3, "mossy_stone")
    m.box(16.2, 1, 3.0, 17, 2.5, 7.3, "mossy_stone")
    m.box(7, 1, 12.5, 7.8, 2.5, 17, "mossy_stone")
    m.box(7, 1, 12.5, 10.5, 1.8, 13.2, "mossy_stone")
    m.box(-7, 1, 13, -3.5, 2.0, 13.8, "mossy_stone")
    m.box(13, 1, -10, 13.8, 2.5, -5, "mossy_stone")
    m.box(9.5, 1, -12.5, 14, 2.0, -11.7, "mossy_stone")
    crate(m, -6.5, 15.5, 1.0)
    crate(m, -5.4, 15.5, 1.0)
    crate(m, -6.0, 15.5, 2.0)
    crate(m, 5.5, -14.5, 1.0)
    crate(m, 5.5, -13.4, 1.0, mat="crate")
    crate(m, 15, 12, 1.0, 1.5)
    # Palm trees on the plateau edge
    for (x, z) in ((16.5, 9.5), (-12, 14.5), (8, -16), (17.5, -7)):
        palm(m, x, z, 1.0, 4.5)
    m.sym = False

    # ---- beach ring ---------------------------------------------------------------------
    m.sym = True
    palms = [(22, 1.5), (24.5, -7), (26, 5), (-18, 22), (-24, 15), (30, 18), (33, 11),
             (12, 24), (20, -18), (3, -25), (14, -33), (6, -31), (-4, 27), (-20, 25), (-26, 6)]
    for i, (x, z) in enumerate(palms):
        palm(m, x, z, 0.0, 4.5 + (i % 3) * 0.5)
    # Rocks and boulders
    for i, (x, z) in enumerate([(24, 22), (28, 6), (-23, 22), (4, 29), (22, -12), (-27, 3), (8, -22)]):
        rock(m, x, z, 0.0, 2.0 + (i % 2), "basalt" if i % 2 else "stone", seed=i)
    # Beach crates and wooden barrels
    crate(m, 19.5, 23, 0.0)
    crate(m, 20.7, 23, 0.0)
    crate(m, 20.1, 23, 1.0)
    crate(m, -16, -24.5, 0.0, 1.5)
    # Rock walls sheltering the spawn beaches (open towards the island)
    m.box(-25, 0, 23, -9, 2.0, 23.8, "basalt")
    m.box(-25.8, 0, 9, -25, 2.0, 23.8, "basalt")
    m.box(-12, 0, 8.0, -6, 1.4, 8.8, "stone")
    m.box(-9.8, 0, 9.5, -9, 1.6, 15, "stone")
    m.box(-17, 0, 14.5, -15.5, 1.2, 16, "stone")
    # Low driftwood walls on the sand
    m.box(24, 0, -4.5, 28, 1.0, -3.8, "wood_dark")
    m.box(-8, 0, 21, -4, 1.0, 21.7, "wood_dark")
    m.box(13, 0, -23, 17, 1.0, -22.3, "wood_dark")
    # Piers (east end of the lobe; the copy is the west pier) with a fishing hut
    m.box(37, -0.5, 12.5, 49, 0.0, 15.5, "wood")
    m.box(46, -0.5, 10.5, 51, 0.0, 17.5, "wood")
    for x in (39, 43, 47, 50):
        for z in (12.5, 15.0):
            m.box(x, -3, z, x + 0.5, -0.5, z + 0.5, "wood_dark")
    m.box(47, 0, 10.5, 51, 3.0, 11, "wood_dark")
    m.box(47, 0, 17, 51, 3.0, 17.5, "wood_dark")
    m.box(46.5, 3.0, 10, 51.5, 3.5, 18, "wood")
    m.box(47, 0, 10.5, 47.5, 1.0, 17.5, "wood_dark")
    # Wreck on the second lobe (sunk into the sand), mast with a torn sail
    m.box(5, 0, -34, 5.5, 2.0, -25, "wood_dark")
    m.box(14.5, 0, -34, 15, 2.0, -25, "wood_dark")
    m.box(5, 0, -25.5, 15, 2.5, -25, "wood_dark")
    m.wedge(5, 0, -37, 15, 3.0, -34, "PosZ", "wood_dark")
    m.box(9.5, 0, -30.5, 10.5, 9, -29.5, "wood_dark")
    m.box(7, 6, -30.3, 13, 8, -29.7, "wood_white")
    m.box(5, 0, -30, 5.5, 3.0, -28.5, "wood")
    m.box(14.5, 0, -32, 15, 3.0, -30.5, "wood")
    # Watchtower on the beach: platform at y=4, access ramp of stairs
    for tx in (-24.5, -19.5):
        for tz in (-11.5, -6.5):
            m.box(tx, 0, tz, tx + 0.5, 4.0, tz + 0.5, "wood_dark")
    m.box(-25.5, 4.0, -12.5, -18.5, 4.5, -5.5, "wood")
    m.box(-25.5, 4.5, -12.5, -25, 5.5, -5.5, "wood")
    m.box(-25.5, 4.5, -12.5, -18.5, 5.5, -12, "wood")
    m.box(-18.5, 4.5, -12.5, -18, 5.5, -9.5, "wood")
    m.box(-25.5, 7.0, -12.5, -18.5, 7.5, -5.5, "wood_dark")
    for tx in (-25.2, -18.8):
        for tz in (-12.2, -5.8):
            m.box(tx - 0.25, 4.5, tz - 0.25, tx + 0.25, 7.0, tz + 0.25, "wood_dark")
    m.stairs(-18.5, 0, -9.5, -13, 4.0, -7.5, "NegX", 10, "wood")
    m.sym = False

    # ---- gameplay --------------------------------------------------------------------------
    # Team A on the north-west beach, team B (the half-turn copy) on the south-east.
    # A spawns are picked from the free sand of the north-west beach (checked
    # against the solids of both halves, so the half-turn copies are free too).
    a_spots = m.find_spots(lambda x, z: x < -8 and z > 4 and x * x + z * z < 27 ** 2 and x * x + z * z > 20.5 ** 2
                           and m.free(-x, -z, 0.0, 0.9), 6, gap=4.0, near=(-17, 18))
    assert len(a_spots) == 6, a_spots
    m.sym = True
    for x, z in a_spots:
        m.spawn(x, 0.0, z, facing(x, z), "A")
    # extra spawns on the lobe: shared by both teams through the half turn
    for x, z in [(30, 21), (26, 24)]:
        pass
    m.pickup("Health", 12, 1.5, 9.5, 25)
    m.pickup("Health", 22, 0.5, 10, 25)
    m.pickup("Armor", 0.0, 5.5, -2.0, 45)
    m.pickup("Ammo", 26, 0.5, 2, 20)
    m.pickup("Ammo", 6, 1.5, -9, 20)
    m.sym = False

    # ---- vegetation (lot 6.11): a lush plateau, sparse tufts and palms on the beaches ----
    m.set_vegetation(61, [
        ("grass", "Grass", 45.0), ("grass", "Fern", 10.0), ("grass", "Bush", 1.5),
        ("sand_white", "Grass", 3.0), ("sand_white", "Palm", 0.3), ("sand_white", "Rock", 0.4),
    ])
    # ---- landscape beyond the bounds (lot 6.12): a lagoon all around
    m.set_surroundings((-60, -60, 60, 60), "Sea", "Sea", "Sea", "Sea", "Treeline", 0.8, 3, ground=-1.0, sea_level=-1.0)
    return m


if __name__ == "__main__":
    import sys
    build().save(sys.argv[1])
