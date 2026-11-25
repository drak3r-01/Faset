# Lots overview

Tableau de synthèse des 58 lots du plan, plus les lots internes (4.11, 6.6 à 6.13) et la phase 8 « monde » (8.1 à 8.12). Modèle et effort sont repris de l'en-tête de chaque fichier `docs/lots/<lot>-*.md`. Statut au moment de la rédaction : fusionnés 0.1-0.4, 1.1-1.4, 2.1-2.2 ; en cours 1.5 et 2.3 ; les autres sont à faire.

| Lot | Titre | Résumé | Modèle / effort | Statut |
|---|---|---|---|---|
| 0.1 | Lire CLAUDE.md, Graphify, corriger le plan | Vérifier les règles du projet et corriger les sections 1 et 2 du plan de référence. | Opus, medium | Fusionné |
| 0.2 | Image Docker de dev, scripts hôte, squelette de workspace | Faire passer `cargo xtask ci` sur un workspace vide, entièrement dans Docker. | Sonnet, medium | Fusionné |
| 0.3 | Cross-targets, image lavapipe, `build` et `package` | Compiler les binaires placeholder pour les cibles Docker et packager avec sommes de contrôle. | Sonnet, medium | Fusionné |
| 0.4 | Pipeline de release par tag, build macOS, image `server-runtime` | Produire une release complète à partir d'un tag de version. | Sonnet, medium | Fusionné |
| 1.1 | Core et protocole : types, codec binaire, quantification | Fournir les types de base et un format de fil bit-exact, validé par tests de round-trip. | Sonnet, medium | Fusionné |
| 1.2 | Sim : monde ECS, tick fixe, ordre des systèmes, collisions de base | Rendre la simulation déterministe, rejouable à l'identique depuis une trace d'entrées. | Opus, medium | À faire |
| 1.3 | Net : transport, UDP, mémoire, simulateur de conditions, canaux | Fournir l'abstraction `Transport` et quatre canaux fiables ou non fiables, testés à 10 % de perte. | Sonnet, high | Fusionné |
| 1.4 | Serveur : connexion, tampon de commandes, tick, snapshots complets | Accepter des clients et envoyer des snapshots complets pour que deux clients se voient. | Sonnet, medium | Fusionné |
| 1.5 | Snapshots delta, accusés de réception, images de référence | Encoder les snapshots en delta par rapport au dernier acquitté, avec 64 images de référence. | Opus, medium | En cours |
| 1.6 | Synchro d'horloge, prédiction et réconciliation côté client | Prédire le mouvement local, réconcilier avec les snapshots et lisser les corrections sans téléportation. | Opus, high | À faire |
| 1.7 | Interpolation distante et compensation de latence | Afficher les entités distantes avec un retard délibéré et rembobiner les hitboxes côté serveur (250 ms max). | Opus, medium | À faire |
| 1.8 | Serveur dédié : configuration, console, arrêt propre, Docker | Livrer l'exécutable `dedicated` headless, configuré par `server.toml`, dans l'image `server-runtime`. | Sonnet, medium | À faire |
| 1.9 | Serveur intégré, ouverture LAN et découverte | Héberger une partie dans le client, l'ouvrir sur le LAN et la découvrir par broadcast UDP. | Sonnet, medium | À faire |
| 1.10 | Endurance et robustesse réseau | Prouver que la pile réseau résiste aux entrées malformées et aux longues sessions sans fuite. | Sonnet, medium | À faire |
| 2.1 | Instance Vulkan, swapchain, boucle de frames, cible hors écran | Créer le device Vulkan 1.2, la boucle de frames fenêtrée et une cible hors écran lue en PNG. | Opus, medium | Fusionné |
| 2.2 | Maillages, instances, caméra reverse-Z, jitter, vecteurs de mouvement, culling | Rendre des maillages instanciés avec projection reverse-Z infinie, jitter et culling frustum. | Opus, medium | Fusionné |
| 2.3 | Chaîne de shaders WGSL, rechargement à chaud, matériaux procéduraux | Compiler les shaders WGSL en SPIR-V et choisir les motifs de matériau par identifiant. | Sonnet, medium | En cours |
| 2.4 | Éclairage stylisé, lumières ponctuelles, brouillard, profils d'ambiance | Implémenter l'éclairage stylisé, 32 lumières, brouillard de hauteur et neuf profils d'ambiance. | Opus, medium | À faire |
| 2.5 | Ombres en cascade | Ajouter des ombres solaires à trois cascades, stables pendant le mouvement de caméra. | Sonnet, high | À faire |
| 2.6 | Ciel procédural : heure, étoiles, nuages, espace | Rendre un ciel procédural piloté par l'heure et la latitude, qui éclaire aussi la scène. | Sonnet, high | À faire |
| 2.7 | Météo : particules, abri, humidité, tempête, transitions | Rendre la météo visuelle (pluie, neige, sable, brouillard) avec particules GPU sans état. | Sonnet, high | À faire |
| 2.8 | Surface de l'eau et milieu sous-marin | Rendre une surface d'eau par ondes de Gerstner et un milieu sous-marin avec caustiques. | Sonnet, medium | À faire |
| 2.9 | Post-traitement : contours, bloom, tonemapping, étalonnage, FXAA | Implémenter la chaîne de post-traitement sur le buffer HDR, pilotée par le profil d'ambiance. | Sonnet, medium | À faire |
| 2.10 | Anti-aliasing temporel, trait `Upscaler`, échelle de rendu, FSR 1 | Rendre à résolution interne réduite puis upscaler proprement, avec TAA et FSR 1. | Sonnet, high | À faire |
| 2.11 | FSR 2/3 par chargement dynamique | Ajouter FSR 2/3 en option, chargé à l'exécution sous Windows x86_64 uniquement. | Opus, high | À faire |
| 2.12 | DLSS via NVIDIA NGX | Ajouter DLSS derrière la feature `dlss`, actif sur cartes RTX Windows x86_64 uniquement. | Opus, high | À faire |
| 2.13 | Assets : recettes de maillages, palettes, validateur, vignettes | Transformer les recettes RON de props en maillages, avec validateur et miniatures hors écran. | Sonnet, medium | À faire |
| 2.14 | Bibliothèque de props par thème (environ 150 recettes) | Écrire environ 150 recettes de props validées sur neuf thèmes, sans changement de code. | Haiku, low | À faire |
| 2.15 | Personnages, armes, animation procédurale, viewmodel | Construire les personnages et armes en blocs rigides, animés par ressorts amortis. | Sonnet, medium | À faire |
| 2.16 | Intégration `egui`, thème, overlay de debug | Dessiner les interfaces `egui` à résolution d'affichage, avec un thème et un overlay de profilage. | Sonnet, medium | À faire |
| 2.17 | Moteur audio : effets synthétisés, spatialisation 3D | Créer le crate `audio` avec 32 voix priorisées, effets issus de recettes RON et spatialisation 3D. | Sonnet, medium | À faire |
| 3.1 | `mapfmt` : format de carte | Implémenter le format RON versionné, réécrit à l'identique sur l'exemple, avec empreinte BLAKE3. | Opus, medium | À faire |
| 3.2 | Build de carte : maillages, collisions, occlusion par sommet | Transformer une carte en maillages CPU et monde de collisions, avec occlusion ambiante baked. | Sonnet, medium | À faire |
| 3.3 | `map-tool` : validateur, rapport, vues du dessus et en perspective | Valider les cartes avec motifs de rejet précis, rapport JSON et images de vue. | Sonnet, medium | À faire |
| 4.1 | `minigame-api`, registre, phases de match, mode course témoin | Créer le framework des mini-jeux et un mode course témoin, sans changer l'API. | Opus, medium | À faire |
| 4.2 | Contrôleur de personnage complet | Implémenter le contrôleur de type Quake dans `sim`, déterministe et partagé client/serveur. | Opus, high | À faire |
| 4.3 | Glissade, slide-hop, saut mural, tremplins, zones | Ajouter les techniques de mouvement et les effets de zone, validés par des traces de référence. | Opus, medium | À faire |
| 4.4 | Armes hitscan, dispersion, recul, chaîne de dégâts, réapparition | Implémenter le pipeline de tir hitscan, prédit par le client et validé par le serveur. | Sonnet, high | À faire |
| 4.5 | Projectiles, explosions, mêlée | Ajouter les armes à projectiles, les explosions avec recul et la mêlée au pipeline de tir. | Sonnet, medium | À faire |
| 4.6 | Classes, équipements, armes originales, outil d'équilibrage | Définir six classes et leurs armes en données RON, avec `xtask balance` (TTK). | Sonnet medium puis Haiku (données) | À faire |
| 4.7 | Ramassages, score, modes équipe et chacun pour soi | Implémenter ramassages, score, minuteurs et deux modes sur `minigame-api`, jouables en headless. | Sonnet, medium | À faire |
| 4.8 | HUD de mode, caméra et sensations | Dessiner le HUD FPS via egui et implémenter la caméra et les constantes de sensation. | Sonnet, medium | À faire |
| 4.9 | Bots : graphe de navigation et recherche de chemin | Générer le graphe de navigation depuis une carte chargée et fournir A* avec lissage. | Sonnet, high | À faire |
| 4.10 | Bots : décision, perception, visée, quatre niveaux | Faire jouer les bots avec décisions utilitaires, perception retardée, visée balistique et quatre niveaux. | Sonnet, high | À faire |
| 5.1 | Rotation des cartes, préchargement, transfert par empreinte | Faire changer de carte toutes les 5 minutes et télécharger par empreinte les cartes manquantes. | Sonnet, medium | À faire |
| 5.2 | Éditeur : caméra, sélection, poignées, volumes, annulation | Permettre de construire une carte dans l'éditeur et de tout annuler pas à pas. | Sonnet, high | À faire |
| 5.3 | Éditeur : bibliothèque, gameplay, ambiance, symétrie | Permettre de créer une carte complète dans l'éditeur sans éditer le fichier à la main. | Sonnet, medium | À faire |
| 5.4 | Éditeur : validation continue, test immédiat, fichiers | Valider en direct, tester immédiatement, sauvegarder, exporter et importer les cartes. | Sonnet, medium | À faire |
| 5.5 | Essais de carte par bots, statistiques, guide de conception | Faire jouer 16 bots sur une carte sans rendu, produire un rapport et un guide de conception. | Sonnet, medium | À faire |
| 5.6 | Catalogue de douze cartes originales | Livrer douze cartes valides, testées par bots et vérifiées à la main. | Opus, medium | À faire |
| 6.1 | États de l'application, menu principal, chargement | Donner au client une machine à états empilée et tous les écrans principaux. | Sonnet, medium | À faire |
| 6.2 | Héberger avec bots et LAN, rejoindre, liste LAN, favoris | Permettre d'héberger (avec bots, éventuellement en LAN) et de rejoindre depuis les menus. | Sonnet, medium | À faire |
| 6.3 | Classes et équipements, budget, aperçu, validation serveur | Construire une tenue dans un budget de points, la voir en 3D, et faire valider côté serveur. | Sonnet, medium | À faire |
| 6.4 | Paramètres : structure, métadonnées, persistance, affichage, qualité | Une seule structure de paramètres, un menu généré par métadonnées, application sans redémarrage. | Sonnet, medium | À faire |
| 6.5 | Paramètres : commandes, audio, réseau, accessibilité, traduction | Finir les paramètres, détecter les conflits de touches, avoir une interface FR/EN complète. | Sonnet medium puis Haiku (textes) | À faire |
| 6.9 | Sensation de tir (lot interne) | Flash, traceurs, recul, impacts par matériau, marqueurs, sons, arme FPS sans clipping. | Sonnet, high | À faire |
| 6.10 | Terrain avec relief (lot interne) | Heightmap lisse, mélange par pente, collisions de pente, navigation bots, sculpture dans l'éditeur. | Opus, high | À faire |
| 6.11 | Props et végétation (lot interne) | Rendu des props, végétation low-poly générée, instanciation GPU, vent, LOD. | Opus, high | À faire |
| 6.12 | Paysage hors limites (lot interne) | Terrain et décor au-delà de la zone jouable, bordures naturelles par carte, brouillard et ciel. | Sonnet, high | À faire |
| 6.13 | Level design (lot interne) | Trois cartes non carrées conçues sur plan puis testées avec les bots. | Opus, high | À faire |
| 8.1 | Large terrain: format v5, chunk streaming, clipmap LOD | Terrain 1 à 2 km lisse, format v5, chunks en streaming, LOD, mélange de matières. | Opus, high | À faire |
| 8.2 | Far landscape: LOD for vegetation and props, landmarks, atmosphere | Horizon rempli : LOD végétation/props, repères lointains, brume atmosphérique. | Opus, high | À faire |
| 8.3 | World generator and editor sculpting tools | Générateur de relief (érosion, rivières) et outils de sculpture/pinceau végétation dans l'éditeur. | Sonnet, high | À faire |
| 8.4 | Natural boundaries and playable zone | Zone jouable de forme libre fermée par le décor (falaise, eau, forêt), sans mur invisible. | Sonnet, high | À faire |
| 8.5 | Organic modelling primitives for prop recipes | Nouveaux opérateurs de modélisation (tour, extrusion, chanfrein, bruit) pour des props réels. | Opus, high | À faire |
| 8.6 | Prop library, outdoor themes | Environ 100 props extérieur (rochers, clôtures, bateaux, bancs...) avec variantes. | Sonnet, medium | À faire |
| 8.7 | Prop library, built themes | Environ 100 props urbains/industriels/intérieur (voitures, étals, conteneurs...) avec variantes. | Sonnet, medium | À faire |
| 8.8 | Vegetation v2 and lighting polish | Végétation plus riche, herbe suivant le sol, occlusion ambiante, étalonnage par thème. | Sonnet, high | À faire |
| 8.9 | Rebuild Pine Hollow and Mossgate | Refaire Pine Hollow et Mossgate en vrais paysages, testés par bots. | Opus, high | À faire |
| 8.10 | Rebuild Driftwood Cove and Rust Harbor | Refaire Driftwood Cove et Rust Harbor en vrais paysages, testés par bots. | Opus, high | À faire |
| 8.11 | Rebuild Red Mesa, Greyfold Quarry and Nightjar Market | Refaire Red Mesa, Greyfold Quarry et Nightjar Market, testés par bots. | Opus, high | À faire |
| 8.12 | World performance and validation | Mesurer et tenir les budgets de la phase monde sur les sept cartes. | Sonnet, medium | À faire |
| 7.1 | Profilage et optimisation rendu et réseau | Mesurer et atteindre chaque budget de la section 20 sans changer les interfaces publiques. | Sonnet, medium | À faire |
| 7.2 | Validation des plateformes et packaging | Vérifier que chaque cible compile et produire une archive avec sommes par cible. | Sonnet, medium | À faire |
| 7.3 | Documentation en anglais, notes de version, tag v0.1.0 | Finaliser la documentation et publier `v0.1.0` par le pipeline de release. | Haiku, low | À faire |

## Jalons

J0 après 0.4 · J1 après 1.8 · J2 après 2.10 · J3 après 4.2 · J4 après 4.10 · J5 après 5.6 · J6 après 6.5 · J7 après 7.3 (`v0.1.0`).
