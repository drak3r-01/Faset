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

    # ---- helpers --------------------------------------------------------
    def _both(self, f):
        f(False)
        if self.sym:
            f(True)

    @staticmethod
    def _p(x, z, flip):
        return (-x, -z) if flip else (x, z)

    def _bounds(self, x0, z0, x1, z1, flip):
        if flip:
            x0, z0, x1, z1 = -x1, -z1, -x0, -z0
        return x0, z0, x1, z1

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
            r = OPP[rise] if flip else rise
            self.brushes.append(f'Wedge(min: ({g(a)}, {g(y0)}, {g(b)}), max: ({g(c)}, {g(y1)}, {g(d)}), rise: {r}, material: "{mat}")')
            self.solids.append((a, y0, b, c, y1, d))
        self._both(f)

    def stairs(self, x0, y0, z0, x1, y1, z1, rise, steps, mat):
        def f(flip):
            a, b, c, d = self._bounds(min(x0, x1), min(z0, z1), max(x0, x1), max(z0, z1), flip)
            r = OPP[rise] if flip else rise
            self.brushes.append(f'Stairs(min: ({g(a)}, {g(y0)}, {g(b)}), max: ({g(c)}, {g(y1)}, {g(d)}), rise: {r}, steps: {steps}, material: "{mat}")')
            self.solids.append((a, y0, b, c, y1, d))
        self._both(f)

    def cyl(self, x0, y0, z0, x1, y1, z1, sides, mat, axis="Y"):
        def f(flip):
            a, b, c, d = self._bounds(min(x0, x1), min(z0, z1), max(x0, x1), max(z0, z1), flip)
            self.brushes.append(f'Cylinder(min: ({g(a)}, {g(y0)}, {g(b)}), max: ({g(c)}, {g(y1)}, {g(d)}), axis: {axis}, sides: {sides}, material: "{mat}")')
            self.solids.append((a, y0, b, c, y1, d))
        self._both(f)

    def prism(self, pts, y0, y1, mat):
        """Convex polygon footprint, points counter-clockwise seen from above."""
        def f(flip):
            q = [self._p(x, z, flip) for x, z in pts]
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
            self.spawns.append(f"(at: ({px:.1f}, {y:.1f}, {pz:.1f}), yaw: {(yaw + (180 if flip else 0)) % 360:.0f}.0, team: {t})")
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
        out.append("    format_version: 1,")
        out.append(f'    meta: (name: "{self.name}", author: "Faset", description: "{self.description}", modes: ["fps"], players: ({self.players[0]}, {self.players[1]})),')
        out.append(f'    theme: (ambience: "{t["ambience"]}", sky: "{t.get("sky", "clear")}", weather: [{weather}], palette: "{t["palette"]}"),')
        if self.gravity:
            out.append(f"    physics: (gravity: {self.gravity}),")
        for key, items in (("brushes", self.brushes), ("spawns", self.spawns), ("pickups", self.pickups), ("zones", self.zones), ("lights", self.lights)):
            out.append(f"    {key}: [")
            out.extend(f"        {i}," for i in items)
            out.append("    ],")
        out.append(")")
        return "\n".join(out) + "\n"

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
