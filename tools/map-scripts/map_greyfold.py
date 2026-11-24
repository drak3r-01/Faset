import math
import random
from maplib import *
from map_cove import facing

HEADER = """// Greyfold Quarry: a marble quarry under a grey sky. Team map, 16 players,
// a loop: the haul road runs all around a flooded pit (deadly), with a rock
// islet and a crane in the middle reached by two plank bridges. Rotate180
// (the west half is built, the east half is its half-turn copy), so each
// team has the cutting shed on one arc and the open terraces on the other.
// Plan: docs/maps/greyfold-quarry.md.
//
//  z=-34  ^^^^^^^^^^^^^^^^^^^^ quarry face (14 m) ^^^^^^^^^^^^^^^^^^^^^^^
//         | bench y=3 |  CUTTING SHED (2 floors) |  open bench terraces ... |
//         |  stairs   |  haul road (north arc) ..... stairs   conveyor    |
//         | A  office |   (    flooded pit    [islet+crane]   )  office B |
//         |  compound |  open bench terraces | CUTTING SHED  haul road S |
//  z=+34  vvvvvvvvvvvvvvvvvvvvvvvvvv quarry face vvvvvvvvvvvvvvvvvvvvvvvvvv
//
// Lanes: outer (the benches at y = 3 and the upper floor of each shed),
// middle (the haul road at the floor of the quarry), inner (the pit rim and
// the bridges over the water to the islet: crane perch with the armour).
"""

X0, Z0 = 50.0, 34.0          # half size of the map
RIM_A, RIM_B = 22.0, 11.0    # half axes of the pit
ISLE_A, ISLE_B = 7.0, 5.5    # half axes of the islet


def in_pit(x, z):
    return (x / RIM_A) ** 2 + (z / RIM_B) ** 2 < 1.0


def in_isle(x, z):
    return (x / ISLE_A) ** 2 + (z / ISLE_B) ** 2 < 1.0


def play_area(x, z):
    """True on the walkable floor of the quarry (used by the trial table)."""
    if not (-44 <= x <= 44 and -27 <= z <= 27):
        return False
    if in_pit(x, z) and not in_isle(x, z) and not (3 <= abs(x) <= 5.5 and abs(z) >= 4):
        return False
    return True


def lamp(m, x, z, y=0.0):
    m.box(x, y - 0.5, z, x + 0.5, y + 3.0, z + 0.5, "metal")
    m.box(x - 0.25, y + 3.0, z - 0.25, x + 0.75, y + 3.5, z + 0.75, "lamp_white")
    m.light(x + 0.25, y + 3.2, z + 0.25, (0.9, 0.95, 1.0), 1.2, 10.0)


def ring(m, a0, b0, a1, b1, y0, y1, mat, n, keep):
    """Wall along an ellipse, between the half axes (a0, b0) and (a1, b1): `n`
    segments over the west half (x <= 0, the half turn builds the rest),
    leaving out the segments whose middle fails `keep(x, z)`."""
    for k in range(n):
        t0 = math.pi / 2 + math.pi * k / n
        t1 = math.pi / 2 + math.pi * (k + 1) / n
        mx, mz = (a0 + a1) / 2 * math.cos((t0 + t1) / 2), (b0 + b1) / 2 * math.sin((t0 + t1) / 2)
        if not keep(mx, mz):
            continue
        pts = [(a0 * math.cos(t0), b0 * math.sin(t0)), (a0 * math.cos(t1), b0 * math.sin(t1)),
               (a1 * math.cos(t1), b1 * math.sin(t1)), (a1 * math.cos(t0), b1 * math.sin(t0))]
        m.prism(pts, y0, y1, mat)


def build():
    m = Map("Greyfold Quarry", "A marble quarry: a loop of haul roads around a flooded pit, cutting sheds and an islet crane.",
            (2, 16), {"ambience": "day", "sky": "overcast", "palette": "nature",
                      "weather": [(0.0, "clear"), (200.0, "rain")]}, HEADER, gravity=9.81)
    m.track_prisms = True
    r = random.Random(23)

    # ---- the whole map -----------------------------------------------------------------------------
    m.zone(-X0 - 4, -20, -Z0 - 4, X0 + 4, -4, Z0 + 4, "KillVolume")       # the pit water and below
    m.zone(-RIM_A, -12, -RIM_B, RIM_A, -6, RIM_B, "Water")

    # ---- centre (built once): the crane on the islet ----------------------------------------------------
    m.box(-2, -0.5, -2, 2, 2.5, 2, "metal")                                     # crane base = perch
    m.box(-2, 2.5, -2, 2, 3.5, -1.5, "metal_orange")                            # perch railings
    m.box(-2, 2.5, 1.5, 2, 3.5, 2, "metal_orange")
    m.box(-0.5, 3.5, -2, 0.5, 9.0, -1.5, "metal_orange")                        # twin masts
    m.box(-0.5, 3.5, 1.5, 0.5, 9.0, 2, "metal_orange")
    m.box(-1, 9.0, -2, 1, 9.5, 2, "metal")
    m.box(-12, 9.5, -0.75, 12, 10.5, 0.75, "metal_orange")                      # jib
    m.box(-1.5, 10.5, -1.5, 1.5, 11.5, 1.5, "metal")

    # ---- the west half, copied by the half turn ----------------------------------------------------------
    m.sym = True
    # quarry faces: stepped walls all around (north side and west end built, the rest is the copy)
    x = -X0
    while x < X0:
        w = min(5 + 4 * r.random(), X0 - x)
        h = 12 + 3 * r.random()
        m.box(x, -0.5, -Z0, x + w, h, -28, "stone")
        m.box(x + 0.5, h, -Z0, x + w - 0.5, h + 1.5 + r.random(), -30, "stone")
        x += w
    m.box(-X0, -0.5, -28, -46, 14, 28, "stone")
    m.box(-X0, 14, -28, -48, 15.5, 28, "stone")
    for (cx, cz, s) in ((-46, -28, 4.0), (-46, -24, 2.0), (-42, -28, 2.0)):
        m.box(cx, -0.5, cz, cx + s, 14, cz + s, "stone")                       # rounded corner
    # the north bench (y = 3) along the face, with stairs down to the haul road
    m.box(-42, -0.5, -28, -30, 3.0, -22, "mossy_stone")
    m.box(-12, -0.5, -28, 36, 3.0, -22, "mossy_stone")
    m.stairs(-40, 0.0, -22, -37, 3.0, -18.5, "NegZ", 7, "mossy_stone")
    m.stairs(10, 0.0, -22, 13, 3.0, -18.5, "NegZ", 7, "mossy_stone")
    m.stairs(30, 0.0, -22, 33, 3.0, -18.5, "NegZ", 7, "mossy_stone")
    for (bx, bz) in ((-36, -26), (-4, -26.5), (4, -24.5), (20, -26.5), (26, -24)):
        m.box(bx, 3.0, bz, bx + 2, 4.0, bz + 1, "concrete_white")               # low cover on the bench
    m.box(16, 3.0, -27.5, 18, 5.0, -25.5, "concrete_white")
    # the cutting shed: machine hall below, gallery at the bench level above
    house(m, -30, -28, -12, -18, "brick", storeys=2, h=3.0,
          openings=[("S", -27.0, -23.0, "door", 0), ("S", -19.0, -15.5, "door", 0),
                    ("E", -21.5, -19.0, "door", 0), ("W", -21.5, -19.0, "door", 0),
                    ("E", -26.5, -23.5, "door", 1), ("W", -26.5, -23.5, "door", 1),
                    ("S", -29.0, -27.5, "window", 0), ("S", -22.5, -19.5, "window", 0),
                    ("S", -29.0, -26.0, "window", 1), ("S", -24.0, -20.0, "window", 1),
                    ("S", -18.0, -14.0, "window", 1), ("E", -21.5, -19.0, "window", 1),
                    ("W", -21.5, -19.0, "window", 1)],
          stair=(-17.5, -27.5, -14.0, -25.5, "PosX"), roof="metal", floor="concrete", trim="metal",
          gable=False)
    m.box(-28, 0, -24.5, -22, 1.2, -23, "metal")                                # saw bench
    m.box(-26, 1.2, -24, -25, 2.0, -23.5, "metal_orange")
    m.box(-20, 0, -24.5, -18, 1.5, -22.5, "concrete_white")                     # blocks
    # haul road cover (north arc): cut blocks, a dump truck, a conveyor
    for (bx, bz, w, d, h) in ((-6, -15.5, 2, 1, 1.0), (-5, -14.5, 1, 1, 2.0), (16, -17, 2, 2, 1.5),
                              (24, -14, 2, 1, 1.0), (-33, -15, 2, 2, 2.0), (-31, -15, 1, 2, 1.0)):
        m.box(bx, -0.5, bz, bx + w, h, bz + d, "concrete_white")
    m.box(-2, -0.5, -20.5, 3, 2.0, -18.5, "metal_green")                       # dump truck
    m.box(3, -0.5, -20.5, 5, 2.5, -18.5, "metal_green")
    m.cyl(-1.5, -0.5, -21, 0.0, 1.0, -20.5, 8, "asphalt", axis="Z")
    m.cyl(3.0, -0.5, -21, 4.5, 1.0, -20.5, 8, "asphalt", axis="Z")
    for x in (28.0, 33.0, 38.0):
        m.box(x, -0.5, -21.0, x + 0.5, 2.5, -20.5, "metal")                    # conveyor legs
    m.box(27.5, 2.5, -21.5, 39.0, 3.0, -20.0, "asphalt")
    m.box(-12, -0.5, -12.5, -8, 0.5, -11.5, "concrete_white")                   # rim blocks
    m.box(12, -0.5, -12.5, 15, 0.5, -11.5, "concrete_white")
    # the bridge to the islet (north side; the copy is the south one)
    m.box(3, -0.5, -12, 5.5, 0.0, -4, "wood_light")
    for z in (-10.0, -7.0):
        m.box(3, -6.0, z, 3.5, -0.5, z + 0.5, "wood_dark")
        m.box(5, -6.0, z, 5.5, -0.5, z + 0.5, "wood_dark")
    # low walls around the pit rim and the islet (gaps at the bridge heads): jump them at your own risk
    gap = lambda x, z: not (-7.0 <= x <= -1.5 and z > 0)
    ring(m, RIM_A - 0.3, RIM_B - 0.3, RIM_A + 0.7, RIM_B + 0.7, -0.5, 1.0, "concrete_white", 14, gap)
    ring(m, ISLE_A - 1.0, ISLE_B - 1.0, ISLE_A, ISLE_B, -0.5, 1.0, "concrete_white", 8, gap)
    # stairs up to the crane perch
    m.stairs(2, 0.0, -1, 5, 2.5, 1, "NegX", 6, "metal")
    # the west end: the office compound (spawn of team A)
    house(m, -46, -4, -40, 4, "concrete_white", storeys=2, h=3.0,
          openings=[("E", -3.0, -1.0, "door", 0), ("E", 1.0, 3.0, "window", 0), ("E", -3.0, 3.0, "window", 1),
                    ("N", -45.0, -42.0, "window", 1)],
          stair=(-45.5, 0.5, -42.0, 3.5, "PosX"), roof="metal", floor="concrete", trim="metal", gable=False)
    wall(m, -46, -10.5, -34, -10, -0.5, 2.5, "brick", holes=[(-40.0, -36.0, -0.5, 2.5)])
    wall(m, -46, 10, -34, 10.5, -0.5, 2.5, "brick", holes=[(-40.0, -36.0, -0.5, 2.5)])
    wall(m, -34.5, -10, -34, 10, -0.5, 2.5, "brick", holes=[(-10.0, -7.0, -0.5, 2.5), (7.0, 10.0, -0.5, 2.5)])
    m.box(-37.5, -0.5, -1, -36.5, 1.2, 3, "concrete_white")                    # block in the yard
    m.box(-28, -0.5, -2, -26, 2.0, 0, "concrete_white")                        # west rim cover
    m.box(-29, -0.5, 0, -28, 1.0, 1, "concrete_white")
    for (lx, lz) in ((-33.5, -11), (-11, -17.5), (14, -18), (34, -18)):
        lamp(m, lx, lz)

    # ---- gameplay ------------------------------------------------------------------------------------
    spots = m.find_spots(lambda x, z: -45.5 <= x <= -35.5 and -9.0 <= z <= 9.0 and not (-46.5 <= x <= -39.0 and -5.0 <= z <= 5.0),
                         8, gap=2.5, margin=0.9, near=(-38, 0))
    assert len(spots) == 8, spots
    for x, z in spots:
        m.spawn(x, 0.0, z, facing(x, z, 0, 0), "A")
    m.pickup("Health", -21.0, 0.5, -21.0, 25)      # shed, machine hall
    m.pickup("Health", 22.0, 3.5, -25.0, 25)       # north bench
    m.pickup("Ammo", -30.0, 0.5, -5.0, 20)         # west rim
    m.pickup("Ammo", 8.0, 0.5, -15.0, 20)          # haul road
    m.sym = False
    m.pickup("Armor", 0.0, 3.0, 0.0, 45)           # crane perch on the islet

    # ---- terrain (lot 6.10): quarry floor with spoil heaps, the pit and the islet ------------------------
    def raw(x, z):
        return 0.6 * (math.sin(0.31 * x) * math.sin(0.43 * z)) ** 2

    def override(x, z):
        if in_isle(x, z):
            return 0.0
        if (x / (RIM_A - 0.6)) ** 2 + (z / (RIM_B - 0.6)) ** 2 < 1.0:
            return -10.0
        return None

    def paint(x, z):
        if in_pit(x, z) and not in_isle(x, z):
            return "rock"
        return "dirt"

    m.auto_terrain(-X0, -Z0, X0, Z0, 1.0, raw, override=override, paint=paint)

    # ---- vegetation (lot 6.11): weeds and shrubs on the gravel, pines on top of the faces -----------
    m.set_vegetation(29, [("terrain:dirt", "Grass", 5.0), ("terrain:dirt", "Bush", 0.25),
                          ("terrain:dirt", "Rock", 0.3), ("stone", "Pine", 1.5)])

    # ---- landscape beyond the bounds (lot 6.12): wooded hills around the pit -----------------------
    m.set_surroundings((-X0, -Z0, X0, Z0), "Forest", "Slope", "Forest", "Slope", "Mountains", 0.7, 17, ground=14.0)
    return m


if __name__ == "__main__":
    import sys
    build().save(sys.argv[1])
