# Lot index

One file per lot, each self-contained for the session that executes it. One lot = one branch (`lot/X.Y-slug`) = one pull request. Read `CLAUDE.md` and the lot file only.

Milestones: J0 after 0.4 · J1 after 1.8 · J2 after 2.10 · J3 after 4.2 · J4 after 4.10 · J5 after 5.6 · J6 after 6.5 · J7 after 7.3 (`v0.1.0`).

Model/effort of each lot is in the header of its file. Parallel threads after phase 0: A network (1.x), B rendering (2.x), C maps (3.1); they join at 3.2. Content lots 2.14, 4.6 (data), 5.6, 6.5 (texts) can run beside code lots. 2.11 and 2.12 block nothing and may be postponed.

Internal lot outside the plan: 4.11, the phase-4 integration through the real server ([4.11-integration.md](4.11-integration.md)).

Internal lot outside the plan: 6.6, the playable client (controls, HUD, scene, map transfer, settings read; [6.6-playable-client.md](6.6-playable-client.md)).

Internal lot outside the plan: 6.7, the art direction (pixel materials, weapon models, light; [6.7-art-direction.md](6.7-art-direction.md)).

Internal lot outside the plan: 6.8, the map redesign (four themed maps with natural borders, the map's look in the client; [6.8-map-redesign.md](6.8-map-redesign.md)).

Internal lots outside the plan, run before phase 7 (series 6.9 to 6.13, one PR each):
- 6.9 shooting feel (Sonnet high) - [6.9-shooting-feel.md](6.9-shooting-feel.md)
- 6.10 terrain with relief (Opus high) - [6.10-terrain-relief.md](6.10-terrain-relief.md)
- 6.11 props and vegetation (Opus high) - [6.11-props-vegetation.md](6.11-props-vegetation.md)
- 6.12 landscape beyond bounds (Sonnet high) - [6.12-landscape-beyond-bounds.md](6.12-landscape-beyond-bounds.md)
- 6.13 level design (Opus high) - [6.13-level-design.md](6.13-level-design.md)

Items the plan leaves unspecified are marked as decisions inside each file: record the chosen value in your report.

Synthèse de tous les lots (numéro, titre, résumé, modèle, statut) : [OVERVIEW.md](OVERVIEW.md)
