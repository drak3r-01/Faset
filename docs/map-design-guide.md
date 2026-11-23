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
