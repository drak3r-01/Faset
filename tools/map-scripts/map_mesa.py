import math, random
from maplib import *
from map_cove import facing

HEADER = """// Red Mesa: a dry canyon at sunset. Team map, 14 players, Rotate180 (the
// west half is built, the east half is its half-turn copy). A dry river bed
// runs north-south through the middle with ramps down and two wooden bridges;
// each team starts on a mesa (y=3) reached by stairs, the middle is a field of
// mushroom rocks, a wagon wreck and a water tower. The canyon walls, stepped
// sandstone terraces up to 16 m, are the border: no way out.
//
//   z=-32 ############ canyon wall (terraced sandstone) ############
//         | mesa W |   rocks   ||| river bed |||   rocks   | mesa E |
//         |  (A)   |  wagon    bridge  bridge   water tower |  (B)   |
//   z=+32 ############ canyon wall (terraced sandstone) ############
//
// Verticality: mesas (3 m), mushroom-rock caps (3.5 m), the water tower deck
// (5 m) and the lookout ledge on the north wall (4 m, with a mine tunnel).
"""


def build():
    m = Map("Red Mesa", "A dry canyon at sunset: two mesas, a river bed with bridges and a field of mushroom rocks.",
            (2, 14), {"ambience": "sunset", "sky": "dusk", "palette": "desert", "weather": [(0.0, "clear")]},
            HEADER, gravity=9.81)
    r = random.Random(11)

    # ---- floor and canyon walls ------------------------------------------------------------
    m.zone(-66, -12, -48, 66, -4, 48, "KillVolume")           # below the map
    # terraced walls: stepped rows with a ragged skyline (north and south),
    # solid ends (east and west)
    for side in (-1, 1):
        x = -62.0
        while x < 62:
            w = 4 + 4 * r.random()
            h1 = 7 + 4 * r.random()
            h2 = h1 + 3 + 3 * r.random()
            z_in, z_out = (33.0, 44.0) if side > 0 else (-44.0, -33.0)
            m.box(x, 0, z_in if side > 0 else z_out - 11, x + w, h1, z_out if side > 0 else z_in + 11, "sandstone" if r.random() < 0.6 else "clay")
            # upper terrace set back from the canyon side
            if side > 0:
                m.box(x + 0.5, h1, 36, x + w - 0.5, h2, 44, "clay")
                m.box(x + 1.0, h2, 40, x + w - 1.0, h2 + 2.5 + 2 * r.random(), 44, "sandstone")
            else:
                m.box(x + 0.5, h1, -44, x + w - 0.5, h2, -36, "clay")
                m.box(x + 1.0, h2, -44, x + w - 1.0, h2 + 2.5 + 2 * r.random(), -40, "sandstone")
            x += w
    m.box(-62, 0, -44, -54, 14, 44, "sandstone")
    m.box(54, 0, -44, 62, 14, 44, "sandstone")
    m.box(-62, 14, -44, -57, 18, 44, "clay")
    m.box(57, 14, -44, 62, 18, 44, "clay")

    # ---- river bed (x in [-6, 6], 1.5 m below the banks) ------------------------------------------------
    m.box(-62, -4, -44, 62, -2, 44, "dirt")                  # the lowest floor
    m.box(-62, -2, -33, -6, 0, 33, "sand")                   # west bank (y = 0)
    m.box(6, -2, -33, 62, 0, 33, "sand")                     # east bank
    m.box(-6, -2, -33, 6, -1.5, 33, "clay")                  # bed gravel
    m.sym = True
    # ramps from the bed up to the bank (rise on the bank side)
    m.wedge(-6, -1.5, -20, 0, 0, -14, "NegX", "sand")
    m.sym = False
    # two wooden bridges over the bed
    for z in (-6, 6):
        m.box(-8, -0.5, z - 1.5, 8, 0.0, z + 1.5, "wood")
        m.box(-8, 0.0, z - 1.5, 8, 0.5, z - 1.2, "wood_dark")
        m.box(-8, 0.0, z + 1.2, 8, 0.5, z + 1.5, "wood_dark")
        for x in (-5.5, 5.5):
            m.box(x - 0.25, -2, z - 1.5, x + 0.25, -0.5, z - 1.0, "wood_dark")
            m.box(x - 0.25, -2, z + 1.0, x + 0.25, -0.5, z + 1.5, "wood_dark")

    # ---- mesas (west half; the copy is the east mesa) --------------------------------------------------
    m.sym = True
    m.box(-52, 0, -14, -30, 3.0, 14, "sandstone")            # mesa body
    m.box(-52, 3.0, -14, -30, 3.5, -13.3, "clay")            # parapets
    m.box(-52, 3.0, 13.3, -30, 3.5, 14, "clay")
    m.box(-52, 3.0, -14, -51.3, 3.5, 14, "clay")
    m.stairs(-30, 0, -3, -24, 3.0, 3, "NegX", 8, "sandstone")    # main stairs
    m.stairs(-44, 0, 14, -40, 3.0, 19, "NegZ", 8, "sandstone")   # side stairs (south)
    m.stairs(-44, 0, -19, -40, 3.0, -14, "PosZ", 8, "sandstone")  # side stairs (north)
    # arch over the main stairs (a natural rock gate)
    m.box(-32, 3.0, -4, -30, 7, -2.5, "clay")
    m.box(-32, 3.0, 2.5, -30, 7, 4, "clay")
    m.box(-33, 7, -4.5, -29.5, 8.5, 4.5, "sandstone")
    # cover on the mesa
    for (x, z, w, d) in [(-40, -8, 4, 0.8), (-46, 6, 0.8, 4), (-36, 9, 3, 0.8), (-44, -2, 1, 1)]:
        m.box(x, 3.5, z, x + w, 4.7, z + d, "sandstone")
    crate(m, -48, -9, 3.5)
    crate(m, -47, -9, 3.5)
    crate(m, -47.5, -9, 4.5)
    crate(m, -38, 11, 3.5, 1.5)

    # ---- the middle field -------------------------------------------------------------------------------
    # mushroom rocks: a stalk and a wide cap that gives cover underneath
    for i, (x, z) in enumerate([(-20, -10), (-14, 12), (-22, 18), (-12, -22), (-24, 4)]):
        m.box(x - 1.2, 0, z - 1.2, x + 1.2, 3.0, z + 1.2, "sandstone" if i % 2 else "clay")
        m.box(x - 2.8, 3.0, z - 2.8, x + 2.8, 4.0, z + 2.8, "clay" if i % 2 else "sandstone")
        m.box(x - 1.6, 4.0, z - 1.6, x + 1.6, 4.6, z + 1.6, "sandstone")
    # boulders and low cover
    for i, (x, z) in enumerate([(-28, -20), (-18, 27), (-8, -14), (-10, 8), (-26, 12), (-16, -4), (-32, 22), (-34, -24)]):
        rock(m, x, z, 0.0, 2.2 + (i % 3) * 0.6, "sandstone" if i % 2 else "clay", seed=40 + i)
    # wagon wreck
    m.box(-18, 0.5, -29, -14, 1.5, -27.4, "wood_dark")
    m.box(-18, 0.5, -29, -17.6, 2.3, -27.4, "wood")
    m.box(-14.4, 0.5, -29, -14, 2.3, -27.4, "wood")
    m.box(-17.4, 0, -29.2, -16.8, 1.2, -29.0, "wood_dark")
    m.box(-15.2, 0, -27.4, -14.6, 1.2, -27.2, "wood_dark")
    # water tower: legs, deck at 5 m, tank, ladder as stairs
    for dx in (-2.0, 2.0):
        for dz in (-2.0, 2.0):
            m.box(-9 + dx - 0.3, 0, 25 + dz - 0.3, -9 + dx + 0.3, 5, 25 + dz + 0.3, "wood_dark")
    m.box(-12, 4.5, 22, -6, 5.0, 28, "wood")
    m.box(-11, 5.0, 23, -7, 8.5, 27, "wood_white")
    m.box(-11.3, 6.0, 23, -10.9, 6.5, 27, "metal_orange")
    m.box(-11.3, 7.5, 23, -10.9, 8.0, 27, "metal_orange")
    m.box(-7.2, 6.0, 23, -6.8, 6.5, 27, "metal_orange")
    m.stairs(-12, 0, 24, -6.0 - 6, 5.0, 26, "PosX", 12, "wood") if False else None
    m.stairs(-18, 0, 23.5, -12, 5.0, 26.5, "PosX", 12, "wood")
    # cacti
    for i, (x, z) in enumerate([(-26, 25), (-33, -10), (-16, 3), (-44, 24), (-48, -24), (-22, -17), (-10, 18)]):
        m.box(x - 0.3, 0, z - 0.3, x + 0.3, 2.5 + (i % 3) * 0.5, z + 0.3, "grass")
        m.box(x + 0.3, 1.2, z - 0.2, x + 1.0, 1.7, z + 0.2, "grass")
        m.box(x + 0.8, 1.2, z - 0.2, x + 1.2, 2.4, z + 0.2, "grass")
        m.box(x - 1.0, 0.9, z - 0.2, x - 0.3, 1.4, z + 0.2, "grass")
        m.box(x - 1.2, 0.9, z - 0.2, x - 0.8, 1.9, z + 0.2, "grass")
    # north-wall lookout ledge with a mine tunnel behind it
    m.box(-34, 0, -33, -22, 4.0, -29, "sandstone")
    m.stairs(-34, 0, -29, -22, 4.0, -26, "NegZ", 10, "wood")
    m.box(-34, 4.0, -33, -22, 4.7, -32.4, "clay")
    m.box(-34, 4.0, -33, -33.4, 4.7, -29, "clay")
    m.sym = False

    # ---- gameplay -------------------------------------------------------------------------------------------
    spots = m.find_spots(lambda x, z: -50 < x < -33 and -12 < z < 12, 7, y=3.0, gap=3.5, near=(-44, 0))
    assert len(spots) == 7, spots
    m.sym = True
    for x, z in spots:
        m.spawn(x, 3.0, z, facing(x, z, 0, 0), "A")
    m.pickup("Health", -20, 0.5, -1, 25)
    m.pickup("Health", -42, 3.5, 11.5, 30)
    m.pickup("Armor", -8.5, 5.5, 22.8, 45)
    m.pickup("Ammo", -37, 3.5, -9, 20)
    m.pickup("Ammo", -16, 0.5, 20, 20)
    m.sym = False
    return m


if __name__ == "__main__":
    import sys
    build().save(sys.argv[1])
