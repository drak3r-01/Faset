"""Tiny builder for Faset map files (RON). Coordinates in metres, y up.

`sym=True` emits every element twice, the second one turned 180 degrees about
the vertical axis through the origin, so one half is enough for a fair map.
"""
import math, random

OPP = {"PosX": "NegX", "NegX": "PosX", "PosZ": "NegZ", "NegZ": "PosZ"}


import math as _m


def g(v):
    return int(_m.floor(v * 2 + 0.5))


def rg(a, b):
    """Grid interval of [a, b] metres, at least one unit wide."""
    ia, ib = g(a), g(b)
    if ib <= ia:
        ib = ia + 1
    return ia, ib


class Map:
    def __init__(self, name, description, players, theme, header, gravity=None):
        self.name, self.description, self.players = name, description, players
        self.theme, self.header, self.gravity = theme, header, gravity
        self.sym = False
        self.brushes, self.spawns, self.pickups, self.zones, self.lights = [], [], [], [], []
        self.solids = []  # (x0, y0, z0, x1, y1, z1) of every box-like brush, for placement checks
        self.terrain = None  # RON of the height field, see `set_terrain`
        self.vegetation = None  # RON of the vegetation block, see `set_vegetation`
        self.surroundings = None  # RON of the landscape beyond the bounds, see `set_surroundings`
        self.track_prisms = False  # record prism bounding boxes in `solids` (lot 6.13 maps)

    # ---- helpers --------------------------------------------------------
    # `sym` is False (one copy), True or "rot" (half turn about the origin),
    # "mirx" (mirror across the plane x = 0) or "diag" (mirror across the
    # vertical plane x = z, which swaps the two axes).
    def _both(self, f):
        f(False)
        if self.sym:
            f(True)

    def _mode(self):
        return "rot" if self.sym is True else self.sym

    def _p(self, x, z, flip):
        if not flip:
            return (x, z)
        mode = self._mode()
        if mode == "mirx":
            return (-x, z)
        if mode == "diag":
            return (z, x)
        return (-x, -z)

    def _bounds(self, x0, z0, x1, z1, flip):
        if not flip:
            return x0, z0, x1, z1
        mode = self._mode()
        if mode == "mirx":
            return -x1, z0, -x0, z1
        if mode == "diag":
            return z0, x0, z1, x1
        return -x1, -z1, -x0, -z0

    def _rise(self, rise, flip):
        if not flip:
            return rise
        mode = self._mode()
        if mode == "mirx":
            return {"PosX": "NegX", "NegX": "PosX"}.get(rise, rise)
        if mode == "diag":
            return {"PosX": "PosZ", "PosZ": "PosX", "NegX": "NegZ", "NegZ": "NegX"}[rise]
        return OPP[rise]

    def _yaw(self, yaw, flip):
        if not flip:
            return yaw % 360
        mode = self._mode()
        if mode == "mirx":
            return (-yaw) % 360
        if mode == "diag":
            return (90 - yaw) % 360
        return (yaw + 180) % 360

    # ---- brushes ----------------------------------------------------------
    def box(self, x0, y0, z0, x1, y1, z1, mat):
        def f(flip):
            a, b, c, d = self._bounds(min(x0, x1), min(z0, z1), max(x0, x1), max(z0, z1), flip)
            (ia, ic), (iy0, iy1), (ib, id_) = rg(a, c), rg(y0, y1), rg(b, d)
            self.brushes.append(f'Box(min: ({ia}, {iy0}, {ib}), max: ({ic}, {iy1}, {id_}), material: "{mat}")')
            self.solids.append((ia / 2, iy0 / 2, ib / 2, ic / 2, iy1 / 2, id_ / 2))
        self._both(f)

    def wedge(self, x0, y0, z0, x1, y1, z1, rise, mat):
        def f(flip):
            a, b, c, d = self._bounds(min(x0, x1), min(z0, z1), max(x0, x1), max(z0, z1), flip)
            r = self._rise(rise, flip)
            self.brushes.append(f'Wedge(min: ({g(a)}, {g(y0)}, {g(b)}), max: ({g(c)}, {g(y1)}, {g(d)}), rise: {r}, material: "{mat}")')
            self.solids.append((a, y0, b, c, y1, d))
        self._both(f)

    def stairs(self, x0, y0, z0, x1, y1, z1, rise, steps, mat):
        def f(flip):
            a, b, c, d = self._bounds(min(x0, x1), min(z0, z1), max(x0, x1), max(z0, z1), flip)
            r = self._rise(rise, flip)
            self.brushes.append(f'Stairs(min: ({g(a)}, {g(y0)}, {g(b)}), max: ({g(c)}, {g(y1)}, {g(d)}), rise: {r}, steps: {steps}, material: "{mat}")')
            self.solids.append((a, y0, b, c, y1, d))
        self._both(f)

    def cyl(self, x0, y0, z0, x1, y1, z1, sides, mat, axis="Y"):
        def f(flip):
            a, b, c, d = self._bounds(min(x0, x1), min(z0, z1), max(x0, x1), max(z0, z1), flip)
            ax = {"X": "Z", "Z": "X"}.get(axis, axis) if flip and self._mode() == "diag" else axis
            self.brushes.append(f'Cylinder(min: ({g(a)}, {g(y0)}, {g(b)}), max: ({g(c)}, {g(y1)}, {g(d)}), axis: {ax}, sides: {sides}, material: "{mat}")')
            self.solids.append((a, y0, b, c, y1, d))
        self._both(f)

    def prism(self, pts, y0, y1, mat):
        """Convex polygon footprint, points counter-clockwise seen from above."""
        def f(flip):
            q = [self._p(x, z, flip) for x, z in pts]
            # counter-clockwise seen from above (north = -z up): a mirror or a
            # hand-written outline may come clockwise
            area = sum(q[i][0] * -q[(i + 1) % len(q)][1] - q[(i + 1) % len(q)][0] * -q[i][1] for i in range(len(q)))
            if area < 0:
                q.reverse()
            if self.track_prisms:
                xs, zs = [p[0] for p in q], [p[1] for p in q]
                self.solids.append((min(xs), y0, min(zs), max(xs), y1, max(zs)))
            s = ", ".join(f"({g(x)}, {g(z)})" for x, z in q)
            self.brushes.append(f'Prism(points: [{s}], y_min: {g(y0)}, y_max: {g(y1)}, material: "{mat}")')
        self._both(f)

    def disc(self, cx, cz, r, y0, y1, mat, n=16, rot=0.0, sx=1.0, sz=1.0):
        pts = []
        for i in range(n):
            a = rot + 2 * math.pi * i / n
            pts.append((cx + r * sx * math.cos(a), cz - r * sz * math.sin(a)))
        # de-duplicate points that collapse on the half-metre grid
        seen, out = set(), []
        for p in pts:
            k = (g(p[0]), g(p[1]))
            if k not in seen:
                seen.add(k)
                out.append(p)
        self.prism(out, y0, y1, mat)

    # ---- gameplay ---------------------------------------------------------
    def spawn(self, x, y, z, yaw, team):
        def f(flip):
            px, pz = self._p(x, z, flip)
            t = team
            if flip and team in ("A", "B"):
                t = "B" if team == "A" else "A"
            self.spawns.append(f"(at: ({px:.1f}, {y:.1f}, {pz:.1f}), yaw: {self._yaw(yaw, flip):.0f}.0, team: {t})")
        self._both(f)

    def pickup(self, kind, x, y, z, respawn):
        def f(flip):
            px, pz = self._p(x, z, flip)
            self.pickups.append(f"(kind: {kind}, at: ({px:.1f}, {y:.1f}, {pz:.1f}), respawn_s: {respawn})")
        self._both(f)

    def zone(self, x0, y0, z0, x1, y1, z1, kind):
        def f(flip):
            a, b, c, d = self._bounds(min(x0, x1), min(z0, z1), max(x0, x1), max(z0, z1), flip)
            self.zones.append(f"Box(min: ({g(a)}, {g(y0)}, {g(b)}), max: ({g(c)}, {g(y1)}, {g(d)}), kind: {kind})")
        self._both(f)

    def light(self, x, y, z, color, intensity, rng_m):
        def f(flip):
            px, pz = self._p(x, z, flip)
            self.lights.append(f"(at: ({px:.1f}, {y:.1f}, {pz:.1f}), color: ({color[0]}, {color[1]}, {color[2]}), intensity: {intensity}, range: {rng_m})")
        self._both(f)

    def free(self, x, z, y=0.0, margin=0.8, head=2.2):
        """True when a standing player at (x, y, z) clears every recorded solid."""
        for (a, b, c, d, e, f_) in self.solids:
            if e <= y + 0.05 or b >= y + head:
                continue
            if a - margin < x < d + margin and c - margin < z < f_ + margin:
                return False
        return True

    def find_spots(self, inside, n, y=0.0, gap=3.0, step=1.0, margin=0.9, near=None, seed=0):
        """Free ground spots (x, z) accepted by `inside`, at least `gap` apart."""
        import random as _r
        cands = []
        x = -60.0
        while x <= 60:
            z = -60.0
            while z <= 60:
                if inside(x, z) and self.free(x, z, y, margin):
                    cands.append((x, z))
                z += step
            x += step
        _r.Random(seed).shuffle(cands)
        if near:
            cands.sort(key=lambda p: (p[0] - near[0]) ** 2 + (p[1] - near[1]) ** 2)
        out = []
        for p in cands:
            if all((p[0] - q[0]) ** 2 + (p[1] - q[1]) ** 2 >= gap * gap for q in out):
                out.append(p)
            if len(out) == n:
                break
        return out

    # ---- output -------------------------------------------------------------
    def ron(self):
        t = self.theme
        weather = ", ".join(f'(from_s: {a}, weather: "{b}")' for a, b in t.get("weather", [(0.0, "clear")]))
        out = [self.header.rstrip(), "Map("]
        out.append(f"    format_version: {4 if self.surroundings else 3},")
        out.append(f'    meta: (name: "{self.name}", author: "Faset", description: "{self.description}", modes: ["fps"], players: ({self.players[0]}, {self.players[1]})),')
        out.append(f'    theme: (ambience: "{t["ambience"]}", sky: "{t.get("sky", "clear")}", weather: [{weather}], palette: "{t["palette"]}"),')
        if self.gravity:
            out.append(f"    physics: (gravity: {self.gravity}),")
        for key, items in (("brushes", self.brushes), ("spawns", self.spawns), ("pickups", self.pickups), ("zones", self.zones), ("lights", self.lights)):
            out.append(f"    {key}: [")
            out.extend(f"        {i}," for i in items)
            out.append("    ],")
        if self.terrain:
            out.append(f"    terrain: {self.terrain},")
        if self.vegetation:
            out.append(f"    vegetation: {self.vegetation},")
        if self.surroundings:
            out.append(f"    surroundings: {self.surroundings},")
        out.append(")")
        return "\n".join(out) + "\n"

    # ---- terrain (lot 6.10) ---------------------------------------------------
    def set_terrain(self, x0, z0, cell, nx, nz, height, paint=None):
        """Height field: `nx` x `nz` samples `cell` metres apart from (x0, z0),
        `height(x, z)` in metres (stored in centimetres, base64 of i16 LE)."""
        import base64, struct
        cms = []
        for j in range(nz):
            for i in range(nx):
                cms.append(int(math.floor(height(x0 + i * cell, z0 + j * cell) * 100 + 0.5)))
        data = base64.b64encode(struct.pack("<%dh" % len(cms), *cms)).decode()
        cell_units = g(cell)
        cell_field = "" if cell_units == 4 else f"cell: {cell_units}, "
        paint_field = ""
        if paint:
            names = ["auto", "grass", "sand", "rock", "snow", "dirt"]
            bs = bytes(names.index(paint(x0 + i * cell, z0 + j * cell) or "auto") for j in range(nz) for i in range(nx))
            if any(bs):
                paint_field = f', paint: "{base64.b64encode(bs).decode()}"'
        self.terrain = f'(origin: ({g(x0)}, {g(z0)}), {cell_field}size: ({nx}, {nz}), heights: "{data}"{paint_field})'
        return cms

    def auto_terrain(self, x0, z0, x1, z1, cell, raw, paved=(), paved_h=-0.2, override=None, paint=None,
                     blend=(1.0, 5.0)):
        """Terrain over [x0, x1] x [z0, z1] (lot 6.13 maps): `raw(x, z)` far
        from everything, flat at y = 0 around every solid standing on the
        ground, every spawn and every pickup (blending over `blend` metres),
        `paved_h` under the `paved` rectangles (hidden by their paving), and
        `override(x, z)` wherever it returns a height. Give a symmetric `raw`
        for a symmetric map: the solids are already recorded in both halves."""
        import re
        rects = []
        for (a, b, c, d, e, f_) in self.solids:
            if b <= 0.1 and e >= -0.05:
                rects.append((a, c, d, f_))
        for s in self.spawns + self.pickups:
            x, y, z = (float(v) for v in re.search(r"at: \(([-\d.]+), ([-\d.]+), ([-\d.]+)\)", s).groups())
            if y < 1.0:
                rects.append((x - 0.6, z - 0.6, x + 0.6, z + 0.6))
        # paving and floors (solids whose top is the ground level) hide the terrain under them
        paved = list(paved) + [(a, c, d, f_) for (a, b, c, d, e, f_) in self.solids
                               if abs(e) < 0.01 and b < 0 and (d - a) >= 1.0 and (f_ - c) >= 1.0]
        rects += paved

        def clearance(x, z):
            best = 1e9
            for (a, c, d, f_) in rects:
                dx = max(a - x, 0.0, x - d)
                dz = max(c - z, 0.0, z - f_)
                if dx < best and dz < best:
                    best = min(best, math.hypot(dx, dz))
            return best

        def height(x, z):
            if override:
                h = override(x, z)
                if h is not None:
                    return h
            for (a, c, d, f_) in paved:
                if a + 0.3 <= x <= d - 0.3 and c + 0.3 <= z <= f_ - 0.3:
                    return paved_h
            t = min(max((clearance(x, z) - blend[0]) / (blend[1] - blend[0]), 0.0), 1.0)
            return t * t * (3 - 2 * t) * raw(x, z)

        nx = int(round((x1 - x0) / cell)) + 1
        nz = int(round((z1 - z0) / cell)) + 1
        return self.set_terrain(x0, z0, cell, nx, nz, height, paint)

    # ---- vegetation (lot 6.11) --------------------------------------------------
    def set_vegetation(self, seed, rules, plants=(), symmetry="Rotate180"):
        """Density rules `(ground, species, plants per 100 m^2)`, scattered by
        the game from `seed`, and hand-placed `plants` `(species, x, y, z,
        yaw)`. The maps of lot 6.8 are built as two half-turn halves, so the
        rules scatter with the same symmetry (both teams get the same cover);
        a map with a symmetry the format cannot express (the diagonal mirror)
        passes `symmetry=None` and places its blocking plants by hand."""
        rs = ", ".join(f'(on: "{on}", species: {sp}, density: {d:.1f})' for on, sp, d in rules)
        out = f"(seed: {seed}, rules: [{rs}]"
        if plants:
            ps = ", ".join(f"(species: {sp}, at: ({x:.1f}, {y:.1f}, {z:.1f}), yaw: {yaw:.0f}.0)" for sp, x, y, z, yaw in plants)
            out += f", plants: [{ps}]"
        if symmetry:
            out += f", symmetry: {symmetry}"
        self.vegetation = out + ")"

    def plant(self, species, x, z, y=0.0, yaw=0.0):
        """Hand-placed plants at (x, z) and its `sym` copy, for `set_vegetation`."""
        out = []

        def f(flip):
            px, pz = self._p(x, z, flip)
            out.append((species, px, y, pz, self._yaw(yaw, flip)))
        self._both(f)
        return out

    # ---- surroundings (lot 6.12) -----------------------------------------------------
    def set_surroundings(self, area, north, east, south, west, horizon="Flat", haze=1.0, seed=1,
                         ground=None, sea_level=None):
        """The landscape beyond the playable rectangle `area` (min_x, min_z, max_x, max_z)."""
        out = f"(area: ({area[0]:.1f}, {area[1]:.1f}, {area[2]:.1f}, {area[3]:.1f})"
        if ground is not None:
            out += f", ground: {ground:.1f}"
        if sea_level is not None:
            out += f", sea_level: Some({sea_level:.1f})"
        out += f", north: {north}, east: {east}, south: {south}, west: {west}, horizon: {horizon}, haze: {haze}, seed: {seed})"
        self.surroundings = out

    def save(self, path):
        with open(path, "w", newline="\n") as fh:
            fh.write(self.ron())


# ---- reusable scenery -----------------------------------------------------------

def palm(m, x, z, y=0.0, h=5.0, leaves="palm_leaves"):
    m.box(x - 0.25, y, z - 0.25, x + 0.25, y + h, z + 0.25, "wood")
    top = y + h
    m.box(x - 2.5, top, z - 0.5, x + 2.5, top + 0.5, z + 0.5, leaves)
    m.box(x - 0.5, top, z - 2.5, x + 0.5, top + 0.5, z + 2.5, leaves)
    m.box(x - 1.5, top + 0.5, z - 1.5, x + 1.5, top + 1.0, z + 1.5, leaves)
    for dx, dz in ((3.0, 0), (-3.0, 0), (0, 3.0), (0, -3.0)):
        m.box(x + dx - 0.5, top - 0.5, z + dz - 0.5, x + dx + 0.5, top, z + dz + 0.5, leaves)
    m.box(x - 0.5, top - 0.5, z - 0.5, x + 0.5, top, z + 0.5, "wood_dark")


def pine(m, x, z, y=0.0, h=7.0, leaves="leaves", snow=False):
    m.box(x - 0.25, y, z - 0.25, x + 0.25, y + 2.5, z + 0.25, "wood_dark")
    tiers = [(2.5, 2.0), (4.0, 1.5), (5.5, 1.0), (6.5, 0.5)]
    for i, (yy, r) in enumerate(tiers):
        if yy > h:
            break
        m.box(x - r, y + yy, z - r, x + r, y + yy + 1.5, z + r, leaves)
        if snow:
            m.box(x - r, y + yy + 1.5, z - r, x + r, y + yy + 2.0, z + r, "snow")
    m.box(x - 0.25, y + min(h, 7.0), z - 0.25, x + 0.25, y + min(h, 7.0) + 1.0, z + 0.25, leaves)


def rock(m, x, z, y=0.0, size=2.0, mat="basalt", seed=0):
    r = random.Random(seed)
    w = size * (0.8 + 0.4 * r.random())
    d = size * (0.8 + 0.4 * r.random())
    h = size * (0.7 + 0.5 * r.random())
    m.box(x - w / 2, y, z - d / 2, x + w / 2, y + h, z + d / 2, mat)
    m.box(x - w / 4, y + h, z - d / 4, x + w / 4, y + h + size * 0.25, z + d / 4, mat)


def crate(m, x, z, y=0.0, s=1.0, mat="crate"):
    m.box(x - s / 2, y, z - s / 2, x + s / 2, y + s, z + s / 2, mat)


# ---- town pieces (lot 6.13) -----------------------------------------------------

def wall(m, x0, z0, x1, z1, y0, y1, mat, holes=()):
    """A wall box with openings. `holes` are `(u0, u1, v0, v1)`: from u0 to
    u1 along the long side of the wall (absolute x or z), from height v0 to
    v1. Keep every value on the half-metre grid."""
    along_x = (x1 - x0) >= (z1 - z0)
    a0, a1 = (x0, x1) if along_x else (z0, z1)

    def piece(u0, u1, v0, v1):
        if u1 - u0 < 0.01 or v1 - v0 < 0.01:
            return
        if along_x:
            m.box(u0, v0, z0, u1, v1, z1, mat)
        else:
            m.box(x0, v0, u0, x1, v1, u1, mat)

    # cut the wall at every hole edge; each slice keeps the solid between its holes
    cuts = sorted({a0, a1} | {u for h in holes for u in h[:2] if a0 < u < a1})
    for u0, u1 in zip(cuts, cuts[1:]):
        spans = sorted((v0, v1) for (h0, h1, v0, v1) in holes if h0 <= u0 and u1 <= h1)
        y = y0
        for v0, v1 in spans:
            piece(u0, u1, y, v0)
            y = max(y, v1)
        piece(u0, u1, y, y1)


def slab(m, x0, z0, x1, z1, y0, y1, mat, hole=None):
    """A floor slab, with an optional rectangular `hole` (x0, z0, x1, z1) for a stairwell."""
    if not hole:
        m.box(x0, y0, z0, x1, y1, z1, mat)
        return
    hx0, hz0, hx1, hz1 = hole
    for (a, b, c, d) in ((x0, z0, x1, hz0), (x0, hz1, x1, z1), (x0, hz0, hx0, hz1), (hx1, hz0, x1, hz1)):
        if c - a > 0.01 and d - b > 0.01:
            m.box(a, y0, b, c, y1, d, mat)


DOOR_H = 2.5
WIN = (1.0, 2.0)  # window sill (low cover) and lintel above a floor


def house(m, x0, z0, x1, z1, mat, storeys=1, h=3.5, openings=(), stair=None, roof="brick_slate",
          floor="wood", trim="wood_dark", gable=True, roof_h=2.0):
    """A house of `storeys` floors `h` apart. `openings` are `(side, u0, u1,
    kind, storey)`: side N (z = z0), S (z = z1), W (x = x0) or E (x = x1),
    kind "door" or "window". `stair` is `(x0, z0, x1, z1, rise)` (one flight
    from the ground to the first floor, the slab is open above it)."""
    t = 0.5
    top = storeys * h
    if floor:
        m.box(x0, -0.4, z0, x1, 0.0, z1, floor)
    holes = {"N": [], "S": [], "W": [], "E": []}
    for side, u0, u1, kind, k in openings:
        base = k * h
        if kind == "door":
            holes[side].append((u0, u1, base, base + DOOR_H))
        else:
            holes[side].append((u0, u1, base + WIN[0], base + WIN[1]))
    wall(m, x0, z0, x1, z0 + t, -0.4, top, mat, holes["N"])
    wall(m, x0, z1 - t, x1, z1, -0.4, top, mat, holes["S"])
    wall(m, x0, z0 + t, x0 + t, z1 - t, -0.4, top, mat, holes["W"])
    wall(m, x1 - t, z0 + t, x1, z1 - t, -0.4, top, mat, holes["E"])
    for k in range(1, storeys):
        hole = stair[:4] if stair and k == 1 else None
        slab(m, x0 + t, z0 + t, x1 - t, z1 - t, k * h - 0.5, k * h, floor or "wood", hole)
    if stair:
        sx0, sz0, sx1, sz1, rise = stair
        steps = max(2, int(math.ceil(h / 0.44)))
        m.stairs(sx0, 0.0, sz0, sx1, h, sz1, rise, steps, "wood_dark")
    # roof: a slab and a gable along the long side
    m.box(x0 - 0.5, top, z0 - 0.5, x1 + 0.5, top + 0.5, z1 + 0.5, trim)
    if gable:
        if x1 - x0 >= z1 - z0:
            zm = (z0 + z1) / 2
            m.wedge(x0 - 0.5, top + 0.5, z0 - 0.5, x1 + 0.5, top + 0.5 + roof_h, zm, "PosZ", roof)
            m.wedge(x0 - 0.5, top + 0.5, zm, x1 + 0.5, top + 0.5 + roof_h, z1 + 0.5, "NegZ", roof)
        else:
            xm = (x0 + x1) / 2
            m.wedge(x0 - 0.5, top + 0.5, z0 - 0.5, xm, top + 0.5 + roof_h, z1 + 0.5, "PosX", roof)
            m.wedge(xm, top + 0.5, z0 - 0.5, x1 + 0.5, top + 0.5 + roof_h, z1 + 0.5, "NegX", roof)
