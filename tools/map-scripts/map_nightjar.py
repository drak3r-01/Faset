from maplib import *
from map_cove import facing

HEADER = """// Nightjar Market: a canal-side market quarter at night, lit by lanterns.
// Team map, 16 players, three lanes around a covered market hall in the
// middle. Built as the west half and mirrored across the plane x = 0 (which
// swaps the teams), so every lane keeps its character from end to end.
// Plan: docs/maps/nightjar-market.md.
//
//  z=-38  ######## warehouses on the far bank (scenery) ##########################
//  z=-30  ~~~~~~~~~~~~~~~~~~~~~~~ canal (deadly) ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//  z=-22  |blk| towpath (north lane) .......  north square  ....... towpath  |blk|
//         |   | house (2 floors) |  | shop |  |          |  | shop |  | house |   |
//  z=-4   | A gate== market street (middle) kiosk  [ MARKET HALL ]  kiosk ==gate B |
//  z=4    |yard| house |  | shop + roof terrace | |  (2 floors)  |  ...          |yard|
//  z=12   |blk| alleys and courtyards (south lane) .. south courtyard .. alleys |blk|
//  z=18   ######## tall houses (border) ###########################################
//
// The hall is open at both ends (arcades on the street) and has doors to the
// north square and the south courtyard; its upper gallery has windows on all
// sides. Each spawn is a walled yard behind a lantern gate.
"""


def play_area(x, z):
    """True on the walkable ground of the quarter (used by the trial table)."""
    if not (-46 < x < 46 and -22 < z < 18):
        return False
    if abs(x) >= 40 and (z <= -10 or z >= 10):
        return False                                   # corner blocks
    return True


def lamp(m, x, z, mat="lamp_warm", color=(1.0, 0.72, 0.4), y=0.0, h=3.0):
    m.box(x, y - 0.5, z, x + 0.5, y + h, z + 0.5, "metal")
    m.box(x - 0.25, y + h, z - 0.25, x + 0.75, y + h + 0.5, z + 0.75, mat)
    m.light(x + 0.25, y + h + 0.2, z + 0.25, color, 1.6, 10.0)


def build():
    m = Map("Nightjar Market", "A canal-side market quarter at night: three lanes around a lantern-lit market hall.",
            (2, 16), {"ambience": "night", "sky": "stars", "palette": "urban",
                      "weather": [(0.0, "clear")]}, HEADER, gravity=9.81)
    m.track_prisms = True

    # ---- the whole map ---------------------------------------------------------------------------------
    m.zone(-52, -12, -42, 52, -2, 30, "KillVolume")
    m.zone(-48, -5, -30, 48, -1, -22, "Water")

    # ---- the market hall (centre): two halves, mirrored ------------------------------------------------
    m.sym = "mirx"
    wall(m, -10, -10, 0, -9.5, -0.4, 8.0, "brick", holes=[(-3.5, -1.0, -0.4, 3.0), (-9.0, -1.0, 5.0, 6.0)])
    wall(m, -10, 9.5, 0, 10, -0.4, 8.0, "brick", holes=[(-3.5, -1.0, -0.4, 3.0), (-9.0, -1.0, 5.0, 6.0)])
    wall(m, -10, -9.5, -9.5, 9.5, -0.4, 8.0, "brick",
         holes=[(-3.0, 3.0, -0.4, 3.5), (-8.0, -4.5, 5.0, 6.0), (4.5, 8.0, 5.0, 6.0), (-8.0, -5.0, 1.0, 2.0),
                (5.0, 8.0, 1.0, 2.0)])
    slab(m, -9.5, -9.5, 0, 9.5, 3.5, 4.0, "wood", hole=(-9.0, -9.5, -4.0, -7.5))
    m.stairs(-9, 0.0, -9.5, -4, 4.0, -7.5, "NegX", 10, "wood_dark")
    m.box(-10.5, 8.0, -10.5, 0, 8.5, 10.5, "brick_slate")                       # roof
    m.wedge(-10.5, 8.5, -10.5, 0, 10.0, 10.5, "PosX", "brick_slate")
    for z0 in (2.0, 6.0):
        m.box(-8, 0, z0, -5, 1.0, z0 + 1, "wood")                                  # stalls
        m.box(-8, 2.5, z0 - 0.5, -5, 3.0, z0 + 1.5, "container_red")
    m.box(-7, 4.0, -2, -6, 5.0, 2, "wood")                                       # gallery benches
    m.light(-5.0, 3.0, 0.0, (1.0, 0.6, 0.35), 1.4, 9.0)
    m.light(-5.0, 7.0, 0.0, (1.0, 0.6, 0.35), 1.4, 9.0)
    m.sym = False
    m.box(-1.5, -0.4, -3, 1.5, 2.5, 3, "brick")                                  # lantern stand
    m.box(-1.0, 2.5, -1.0, 1.0, 3.5, 1.0, "lamp_red")
    m.light(0.0, 3.0, 0.0, (1.0, 0.3, 0.2), 1.8, 10.0)
    m.box(-2, 10.0, -2, 2, 12.0, 2, "wood_dark")                                 # cupola
    m.box(-1.5, 12.0, -1.5, 1.5, 12.5, 1.5, "lamp_warm")
    m.light(0.0, 13.0, 0.0, (1.0, 0.75, 0.45), 2.0, 16.0)

    # ---- the west half; the mirror builds the east one ----------------------------------------------------
    m.sym = "mirx"
    m.box(-46, -0.4, -22, 0, 0.0, 18, "stone")                                  # paving
    # canal: quay walls, bollards, the far bank and its warehouses
    m.box(-48, -4.0, -23, 0, 0.0, -22, "stone")
    m.box(-48, -4.0, -31, 0, 0.0, -30, "stone")
    for x in (-38, -30, -20, -12, -4):
        m.box(x, 0, -22.5, x + 0.5, 0.5, -22, "metal")
    for (x0, x1, h, mat) in ((-48, -38, 9.0, "brick"), (-38, -27, 7.5, "wood_dark"), (-27, -16, 10.0, "brick_slate"),
                             (-16, -6, 8.0, "brick"), (-6, 0, 9.0, "wood")):
        m.box(x0, -0.5, -38, x1, h, -31, mat)
        m.box(x0 + 1.5, h - 3.0, -31.5, x0 + 3.0, h - 2.0, -31, "lamp_warm")
    # border blocks: west wall, north-west and south-west corners, south row of tall houses
    m.box(-48, -0.5, -22, -46, 9.0, 18, "brick")
    m.box(-46, -0.5, -22, -40, 8.0, -10, "brick_slate")
    m.box(-46, -0.5, 10, -40, 8.0, 18, "brick_slate")
    for (x0, x1, h, mat) in ((-46, -34, 8.0, "brick"), (-34, -24, 9.5, "concrete_white"), (-24, -12, 7.5, "brick"),
                             (-12, 0, 9.0, "wood")):
        m.box(x0, -0.5, 18, x1, h, 26, mat)
        m.box(x0 + 2, 0, 17.5, x0 + 3.5, 2.5, 18, "wood_dark")                    # doors
        m.box(x0 + 5, 4.0, 17.5, x0 + 6, 5.0, 18, "lamp_warm")                    # lit windows

    # the spawn yard and its lantern gate
    wall(m, -46, -10.5, -36, -10, -0.4, 3.0, "brick", holes=[(-40.0, -37.0, -0.4, 3.0)])
    wall(m, -46, 10, -36, 10.5, -0.4, 3.0, "brick", holes=[(-40.0, -37.0, -0.4, 3.0)])
    m.box(-37, -0.4, -4.5, -36, 5.5, -4, "brick")                               # the gate
    m.box(-37, -0.4, 4, -36, 5.5, 4.5, "brick")
    m.box(-37, 4.0, -4.5, -36, 5.5, 4.5, "brick")
    m.box(-36.5, 3.0, -0.5, -36, 4.0, 0.5, "lamp_red")
    m.light(-36.0, 3.5, 0.0, (1.0, 0.3, 0.2), 1.6, 10.0)
    m.box(-44, -0.4, -1.5, -42, 1.2, 1.5, "wood")                               # handcart in the yard
    m.box(-42, -0.4, -7, -41, 1.0, -5, "crate")

    # the north row (between the towpath and the street)
    house(m, -36, -14, -27, -4, "brick", storeys=2, h=3.5, floor=None,
          openings=[("N", -30.5, -28.5, "door", 0), ("S", -31.0, -29.0, "door", 0),
                    ("N", -35.5, -34.0, "window", 0), ("S", -35.0, -33.0, "window", 0),
                    ("N", -35.0, -31.0, "window", 1), ("S", -34.0, -28.0, "window", 1),
                    ("E", -11.0, -7.0, "window", 1), ("W", -11.0, -7.0, "window", 1)],
          stair=(-35.5, -13.5, -31.5, -11.5, "NegX"), roof="brick_slate")
    house(m, -24, -14, -13, -4, "wood", h=4.0, floor=None,
          openings=[("N", -20.0, -18.0, "door", 0), ("S", -17.0, -15.0, "door", 0),
                    ("N", -23.0, -21.0, "window", 0), ("S", -23.0, -19.5, "window", 0),
                    ("E", -12.0, -9.0, "window", 0)], roof="clay")
    m.box(-22, 0, -9, -19, 1.0, -8, "wood_dark")                                 # shop counter
    # the street: kiosk and lamps
    m.box(-25, -0.4, -1, -23, 2.5, 1, "wood_dark")
    m.box(-25.5, 2.5, -1.5, -22.5, 3.0, 1.5, "tiles_teal")
    lamp(m, -32, -3.5)
    lamp(m, -16, 3.0)
    # the south row, with a roof terrace on the long shop
    house(m, -36, 4, -28, 12, "concrete_white", h=3.5, floor=None,
          openings=[("N", -33.0, -31.0, "door", 0), ("S", -31.0, -29.0, "door", 0),
                    ("N", -35.0, -33.5, "window", 0), ("E", 6.0, 8.5, "window", 0)], roof="clay")
    house(m, -25, 4, -13, 12, "brick", h=3.5, floor=None, gable=False,
          openings=[("N", -21.0, -19.0, "door", 0), ("S", -16.0, -14.0, "door", 0),
                    ("N", -24.0, -22.0, "window", 0), ("N", -17.0, -14.0, "window", 0),
                    ("S", -24.0, -22.0, "window", 0), ("W", 6.0, 9.0, "window", 0)])
    wall(m, -25.5, 3.5, -12.5, 4.0, 4.0, 5.0, "brick")                          # terrace parapets
    wall(m, -25.5, 4.0, -25.0, 12.5, 4.0, 5.0, "brick")
    wall(m, -13.0, 4.0, -12.5, 12.5, 4.0, 5.0, "brick")
    wall(m, -25.0, 12.0, -13.0, 12.5, 4.0, 5.0, "brick", holes=[(-20.0, -17.5, 4.0, 5.0)])
    m.stairs(-23, 0.0, 12.5, -18, 4.0, 14.5, "PosX", 10, "metal")
    m.box(-22, 4.0, 7, -20, 5.0, 8, "crate")                                    # cover on the terrace
    # the north square and the towpath
    m.box(-7, -0.4, -13.5, -4, 0.8, -11.5, "grass")                             # planter
    for (x, z) in ((-33, -19.5), (-32, -19.5)):
        crate(m, x, z)
    crate(m, -32.5, -19.5, 1.0)
    m.box(-16, -0.4, -21, -12, 1.0, -20, "wood")                                # bench, nets
    m.box(-26, -0.4, -16, -25, 1.2, -15, "metal")
    lamp(m, -36, -15)
    lamp(m, -21, -15)
    lamp(m, -8, -21.5)
    # the alleys and courtyards (south lane)
    m.box(-34, -0.4, 14, -33, 1.0, 17, "wood")
    crate(m, -35.5, 16.5)
    m.box(-30, -0.4, 16.5, -27, 1.0, 17.5, "brick")                             # low wall
    m.box(-27.5, 3.5, 12, -27, 4.5, 18, "brick")                                # arch over the alley
    m.box(-11, -0.4, 13, -8, 0.8, 15, "grass")                                  # courtyard planter
    m.box(-4, -0.4, 15.5, -2, 1.0, 17.5, "crate")
    for (x, z) in ((-38, 17.5), (-24, 17.5), (-10, 17.5)):
        m.box(x, 3.0, z, x + 0.5, 3.5, z + 0.5, "lamp_red")                     # wall lanterns
        m.light(x + 0.25, 3.0, z + 0.25, (1.0, 0.35, 0.25), 1.4, 9.0)

    # ---- gameplay ---------------------------------------------------------------------------------------
    spots = m.find_spots(lambda x, z: -45.0 <= x <= -38.0 and -9.0 <= z <= 9.0, 8, gap=2.5, margin=0.9, near=(-41, 0))
    assert len(spots) == 8, spots
    for x, z in spots:
        m.spawn(x, 0.0, z, facing(x, z, 0, 0), "A")
    m.pickup("Health", -29.0, 0.5, -18.0, 25)      # towpath
    m.pickup("Health", -31.0, 0.5, 15.0, 25)       # alley
    m.pickup("Ammo", -32.0, 4.0, -7.0, 20)         # north house, upper floor
    m.pickup("Ammo", -16.0, 4.5, 8.0, 20)          # roof terrace
    m.sym = False
    m.pickup("Armor", 0.0, 4.5, 6.0, 45)           # hall gallery
    m.pickup("Health", 0.0, 0.5, -12.0, 25)        # north square
    m.pickup("Ammo", 0.0, 0.5, 14.0, 20)           # south courtyard

    # ---- terrain (lot 6.10): the canal bed; the town floor is paved -------------------------------------
    def override(x, z):
        if -30.5 < z < -22.5:
            return -4.0
        return None

    m.auto_terrain(-48, -38, 48, 26, 1.0, lambda x, z: 0.0, paved=[(-46, -22, 46, 18)], override=override,
                   paint=lambda x, z: "rock" if -31 < z < -22 else None)

    # ---- vegetation (lot 6.11): bushes and birches in the planters -----------------------------------
    m.set_vegetation(53, [("grass", "Bush", 8.0), ("grass", "Grass", 40.0), ("grass", "Birch", 4.0)],
                     symmetry="MirrorX")

    # ---- landscape beyond the bounds (lot 6.12): the sea behind the warehouses, hills elsewhere -----
    m.set_surroundings((-48, -38, 48, 26), "Sea", "Forest", "Slope", "Forest", "Mountains", 0.8, 19, sea_level=-1.0)
    return m


if __name__ == "__main__":
    import sys
    build().save(sys.argv[1])
