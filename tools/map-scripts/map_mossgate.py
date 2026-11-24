import math
from maplib import *
from map_cove import facing

HEADER = """// Mossgate: a walled mill village on a pond, on a rainy autumn day. Team
// map, 16 players, L-shaped: two village streets meet at a square in the
// elbow of the L, the inside of the L is the mill pond. Built as one arm plus
// the half of the elbow; the other arm is its mirror across the vertical
// plane x = z, which swaps the two teams. Plan: docs/maps/mossgate.md.
//
//  z=-22  ######################### town wall ###########################
//         |chapel| orchard lane (outer) ......................... |barn |
//         |  ch. | bakery | | house | | inn (2 floors) | |shed|   |     |
//         | yard |  SQUARE (well)  high street (middle)  cross  cart | A |
//         |      |         | stalls, wash-house ................ |yard |
//         |      |         | mill race walk (inner), pond wall .. |     |
//   z=10  |  B arm (mirror) [mill]~~~~~~~~~~~ mill pond (deadly) ~~~~~~~ |
//
// Lanes: outer (orchard, through the chapel in the corner), middle (high
// street, then the square) and inner (mill race walk along the pond wall,
// which ends at the square or goes through the two-floor mill standing in
// the water). Spawns are the farmyards at the ends of the arms.
"""

W, I, E = -22.0, 10.0, 54.0  # outer edge, inner corner, end of the arms


def play_area(x, z):
    """True on the walkable ground of the map, walls of the border left out
    (used by the trial table)."""
    arm_a = W + 2 <= x <= E - 2 and W + 2 <= z <= I
    arm_b = W + 2 <= z <= E - 2 and W + 2 <= x <= I
    mill = I <= x <= 17 and I <= z <= 17
    return arm_a or arm_b or mill


def build():
    m = Map("Mossgate", "A walled mill village on a pond: an L of two streets, a square, a chapel and a water mill.",
            (2, 16), {"ambience": "storm", "sky": "overcast", "palette": "nature",
                      "weather": [(0.0, "rain"), (180.0, "clear")]}, HEADER, gravity=9.81)
    m.track_prisms = True

    # ---- the whole map: kill volume, pond --------------------------------------------------------
    m.zone(W - 4, -12, W - 4, E + 4, -1, E + 4, "KillVolume")
    m.zone(I, -6, I, E, -0.5, E, "Water")

    # ---- the elbow, on the mirror plane (built once) ---------------------------------------------------
    m.box(-6, -0.4, -6, 10, 0.0, 10, "stone")                                   # the square
    m.cyl(0.5, 0.0, 0.5, 3.5, 1.0, 3.5, 12, "mossy_stone")                      # the well
    m.box(1.5, 1.0, 1.5, 2.5, 2.5, 2.5, "wood_dark")
    m.box(0.0, 2.5, 0.0, 4.0, 3.0, 4.0, "wood")
    m.box(-13, -0.4, -13, -6, 0.0, -6, "dirt")                                  # churchyard
    # chapel in the outer corner: two floors, the upper one with windows down both orchard lanes
    house(m, -21, -21, -13, -13, "mossy_stone", storeys=2, h=3.5,
          openings=[("E", -18.5, -16.0, "door", 0), ("S", -18.5, -16.0, "door", 0),
                    ("E", -20.0, -15.0, "window", 1), ("S", -20.0, -15.0, "window", 1),
                    ("E", -20.0, -19.0, "window", 0), ("S", -20.0, -19.0, "window", 0)],
          stair=(-20.5, -20.5, -16.5, -18.5, "NegX"), roof="brick_slate", floor="wood_dark")
    m.box(-21, 7.5, -21, -17, 13.0, -17, "mossy_stone")                         # bell tower
    m.box(-20.5, 10.0, -21.5, -17.5, 12.0, -16.5, "stone")
    m.box(-21.5, 10.0, -20.5, -16.5, 12.0, -17.5, "stone")
    m.wedge(-21.5, 13.0, -21.5, -19, 15.5, -16.5, "PosX", "brick_slate")
    m.wedge(-19, 13.0, -21.5, -16.5, 15.5, -16.5, "NegX", "brick_slate")
    # the water mill standing in the pond at the inner corner: entered from both mill race walks,
    # upper floor with windows over the square and the water (the armour is up there)
    for (sx, sz) in ((10.5, 16), (16, 10.5), (16, 16)):
        m.box(sx, -3.0, sz, sx + 1, -0.5, sz + 1, "wood_dark")                  # stilts
    house(m, I, I, 17, 17, "stone", storeys=2, h=3.5,
          openings=[("N", 11.5, 13.5, "door", 0), ("W", 11.5, 13.5, "door", 0),
                    ("N", 13.5, 16.0, "window", 0), ("W", 13.5, 16.0, "window", 0),
                    ("N", 11.0, 16.0, "window", 1), ("W", 11.0, 16.0, "window", 1),
                    ("E", 11.0, 14.0, "window", 1), ("S", 11.0, 14.0, "window", 1)],
          stair=(11.5, 14.5, 15.5, 16.5, "PosX"), roof="clay", floor="wood")
    m.cyl(17.5, -1.5, 11.0, 18.5, 3.5, 16.0, 12, "wood_dark", axis="X")         # water wheel
    m.box(17, 0.5, 13, 17.5, 1.5, 14, "wood")
    m.light(13.5, 6.0, 13.5, (1.0, 0.75, 0.45), 1.4, 10.0)

    # ---- one arm (east) and its half of the elbow; the mirror builds the other ------------------
    m.sym = "diag"
    # town wall along the north edge, with buttresses
    m.box(W, -0.5, W, E, 5.0, W + 1, "mossy_stone")
    for x in range(-6, 46, 8):
        m.box(x, -0.5, W + 1, x + 1, 3.5, W + 1.5, "stone")
    m.box(W, 5.0, W, E, 5.5, W + 1.5, "stone")
    m.box(E - 0.5, -0.5, W + 1, E, 5.0, I, "stone")                             # back of the farm
    m.light(-12.5, 4.5, -17.0, (1.0, 0.75, 0.45), 1.6, 12.0)                   # chapel lamp
    # churchyard: graves and a low wall towards the orchard
    for x in (-11.5, -9.0):
        m.box(x, -0.4, -10.5, x + 1.0, 1.0, -10.0, "stone")
    wall(m, -13, -13.5, -6, -13, -0.4, 1.0, "mossy_stone", holes=[(-10.5, -8.5, -0.4, 1.0)])

    # streets
    m.box(10, -0.4, -6, 46, 0.0, 2, "stone")                                    # high street
    m.box(10, -0.4, 2, 44, 0.0, 6, "stone")                                     # stalls row
    m.box(17, -0.4, 6, 44, 0.0, 10, "wood_dark")                                # mill race walk
    m.box(10, -0.4, 6, 17, 0.0, I, "wood_dark")
    wall(m, 17, 9.5, 44, 10, -0.4, 2.5, "mossy_stone",
         holes=[(22.0, 24.0, 1.0, 2.0), (31.0, 33.0, 1.0, 2.0), (39.0, 41.0, 1.0, 2.0)])   # pond wall, windows
    m.box(17, 2.5, 9.0, 44, 3.0, 10.5, "brick_slate")
    for x, z in ((12, -5.5), (24, -5.5), (36, -5.5), (28, 8.5), (6, -5.5)):
        m.box(x, 0, z, x + 0.5, 3.0, z + 0.5, "wood_dark")                      # lamp posts
        m.box(x - 0.25, 3.0, z - 0.25, x + 0.75, 3.5, z + 0.75, "lamp_warm")
        m.light(x + 0.25, 3.2, z + 0.25, (1.0, 0.8, 0.5), 1.3, 9.0)
    # square: planters and benches
    m.box(3.5, -0.4, -4.5, 6, 1.0, -3, "brick")
    m.box(-4, -0.4, 7.5, -1.5, 0.5, 8.5, "wood")

    # the row of houses between the orchard and the high street (doors on both sides)
    house(m, -5, -13, 4, -6, "brick", openings=[("N", -2.5, -0.5, "door", 0), ("S", 1.0, 3.0, "door", 0),
                                                ("S", -4.0, -1.5, "window", 0), ("E", -11.0, -9.0, "window", 0)],
          roof="clay")                                                          # bakery
    m.box(-4.5, 0, -12.5, -2.5, 1.0, -10.5, "brick")                            # bread oven
    house(m, 7, -13, 15, -6, "concrete_white", openings=[("N", 9.0, 11.0, "door", 0), ("S", 10.0, 12.0, "door", 0),
                                                         ("E", -11.0, -9.0, "window", 0),
                                                         ("S", 7.5, 9.0, "window", 0), ("W", -11.0, -9.0, "window", 0)],
          roof="brick_slate")
    house(m, 19, -13, 29, -6, "wood", storeys=2, h=3.5,
          openings=[("N", 20.0, 22.0, "door", 0), ("S", 24.0, 26.0, "door", 0),
                    ("S", 20.0, 22.5, "window", 0), ("S", 27.0, 28.5, "window", 0),
                    ("S", 20.0, 23.0, "window", 1), ("S", 25.0, 28.0, "window", 1),
                    ("N", 23.0, 26.0, "window", 1), ("E", -11.0, -8.0, "window", 1),
                    ("W", -11.0, -8.0, "window", 1)],
          stair=(23.5, -12.5, 27.5, -10.5, "PosX"), roof="brick_slate", floor="wood_light")   # the inn
    m.box(20, 0, -9, 22.5, 1.0, -7.5, "wood_dark")                              # inn counter
    house(m, 33, -13, 39, -6, "wood_dark", openings=[("S", 35.0, 37.0, "door", 0), ("N", 34.0, 36.0, "door", 0),
                                                     ("W", -11.0, -9.0, "window", 0)],
          roof="wood", roof_h=1.0)                                              # tool shed

    # stalls and the wash-house between the high street and the mill race walk
    for x0 in (11.5, 28.5):
        m.box(x0, 0, 2.5, x0 + 4, 1.0, 3.5, "wood")                               # counter
        for px in (x0, x0 + 3.5):
            m.box(px, 0, 5.0, px + 0.5, 2.5, 5.5, "wood_dark")
        m.box(x0 - 0.5, 2.5, 2.0, x0 + 4.5, 3.0, 6.0, "container_red" if x0 < 20 else "tiles_teal")
    m.box(19, -0.4, 2.5, 19.5, 2.5, 6, "stone")                                  # wash-house
    m.box(25.5, -0.4, 2.5, 26, 2.5, 6, "stone")
    m.box(18.5, 2.5, 2, 26.5, 3.0, 6.5, "brick_slate")
    m.box(20.5, 0, 3.5, 24.5, 0.5, 5.0, "stone")
    for x in (36.0, 37.2):
        crate(m, x, 4.0, 0.0)
    crate(m, 36.6, 4.0, 1.0)
    # market cross and a cart in the high street (they break the long sightline)
    m.box(21.5, 0, -3.5, 24.5, 0.5, -0.5, "stone")
    m.box(22.5, 0.5, -2.5, 23.5, 4.5, -1.5, "mossy_stone")
    m.box(22, 3.5, -2.25, 24, 4.0, -1.75, "mossy_stone")
    m.box(35, 0, -0.5, 39, 1.5, 1.5, "wood")                                    # cart
    m.cyl(35.5, -0.4, -1.0, 36.5, 1.0, -0.5, 8, "wood_dark", axis="Z")
    m.cyl(37.5, -0.4, -1.0, 38.5, 1.0, -0.5, 8, "wood_dark", axis="Z")

    # the orchard lane: dry-stone walls (low cover) and a cider press
    for x0, x1, z in ((-2, 3, -17.5), (14, 19, -16.0), (28, 33, -18.0), (38, 42, -16.5)):
        m.box(x0, -0.4, z, x1, 1.0, z + 0.5, "stone")
    m.box(7, -0.4, -19.5, 9, 1.5, -17.5, "wood_dark")
    m.box(7.5, 1.5, -19, 8.5, 3.0, -18, "wood")

    # the farmyard (spawn of the team of this arm): barn, walls, hay
    house(m, 45, -21, E - 0.5, -10, "wood_dark", h=6.0,
          openings=[("W", -17.0, -14.0, "door", 0), ("S", 47.0, 51.0, "door", 0)], roof="container_red", roof_h=2.5)
    m.box(46, 0, -20.5, 49, 1.5, -18, "savanna")                                # hay inside
    m.box(44, -0.4, 9.5, E - 0.5, 3.0, 10, "stone")                             # pond-side farm wall
    wall(m, 42, -6, 42.5, 10, -0.4, 2.5, "stone", holes=[(-3.0, 0.0, -0.4, 2.5), (6.0, 9.5, -0.4, 2.5)])
    m.box(46.5, -0.4, -4.5, 49, 1.5, -1.0, "savanna")                           # hay wagon behind the gate
    m.box(48.5, -0.4, 3, 50.5, 1.2, 5, "savanna")
    m.box(44, -0.4, -9.5, 45, 1.0, -6.5, "wood")                                # trough
    m.light(46, 4.5, -9.6, (1.0, 0.8, 0.5), 1.5, 12.0)

    # ---- gameplay ---------------------------------------------------------------------------------
    spots = m.find_spots(lambda x, z: 44.5 <= x <= 52.5 and -8.5 <= z <= 8.5, 8, gap=2.5, margin=0.9, near=(50, 0))
    assert len(spots) == 8, spots
    for x, z in spots:
        m.spawn(x, 0.0, z, facing(x, z, 20, -4), "A")
    m.pickup("Health", 9.5, 0.5, -15.5, 25)        # orchard, by the cider press
    m.pickup("Health", 26.0, 0.5, -8.0, 30)        # inn, ground floor
    m.pickup("Ammo", 22.5, 0.5, 4.5, 20)           # wash-house
    m.pickup("Ammo", -9.0, 0.5, -12.0, 20)         # churchyard
    m.sym = False
    m.pickup("Armor", 12.5, 4.0, 12.5, 45)         # mill, upper floor: windows on every side
    m.pickup("Health", -1.0, 0.5, -1.0, 25)        # square
    m.pickup("Ammo", 7.0, 0.5, 7.0, 20)            # square, by the mill doors
    m.pickup("Health", -16.5, 0.5, -16.5, 30)      # chapel, ground floor

    # ---- terrain (lot 6.10): orchard hummocks, pond bed, islet and the far bank -------------------
    def raw(x, z):
        return 0.3 * (1 + math.cos(0.35 * x + 0.15 * z)) * (1 + math.cos(0.35 * z + 0.15 * x)) / 2

    def override(x, z):
        if x <= I + 0.2 or z <= I + 0.2:
            return None
        r = math.hypot(x - I, z - I)
        if math.hypot(x - 34, z - 34) < 4.5:
            return 0.5                                                     # islet
        if r < 40:
            return -3.0
        return min(0.6, -3.0 + (r - 40) * 1.2) + 0.15 * math.cos(0.5 * (x + z))

    def paint(x, z):
        if (x > 42 and z < I) or (z > 42 and x < I):
            return "dirt"
        if x > I and z > I and math.hypot(x - I, z - I) < 41:
            return "sand"
        return None

    m.auto_terrain(W, W, E, E, 1.0, raw, override=override, paint=paint)

    # ---- vegetation (lot 6.11): orchard trees and bushes placed by hand (the diagonal mirror has
    # no format symmetry), grass and ferns scattered ------------------------------------------------------
    plants = []
    m.sym = "diag"
    for i, x in enumerate((1.0, 5.0, 12.0, 21.0, 26.0, 35.5, 40.0)):
        plants += m.plant("Oak", x, -19.5 if i % 2 else -15.0, yaw=30 * i)
    for x, z in ((12.0, -14.5), (31.0, -14.5), (44.0, -15.0)):
        plants += m.plant("Bush", x, z)
    m.sym = False
    m.box(32, -0.5, 32, 36, 2.5, 32.5, "wood_dark")                             # ruined boathouse
    m.box(32, -0.5, 32, 32.5, 2.5, 36, "wood_dark")
    m.box(31.5, 2.5, 31.5, 35, 3.0, 35, "brick_slate")
    for i, (x, z) in enumerate(((33.0, 35.5), (35.5, 33.0), (46.0, 48.0), (48.0, 46.0), (40.0, 52.0), (52.0, 40.0),
                                (32.0, 52.0), (52.0, 32.0))):
        plants.append(("Birch" if i % 3 else "Oak", x, 0.5 if i < 2 else 0.6, z, 40.0 * i))
    m.set_vegetation(41, [("terrain:grass", "Grass", 30.0), ("terrain:grass", "Fern", 4.0)], plants, symmetry=None)

    # ---- landscape beyond the bounds (lot 6.12): woods behind the town walls, hills behind the pond
    m.set_surroundings((W, W, E, E), "Forest", "Slope", "Slope", "Forest", "Mountains", 0.6, 13)
    return m


if __name__ == "__main__":
    import sys
    build().save(sys.argv[1])
