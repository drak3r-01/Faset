import math, random
from maplib import *
from map_cove import facing

HEADER = """// Rust Harbor: a container quay at night. Team map, 14 players, Rotate180
// (the west half is built, the east half is its half-turn copy). A strip of
// quay between two stretches of deadly black water (a KillVolume below the
// quay, the sea drawn at y=-1); on each long side a cargo ship is moored,
// reached by a gangway (deck 1.5 m); a gantry crane spans the middle. Brick
// warehouses close the east and west ends, the far shores are breakwater
// rocks. Sodium lamps and the ships' lights are the only illumination.
//
//        sea (deadly)  ~~~~~~~~ ship (N) ~~~~~~~~
//   +-----------------------------------------------+
//   | warehouse W |  container lanes  | crane | ...  | warehouse E |
//   +-----------------------------------------------+
//        sea (deadly)  ~~~~~~~~ ship (S) ~~~~~~~~
"""


def container(m, x, z, y=0.0, along="x", mat="container_red", length=6.0):
    w, h = 2.4, 2.6
    if along == "x":
        m.box(x, y, z, x + length, y + h, z + w, mat)
        for i in range(1, 4):
            m.box(x + i * length / 4 - 0.05, y + 0.1, z - 0.05, x + i * length / 4 + 0.05, y + h - 0.1, z + w + 0.05, "metal")
    else:
        m.box(x, y, z, x + w, y + h, z + length, mat)
        for i in range(1, 4):
            m.box(x - 0.05, y + 0.1, z + i * length / 4 - 0.05, x + w + 0.05, y + h - 0.1, z + i * length / 4 + 0.05, "metal")


def lamp(m, x, z, y=0.0, h=6.0):
    m.box(x - 0.15, y, z - 0.15, x + 0.15, y + h, z + 0.15, "metal")
    m.box(x - 0.6, y + h, z - 0.4, x + 0.6, y + h + 0.3, z + 0.4, "metal")
    m.box(x - 0.45, y + h - 0.1, z - 0.3, x + 0.45, y + h, z + 0.3, "lamp_warm")
    m.light(x, y + h - 0.6, z, (1.0, 0.65, 0.3), 2.2, 16.0)


def build():
    m = Map("Rust Harbor", "A container quay at night between two dark basins: moored ships, a gantry crane and warehouses.",
            (2, 14), {"ambience": "night", "sky": "stars", "palette": "industrial", "weather": [(0.0, "clear")]},
            HEADER, gravity=9.81)
    r = random.Random(23)

    # ---- sea, far shores, quay ----------------------------------------------------------------------------
    m.box(-70, -8, -56, 70, -4, 56, "sand")
    m.zone(-72, -14, -58, 72, -1.5, 58, "KillVolume")                  # the water is deadly
    m.zone(-72, -8, -58, 72, -1.0, 58, "Water")                        # sea level (drawn by the renderer)
    m.box(-56, -4, -22, 56, 0, 22, "asphalt")                          # the quay
    m.box(-56, -4, -22.5, 56, -0.4, -22, "concrete")                   # quay walls, a lip above the water
    m.box(-56, -4, 22, 56, -0.4, 22.5, "concrete")
    # breakwater rocks along the far shores (north and south) and the east/west seawalls
    for side in (-1, 1):
        x = -70.0
        while x < 70:
            w = min(4 + 5 * r.random(), 70 - x)
            if w < 1:
                break
            h = 3 + 4 * r.random()
            z0 = 46 if side > 0 else -56
            m.box(x, -4, z0, x + w, h, z0 + 10, "basalt")
            x += w
    for side in (-1, 1):
        m.box(side * 66 - (0 if side > 0 else 4), -4, -56, side * 66 + (4 if side > 0 else 0), 5, 56, "basalt")

    # ---- the west half (copied by the half turn) -------------------------------------------------------------
    m.sym = True
    # warehouse closing the end of the quay: a 5 m high block with a roof terrace reached by stairs
    m.box(-56, 0, -22, -44, 6.5, 22, "brick_slate")
    m.box(-56, 6.5, -22, -44, 7.0, 22, "concrete")
    m.box(-56, 7.0, -22, -55.5, 8.0, 22, "concrete")
    m.box(-56, 7.0, -22, -44, 8.0, -21.5, "concrete")
    m.box(-56, 7.0, 21.5, -44, 8.0, 22, "concrete")
    m.stairs(-44, 0, 13, -36, 6.5, 17, "NegX", 16, "metal")               # stairs to the roof
    m.box(-44, 0, 17, -36, 4.0, 17.5, "metal")                              # stair rail
    m.box(-50, 7.0, -4, -46, 9.0, 4, "metal_orange")                        # roof cabin
    # loading dock: a platform at 1.2 m with a ramp
    m.box(-44, 0, -16, -41, 1.2, -4, "concrete")
    m.wedge(-41, 0, -16, -37, 1.2, -4, "NegX", "concrete")
    # container lanes: stacks that can be climbed through ramps
    container(m, -34, -19, 0.0, "x", "container_red")
    container(m, -34, -19, 2.6, "x", "container_blue")
    container(m, -26, -19, 0.0, "x", "metal_orange")
    container(m, -33, -12, 0.0, "x", "container_blue")
    container(m, -33, -12, 2.6, "x", "metal_green")
    container(m, -33, -12, 5.2, "x", "container_red")
    m.wedge(-37.5, 0, -12, -33, 2.6, -9.6, "PosX", "metal")                  # ramp onto the first level (blocks 0..2.6)
    container(m, -26, -8, 0.0, "z", "container_red", 6.0)
    container(m, -26, -8, 2.6, "z", "metal_orange", 6.0)
    container(m, -18, -17, 0.0, "x", "metal_green")
    container(m, -17, -9, 0.0, "x", "container_blue")
    container(m, -17, -9, 2.6, "x", "container_red")
    container(m, -17, -2, 0.0, "x", "container_red")
    container(m, -28, 3, 0.0, "x", "container_blue")
    container(m, -28, 3, 2.6, "x", "metal_orange")
    container(m, -22, 8, 0.0, "x", "metal_green")
    container(m, -22, 8, 2.6, "x", "container_red")
    container(m, -14, 12, 0.0, "z", "container_blue", 6.0)
    container(m, -14, 12, 2.6, "z", "container_red", 6.0)
    container(m, -8, 5, 0.0, "x", "metal_orange")
    container(m, -34, 14, 0.0, "x", "container_red")
    # fuel tank and crates at the quay edge
    m.cyl(-10, 0, -18, -4, 5, -12, 10, "metal")
    m.box(-10.3, 2.4, -18.3, -3.7, 2.8, -11.7, "hazard")
    crate(m, -38, -3, 0.0, 1.2)
    crate(m, -38, -1.6, 0.0, 1.2)
    crate(m, -37.4, -2.3, 1.2, 1.0)
    crate(m, -2, 18, 0.0, 1.3)
    crate(m, -2, 19.4, 0.0, 1.3)
    # bollards and barriers
    for z in (-21.5, -18.5):
        pass
    for x in (-30, -24, -12, -6):
        m.box(x - 0.3, 0, -21.8, x + 0.3, 0.7, -21.2, "metal")
    m.box(-8, 0, 6, -4.5, 1.0, 6.6, "hazard")
    m.box(-12, 0, -4, -11.4, 1.0, 0, "hazard")
    # the ship moored at the north quay (z < -22): hull, deck at 1.5, bridge house, funnel, cargo
    m.box(-22, -3.5, -40, 12, 1.5, -22.5, "metal")
    m.wedge(-34, -3.5, -40, -22, 1.5, -22.5, "NegX", "metal")
    m.box(-22, 1.5, -40, 12, 2.5, -39.5, "metal")
    m.box(-22, 1.5, -23, -18.5, 2.5, -22.5, "metal")
    m.box(-11.5, 1.5, -23, 12, 2.5, -22.5, "metal")
    m.box(-4, 1.5, -39, 8, 5.5, -33, "wood_white")                           # bridge house
    m.box(-4.5, 5.5, -39.5, 8.5, 6.0, -32.5, "metal")
    m.box(-2, 6.0, -38, 4, 9.0, -35, "container_red")                       # funnel
    container(m, -20, -32, 1.5, "x", "container_blue")
    container(m, -20, -29, 1.5, "x", "metal_orange")
    container(m, -12, -29, 1.5, "x", "container_red")
    container(m, -12, -32, 1.5, "x", "metal_green")
    m.wedge(-18, 0, -22.5, -12, 1.5, -16.5, "NegZ", "concrete")             # gangway from the quay to the deck
    # lamps with their halos
    for (x, z) in ((-40, -10), (-40, 10), (-30, 0), (-20, -16), (-20, 17), (-8, -10), (-6, 12)):
        lamp(m, x, z)
    m.light(-47, 3, 0, (1.0, 0.5, 0.2), 1.4, 12.0)
    m.light(-1, 8, -36, (0.6, 0.8, 1.0), 1.6, 14.0)
    m.sym = False
    # gantry crane over the middle of the quay
    for z in (-18.5, 18.5):
        m.box(-1.0, 0, z - 0.5, 1.0, 15, z + 0.5, "metal_orange")
        m.box(-0.5, 0, z - 1.5, 0.5, 15, z - 0.5, "metal_orange") if False else None
    m.box(-1.5, 15, -22, 1.5, 16.5, 22, "metal_orange")
    m.box(-2.5, 12.5, -5, 2.5, 15, 5, "metal")
    m.box(-0.6, 8, -0.6, 0.6, 12.5, 0.6, "metal")
    m.light(0, 14, 0, (1.0, 0.9, 0.7), 1.8, 22.0)

    # ---- gameplay -----------------------------------------------------------------------------------------------
    spots = m.find_spots(lambda x, z: -43 < x < -36 and -14 < z < 12 and not (-17 < z < -3 and x < -40), 7, gap=3.0, near=(-40, 0))
    assert len(spots) == 7, spots
    m.sym = True
    for x, z in spots:
        m.spawn(x, 0.0, z, facing(x, z, 0, 0), "A")
    m.pickup("Health", -22, 0.5, 0, 25)
    m.pickup("Health", -8, 0.5, 14, 30)
    m.pickup("Armor", 5, 2.0, -30, 45)
    m.pickup("Ammo", -37, 0.5, 6, 20)
    m.pickup("Ammo", -12, 0.5, -20, 20)
    m.sym = False

    # ---- vegetation (lot 6.11): weeds in the cracks of the quay ----
    m.set_vegetation(23, [("asphalt", "Grass", 2.0)])
    # ---- landscape beyond the bounds (lot 6.12)
    m.set_surroundings((-70, -56, 70, 56), "Sea", "Slope", "Sea", "Rocks", "Mountains", 0.8, 5, sea_level=-1.0)
    return m


if __name__ == "__main__":
    import sys
    build().save(sys.argv[1])
