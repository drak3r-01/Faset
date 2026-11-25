# Map design guide

Rules for authors of Faset maps, human or session. Units are metres, seconds
and radians; the world is right-handed, Y up, -Z forward. Maps are RON files
(see `assets/testdata/example.ron`; the shipped maps in `assets/maps` are authored with `tools/map-scripts`).

## Dimensions to respect

| Dimension | Value |
|---|---|
| Corridor width | at least 3 m |
| Door | 2 m × 2.5 m |
| Low cover, high cover | 1.0 m and 2.0 m |
| Step climbable without jump | 0.45 m |
| Obstacle climbable by jumping | 1.0 m |
| Gap crossable while running | 4 m |
| Recommended maximum line of sight | 60 m |

Other guidance: give every team at least 8 spawns (16 players per match), keep
spawns out of long sightlines, and place health pickups away from spawns so
that fights are not decided at the spawn.

## Authoring loop

1. **Sheet**: theme, game mode, number of players, layout archetype (for
   example two bases and a centre, ring, vertical hub).
2. **ASCII grid schema**: sketch zones, passages and heights on a grid of
   0.5 m cells, one character per cell, before any RON.
3. **RON writing**: translate the schema into brushes, spawns, pickups and
   zones, using the dimensions above.
4. **Validation**: `map-tool validate <map.ron>`; fix every error.
5. **Views**: `map-tool view <map.ron> --top top.png --perspective p.png`:
   one top view and three perspective renders from different spots.
6. **Correction**: at most 3 rounds of steps 3 to 5.
7. **Bot trial**: `map-tool trial <map.ron> --seed 1 --out report.json`.
   16 bots play 5 minutes of match time with no rendering; the same map,
   mode and seed always give the same report. Read it as follows:
   - `elimination_zones`: where deaths pile up; a cell holding most of the
     deaths is a choke point or a sightline to fix;
   - `spawn_deaths.rate`: deaths within 6 m of a spawn; high means spawn
     camping, move spawns or add cover;
   - `mean_engagement_distance_m`: should match the intended play style;
   - `unvisited_zones`: dead space nobody walks into, remove or connect it;
   - `team_balance`: kills and living time per team, they should be close.
8. **Human validation** by Gabriel, then the map joins the catalogue.

## Setting and borders

A map has a theme (ambience, sky, weather, palette) that the client really
uses, and its edge must be part of the scene, not a bare wall: water (a
`KillVolume` under the sea level plus a `Water` zone whose top is the sea
level), cliffs or mountains in rock materials, buildings, breakwater rocks.
Prefer the pixel materials of `mapfmt::geometry::MATERIAL_NAMES` (`brick`,
`wood`, `grass`, `sandstone`, `basalt`, `ice`, `lagoon`, `lamp_warm`...) and
name every light of a night map in `lights`. Level changes need stairs or ramps
(a rise of at most 0.45 m per step or per 0.5 m of run); a step of 0.5 m or more
is only reachable by jumping and counts as unreachable for the validator.

## Props and vegetation

Props (`props`, recipes of `assets/props`) are drawn by the client standing on
their position. Vegetation (format version 3) is an optional block:

```ron
vegetation: (seed: 61, rules: [(on: "grass", species: Grass, density: 45.0)],
             plants: [(species: Oak, at: (3.0, 0.0, -4.0), yaw: 30.0)],
             symmetry: Rotate180),
```

- `rules` scatter `density` plants per 100 m^2 (at most 400) on every flat
  top of a box or prism brush of material `on`, or on terrain samples of that
  ground (`grass`, `sand`, `rock`, `snow`, `dirt`; slopes over 35 degrees are
  `rock`); `terrain:<ground>` matches the terrain only. Plants never grow under
  a brush, in water, on slopes too steep for the species or within 1.5 m of a
  spawn or pickup. The same seed always gives the same plants.
- Species: `Oak`, `Pine`, `Birch`, `Palm`, `Bush`, `Rock`, `Grass`, `Fern`,
  coloured by the map palette (`snow`, `desert`, else `nature`).
- Plants have no collision: players and bullets go through them. Trees,
  bushes and rocks block the bots' line of sight; grass and ferns do not.
- Maps built as two half-turn halves set `symmetry: Rotate180` so both teams
  get the same cover (a map that declares its own `symmetry` uses that one).
- Keep dense grass (tens per 100 m^2) to areas that need it: thousands of
  plants are cheap to draw, but bushes and trees change sightlines.

## Landscape beyond the bounds

The world continues past the playable area (format version 4, optional
`surroundings` block). It is scenery only: the walls and `KillVolume`s of the
map still hold the players, and no collision is built for the far terrain.

```ron
surroundings: (area: (-64.0, -48.0, 64.0, 48.0), sea_level: Some(-1.0),
               north: Slope, east: Cliff, south: Slope, west: Forest,
               horizon: Mountains, haze: 0.3, seed: 7),
```

- `area` is the playable rectangle in metres `(min_x, min_z, max_x, max_z)`
  (every spawn and pickup must be inside, and one `KillVolume` should span
  it); `ground` is the floor height at its edge (default 0); `sea_level`
  gives the sea of a coast that has no `Water` zone.
- Each side (`north` is -Z, `east` +X, `south` +Z, `west` -X) is a `Border`:
  `Forest` (hills and trees), `Cliff` (a drop to a beach or a low plain),
  `Sea`, `Slope` (steep climb), `Dunes` or `Rocks`. Pick borders that make
  leaving the area look like a bad idea: no visible wall is needed.
- `horizon` is the far silhouette: `Flat` (default), `Mountains`, `Treeline`
  or `Dunes`. `haze` scales the fog density of the ambience (default 1; use
  less than 1 under a thick fog such as `snow` so the silhouette shows).
- The far terrain is generated at load time from this block and `seed`
  (about 3 000 vertices); forest borders add up to 2 400 instanced trees.

## World (format version 5)

A map can sit in a 1 to 2 km world (optional `world` block, lot 8.1) while
only its playable rectangle is simulated:

```ron
world: (extent: (-512.0, -512.0, 512.0, 512.0),
        playable: (-64.0, -48.0, 64.0, 48.0),
        far: (cell: 8, size: (129, 129), heights: "...", paint: "..."),
        snow_line: Some(140.0)),
```

- `extent` is the world rectangle in metres; the far field covers it exactly
  (`(size - 1) * cell` metres per axis, at most 4 096 m and 1 025 samples).
  `playable` is the simulated rectangle, the same as the `surroundings` area.
- The sculpted ground of the playable zone stays the `terrain` block: it is
  the only ground the server, the client prediction and the bots collide
  with. The far field (heights in decimetres, a varint stream in base64) is
  scenery: the client draws the whole world as streamed chunks with levels of
  detail, the terrain on its own grid and the far field around it, blended
  over 16 m from the terrain's border. Keep the walls and `KillVolume`s that
  hold the players; a map with a world needs no landscape mesh (its
  `surroundings` still give the woods, the haze and the sea).
- Ground layers: the palette ground, then dirt, sand, snow and rock from the
  paint (`paint` of the far field, run-length coded; the terrain's own
  paint), sand on the shore of `sea_level`, snow above `snow_line`, cliff
  rock blending in from 28 to 46 degrees of slope.
- Let the far field rise into a rim near the edge of the extent so the end of
  the world never shows. `Map.set_world` in `tools/map-scripts/maplib.py`
  writes the block from a height function (Pine Hollow is the example).

## Layout (lot 6.13)

Plans of the lot 6.13 maps are in `docs/maps/` (Mossgate, Greyfold Quarry,
Nightjar Market): start a new map with such a plan before any script.

- **Shape:** avoid a square box. Pick an archetype that gives the map its
  silhouette: an L (two arms meeting at an elbow), a loop around an obstacle
  (pit, lake, block of houses), or lanes around a central building. Fill the
  rest of the rectangle with scenery the players cannot reach (water, quarry
  faces, rows of tall houses).
- **Three lanes:** an outer flank (longer, quieter), a middle lane (the
  direct way) and an inner lane (closest to the centre, most exposed), joined
  by cross links (alleys, walk-through houses) every 10 to 15 m.
- **Contested centre:** put the place where the lanes meet on the shortest
  route between the spawns. A route clearly shorter than the others (a
  bridge across a pond, for example) takes every fight and leaves the rest
  of the map empty: keep the alternatives within about 10 % of each other.
- **Fairness and readability:** competitive maps keep a symmetry
  (`Rotate180`, `MirrorX`, `MirrorZ`, or the diagonal mirror x = z that only
  the scripts can write) and break it in what does not change the balance:
  landmarks, one-off props, the side each building faces. Each half needs at
  least one landmark of its own so players know where they are.
- **Sightlines:** no open axis longer than about 40 m through the middle of
  the map; break streets with a kiosk, a cart or a monument. Windows are
  holes 1.0 to 2.0 m above the floor (low cover under them).
- **Floors:** stairs rise at most 0.44 m per 0.5 m of run; the slab above is
  open over the whole flight (2 m wide) and the top step lands sideways onto
  the floor. Doors are 2 m by 2.5 m and never under a flight.
- **Spawns:** a walled yard at each end, two or three exits, no sightline
  from a lane into the yard, enemy spawns more than 60 m apart.
- **Deadly water and pits:** line them with low walls (1 m, jumpable on
  purpose), with gaps only at bridges, or bots and players fall in while
  dodging. `map-tool trial` does not model kill volumes: a bot that falls
  keeps fighting at the bottom and skews the report.

### Trial targets (decisions of lot 6.13)

Run `tools/map-scripts/trial_table.py` (seeds 1 to 3, 16 bots, team mode)
and aim for, on every seed:

| Figure | Target |
|---|---|
| Deaths within 6 m of a spawn | under 10 % |
| Gap between the kills of the two teams | 15 % of the larger at most |
| Dead zones: 4 m cells of the playable area no bot entered | 25 % at most (walls, roofs and spawn buildings count, so 0 is not reachable) |
| Mean engagement distance | 6 to 20 m, set per map in its plan (6 to 12 m for close quarters) |

A map script declares `play_area(x, z)` so that the table counts only the
cells players are meant to walk on.

### Script helpers

`tools/map-scripts/maplib.py` writes one half and its copy: `Map.sym` is
`True` (half turn), `"mirx"` (mirror across x = 0) or `"diag"` (mirror across
x = z). `wall` cuts doors and windows into a wall, `house` builds a house of
one or more floors with its openings, stairs and roof, `Map.auto_terrain`
lays a height field that stays flat under everything built and `Map.plant`
places trees on both halves. The seven shipped maps are rebuilt with
`docker compose run --rm dev python3 tools/map-scripts/build.py`.
