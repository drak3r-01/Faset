import math, random
from maplib import *
from map_cove import facing

HEADER = """// Pine Hollow: a snowed-in alpine valley. Team map, 14 players, Rotate180
// (the west half is built, the east half is its half-turn copy). A frozen
// lake in the middle, a wooden cabin with a snow roof on each side (open
// front, rooftop reachable by a ramp), a ridge with a watch hut on the north
// side (and, by the half turn, one on the south side), pines and boulders as
// cover. The valley is closed by mountain walls of stone, ice and snow caps
// up to 24 m: no way out. The snow is a terrain (lot 6.10): rolling fields
// and banks rising at the foot of the mountains, flat around everything
// built on the ground.
//
//   z=-36 ^^^^^^^^^^^^^ mountains ^^^^^^^^^^^^^^^^
//         |  cabin W (A)  .   frozen lake   .  cabin E (B)  |
//   z=+36 vvvvvvvvvvvvv mountains vvvvvvvvvvvvvvvv
"""


def cabin(m, x0, z0):
    """A 12 x 10 m cabin, open to the east, snow roof, rooftop ramp on its north side."""
    x1, z1 = x0 + 12, z0 + 10
    m.box(x0, 0, z0, x0 + 0.5, 3.0, z1, "wood")                       # back wall (west)
    m.box(x0, 0, z0, x1, 3.0, z0 + 0.5, "wood")                       # north wall
    m.box(x0, 0, z1 - 0.5, x1, 3.0, z1, "wood")                       # south wall
    # windows: cut a band in the side walls by replacing the middle with posts
    m.box(x0 + 3, 1.2, z0 - 0.05, x0 + 5, 2.2, z0 + 0.55, "tiles_teal")
    m.box(x0 + 7, 1.2, z0 - 0.05, x0 + 9, 2.2, z0 + 0.55, "tiles_teal")
    m.box(x0 - 0.5, 3.0, z0 - 0.5, x1 + 0.5, 3.5, z1 + 0.5, "wood_dark")    # roof
    m.box(x0 - 0.5, 3.5, z0 - 0.5, x1 + 0.5, 4.0, z1 + 0.5, "snow")
    m.box(x0 + 2, 3.5, z0 + 3, x0 + 4, 6.0, z0 + 5, "brick_slate")           # chimney
    m.box(x0 + 2, 6.0, z0 + 3, x0 + 4, 6.5, z0 + 5, "snow")
    # inside: a table, benches, crates
    m.box(x0 + 3, 0, z0 + 3.5, x0 + 6, 1.0, z0 + 5.5, "wood_light")
    m.box(x0 + 3, 0, z0 + 2.4, x0 + 6, 0.6, z0 + 3.0, "wood")
    m.box(x0 + 3, 0, z0 + 6.0, x0 + 6, 0.6, z0 + 6.6, "wood")
    crate(m, x0 + 1.0, z0 + 8.0, 0.0)
    crate(m, x0 + 2.2, z0 + 8.4, 0.0)
    # roof ramp on the north outside: 4 m run, 4 m rise would be too steep; use a longer ramp
    m.wedge(x0, 0, z0 - 8, x0 + 4, 4.0, z0 - 0.5, "PosZ", "wood")


def build():
    m = Map("Pine Hollow", "A snowed-in alpine valley: a frozen lake, two cabins, a ridge and pines.",
            (2, 14), {"ambience": "snow", "sky": "overcast", "palette": "snow", "weather": [(0.0, "snow")]},
            HEADER, gravity=9.81)
    r = random.Random(5)

    # ---- ground: a frozen lake in the middle; the snow around it is the terrain (see the end) -------
    m.box(-64, -4, -48, 64, -2, 48, "dirt")
    m.zone(-68, -12, -52, 68, -2, 52, "KillVolume")
    m.box(-14, -2, -10, 14, 0, 10, "ice")                              # the lake
    # snow banks around the lake (low wedges)
    m.box(-14, 0, -10.5, 14, 0.5, -10, "snow")
    m.box(-14, 0, 10, 14, 0.5, 10.5, "snow")

    # ---- mountains: the border --------------------------------------------------------------------------
    for side in (-1, 1):
        x = -64.0
        while x < 64:
            w = min(5 + 5 * r.random(), 64 - x)
            if w < 1.0:
                break
            h = 12 + 8 * r.random()
            z0, z1 = (36.0, 48.0) if side > 0 else (-48.0, -36.0)
            m.box(x, 0, z0, x + w, h, z1, "stone" if r.random() < 0.7 else "basalt")
            m.box(x + 0.4, h, z0 + (3 if side > 0 else 0), x + w - 0.4, h + 2 + 3 * r.random(), z1 - (0 if side > 0 else 3), "snow")
            x += w
    m.box(-64, 0, -48, -52, 18, 48, "stone")
    m.box(52, 0, -48, 64, 18, 48, "stone")
    m.box(-64, 18, -48, -56, 24, 48, "snow")
    m.box(56, 18, -48, 64, 24, 48, "snow")
    m.box(-56, 0, -36, -52, 10, 36, "stone")
    m.box(52, 0, -36, 56, 10, 36, "stone")

    # ---- west half (copied): cabin, ridge, forest, rocks ---------------------------------------------------
    m.sym = True
    cabin(m, -44, -5)
    # the ridge (north side): a raised bench y=2 with stairs at both ends and a watch hut
    m.box(-26, 0, -34, 10, 2.0, -26, "snow")
    m.stairs(-30, 0, -33, -26, 2.0, -27, "PosX", 5, "stone")
    m.stairs(10, 0, -33, 14, 2.0, -27, "NegX", 5, "stone")
    # watch hut on the ridge
    m.box(-12, 2.0, -34, -4, 2.5, -28, "wood")
    m.box(-12, 2.5, -34, -11.5, 5.0, -28, "wood")
    m.box(-4.5, 2.5, -34, -4, 5.0, -28, "wood")
    m.box(-12, 2.5, -34, -4, 5.0, -33.5, "wood")
    m.box(-12.5, 5.0, -34.5, -3.5, 5.5, -27.5, "wood_dark")
    m.box(-12.5, 5.5, -34.5, -3.5, 6.0, -27.5, "snow")
    m.box(-9, 2.5, -29.5, -7, 3.5, -28.5, "wood_dark")
    # pines
    for i, (x, z) in enumerate([(-30, 14), (-24, 22), (-34, 28), (-18, 30), (-10, 18), (-8, -20), (-28, -18),
                                (-36, -22), (-22, -8), (-20, 12), (-4, 26), (-40, 16), (-48, -26), (-14, -30)]):
        pine(m, x, z, 0.0, 6.5 + (i % 3), "leaves", snow=True)
    # boulders with a snow cap
    for i, (x, z) in enumerate([(-30, -12), (-18, 20), (-26, 6), (-12, -14), (-6, 12)]):
        rock(m, x, z, 0.0, 2.4 + (i % 2) * 0.8, "stone", seed=70 + i)
        m.box(x - 0.8, 2.4 + (i % 2) * 0.8, z - 0.8, x + 0.8, 2.9 + (i % 2) * 0.8, z + 0.8, "snow")
    # wood stack, sled, low fences as cover
    m.box(-36, 0, 12, -30, 1.2, 13, "wood_dark")
    m.box(-36, 0, 13, -30, 0.8, 14, "wood")
    m.box(-22, 0, -20, -16, 1.0, -19.5, "wood")
    m.box(-22, 0, -20, -21.5, 1.0, -16, "wood")
    m.box(-16, 0, 4, -15.5, 1.0, 9, "wood")
    crate(m, -20, 26, 0.0)
    crate(m, -20, 27.2, 0.0)
    crate(m, -19, 26.6, 1.0, 0.9)
    # snowmen
    for (x, z) in ((-26, 17), (-6, -24)):
        m.box(x - 0.8, 0, z - 0.8, x + 0.8, 1.2, z + 0.8, "snow")
        m.box(x - 0.6, 1.2, z - 0.6, x + 0.6, 2.2, z + 0.6, "snow")
        m.box(x - 0.4, 2.2, z - 0.4, x + 0.4, 2.8, z + 0.4, "snow")
        m.box(x - 0.1, 2.4, z - 0.6, x + 0.1, 2.5, z - 0.4, "metal_orange")
    m.sym = False
    # lake: two ice boulders and a frozen log
    m.box(-4, 0, -2, -1, 1.0, 1, "ice")
    m.box(1, 0, 4, 4, 1.4, 6.5, "ice")
    m.box(2, 0, -5, 6, 0.6, -4, "wood_dark")

    # ---- gameplay ------------------------------------------------------------------------------------------
    spots = m.find_spots(lambda x, z: -36 < x < -26 and -16 < z < 16, 7, gap=3.0, near=(-31, 0))
    assert len(spots) == 7, spots
    m.sym = True
    for x, z in spots:
        m.spawn(x, 0.0, z, facing(x, z, 0, 0), "A")
    m.pickup("Health", -15, 0.5, -14, 25)
    m.pickup("Health", -33, 0.5, 20, 30)
    m.pickup("Armor", -8.0, 3.0, -31.0, 45)
    m.pickup("Ammo", -20, 0.5, 1, 20)
    m.pickup("Ammo", -40, 0.5, -12, 20)
    m.sym = False
    snow_fields(m)

    # ---- vegetation (lot 6.11): pines, rocks, shrubs and pale grass on the snow fields ----
    # (`terrain:` keeps the snow roofs, banks and snowmen bare)
    m.set_vegetation(37, [
        ("terrain:snow", "Pine", 0.6), ("terrain:snow", "Rock", 0.5),
        ("terrain:snow", "Bush", 0.6), ("terrain:snow", "Grass", 8.0),
    ])
    # ---- landscape beyond the bounds (lot 6.12)
    m.set_surroundings((-64, -48, 64, 48), "Slope", "Cliff", "Slope", "Forest", "Mountains", 0.3, 7)
    return m


def smoothstep(a, b, x):
    t = min(max((x - a) / (b - a), 0.0), 1.0)
    return t * t * (3 - 2 * t)


def snow_fields(m):
    """Terrain (lot 6.10): rolling snow fields over the valley floor, rising
    into banks at the foot of the mountains. Everything standing on the
    ground (cabins, ridge, pines, rocks, spawns, pickups) keeps a flat patch
    at y = 0 that blends into the hills over 1 to 5 m; under the lake the
    terrain hides below the ice. Symmetric under the half turn, like the map.
    """
    import re
    rects = []
    for (a, b, c, d, e, f_) in m.solids:
        cx, cz = (a + d) / 2, (c + f_) / 2
        if abs(b) <= 0.05 and abs(cx) < 52 and abs(cz) < 36:
            rects.append((a, c, d, f_))
    for s in m.spawns + m.pickups:
        x, _, z = (float(v) for v in re.search(r"at: \(([-\d.]+), ([-\d.]+), ([-\d.]+)\)", s).groups())
        rects.append((x - 0.5, z - 0.5, x + 0.5, z + 0.5))
    rects.append((-14, -10, 14, 10))  # the lake

    def clearance(x, z):
        best = 1e9
        for (a, c, d, f_) in rects:
            dx = max(a - x, 0.0, x - d)
            dz = max(c - z, 0.0, z - f_)
            best = min(best, math.hypot(dx, dz))
        return best

    def raw(x, z):
        hills = 0.8 * math.cos(0.19 * x + 0.11 * z) + 0.6 * math.cos(0.07 * x - 0.23 * z) \
            + 0.4 * math.cos(0.29 * x + 0.25 * z)
        bank = 0.45 * max(0.0, abs(z) - 27) + 0.45 * max(0.0, abs(x) - 45)
        return 0.6 + 0.7 * hills + bank

    def height(x, z):
        if -14 < x < 14 and -10 < z < 10:
            return -0.5  # under the ice
        mask = smoothstep(1.0, 5.0, min(clearance(x, z), clearance(-x, -z)))
        return mask * raw(x, z)

    cms = m.set_terrain(-52, -36, 2.0, 53, 37, height)
    steep = 0
    for j in range(36):
        for i in range(52):
            h = lambda a, b: cms[b * 53 + a] / 100
            grade = max(abs(h(i + 1, j) - h(i, j)), abs(h(i, j + 1) - h(i, j)), abs(h(i + 1, j + 1) - h(i, j)) / 1.414) / 2
            steep += grade > 1.0
    print(f"pine-hollow terrain: {min(cms) / 100} to {max(cms) / 100} m, {steep} cells steeper than 45 degrees")


if __name__ == "__main__":
    import sys
    build().save(sys.argv[1])
