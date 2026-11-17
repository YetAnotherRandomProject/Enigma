# Enigma Notes

Éditeur de notes Markdown visuel, en Rust natif, pensé pour les jeux d'énigmes
(premier cas d'usage : **Blue Prince**). Chaque fiche peut avoir plusieurs
catégories, des liens typés vers d'autres fiches, et l'ensemble se visualise en
graphe 2D/3D filtrable. Réutilisable pour d'autres jeux via des **packs de jeu**.

## Contraintes non négociables
- Rust natif, **aucun navigateur embarqué** (pas de Tauri/Electron/webview/WASM).
- UI : `eframe`/`egui` en mode immédiat. Aucun gros calcul à chaque frame :
  caches + dirty flags (parsing, index de liens, layout, mise en forme).
- Cible Windows ; doit compiler sur Linux et macOS. Pas de code spécifique OS
  sans nécessité (et alors derrière `cfg` documenté).
- Données = dossier de fichiers `.md` (une fiche par fichier), lisible et
  versionnable avec git. Écritures **atomiques** (fichier temporaire dans le
  même dossier puis renommage).
- Pas de compatibilité Obsidian requise.

## Architecture (workspace Cargo)
| Crate | Rôle | Dépend de egui ? |
|---|---|---|
| `crates/core` (`enigma-core`) | modèle, parsing des fiches, index des liens, filtres, packs, stockage disque | **non** |
| `crates/graph` (`enigma-graph`) | layout en forces 2D/3D, caméra, projection, picking | **non** |
| `crates/app` (`enigma-app`, binaire `enigma`) | application eframe : panneaux, éditeur, rendu du graphe, raccourcis | oui |
| `packs/` | packs de jeu déclaratifs (`packs/blue_prince/pack.json`) | — |

Règle : toute logique testable vit dans `core` ou `graph`. `app` ne fait que
présenter l'état et traduire les interactions en appels à `core`.

### Extensibilité (phase 2, non implémentée)
Les packs sont purement déclaratifs pour l'instant. Le code doit permettre
d'ajouter plus tard un trait `Plugin` (vues spécialisées type grille 5×9 du
manoir, importeurs) ou un moteur de script (Rhai) : `core` expose une API de
projet claire (lecture des fiches, index, événements de modification), et
`app` organise ses vues de façon à pouvoir en enregistrer de nouvelles. Ne pas
écrire ce trait tant que ce n'est pas demandé.

## Format d'une fiche
Fichier `.md` avec un en-tête YAML (`---` … `---`) **jamais affiché en texte
brut dans l'éditeur** :

```yaml
---
id: 01HXYZ...                 # ULID stable, généré automatiquement
title: Salle des Archives
categories: [piece, indice]   # plusieurs catégories, issues du pack
status: theorie               # statut issu du pack
fields: { jour_decouverte: 3 }
links:
  - { to: 01HABC..., type: indique, note: "via le code du coffre" }
pos: { x: 120.0, y: -40.0, z: 0.0, pinned: true }
---
Texte libre avec [[Autre fiche]] ou [[Autre fiche|type_de_lien]].
```

Règles :
- `[[Titre]]` résolu par titre (insensible à la casse) ; l'**ID est la
  référence**. Renommer une fiche met à jour toutes les références textuelles ;
  un lien ne doit jamais casser.
- Liens du corps + liens des métadonnées fusionnés dans un seul index, sans doublon.
- Fichier sans en-tête accepté (valeurs par défaut) ; l'en-tête est ajouté à la
  première sauvegarde.
- Clés YAML inconnues conservées telles quelles à la réécriture.

## Packs de jeu
Dossier contenant un `pack.json` (champ `version` obligatoire) : `categories`,
`link_types`, `statuses`, `fields`, `templates`. Validation stricte
(`deny_unknown_fields`, ids uniques, références valides, couleurs `#rrggbb`)
avec messages d'erreur lisibles. Un projet référence son pack dans
`.enigma/project.json`.

## Conventions
- Édition Rust 2024, MSRV = celle d'eframe (1.95).
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` sans
  avertissement, tests unitaires sur toute la logique non-UI.
- CI locale : `scripts/ci.ps1` (Windows) / `scripts/ci.sh` (Linux/macOS) = fmt
  check + clippy + tests.
- **Jamais de `unwrap()`/`expect()` hors tests** (lint clippy `unwrap_used` en
  deny au niveau workspace). Erreurs : `thiserror` dans les bibliothèques,
  `anyhow` dans l'app, affichées lisiblement dans l'interface.
- Journalisation via `tracing`.
- Tout ajout de dépendance est justifié (ici ou dans le commit) ; versions
  vérifiées sur crates.io au moment de l'ajout.
- Petits commits clairs, un par étape logique. Code et identifiants en anglais,
  interface et docs utilisateur en français.
- Décision de conception non évidente → exposer les options et recommander,
  ne pas choisir silencieusement.

## Notes techniques (pièges connus)
- **Rendu Windows** : DX12 est forcé par défaut (`main.rs`), car des couches
  Vulkan tierces (overlays, ReShade…) font planter la création de l'instance
  Vulkan (0xC0000005). `WGPU_BACKEND=vulkan|dx12|gl` reste prioritaire.
- **Polices** : egui n'embarque pas de gras ; le gras et les titres utilisent
  la couleur « forte » du thème. Emoji : feature egui `monochrome_emoji_fonts`
  (+ `system_fonts` d'eframe en repli). Les glyphes absents des polices
  embarquées (ex. `●`, `＋`, `→`, `🧑`) font paniquer les tests d'interface :
  préférer `•`, `+`, `»`, `👤`.
- **Historique** : Ctrl+Z/Y sont consommés avant les widgets ; l'undo interne
  du `TextEdit` d'egui n'est jamais utilisé (pile maison par fiche,
  `enigma_core::history`).
- **Index de liens** (`enigma_core::links`) : liens bruts par fiche + index
  inverses titre/id. Fusion corps + en-tête : même cible et même type → un
  seul lien ; un lien sans type est absorbé par un lien typé vers la même
  fiche. Deux types différents vers la même fiche = deux liens.
- **Auto-complétion** : les touches ↑/↓/Entrée/Tab/Échap sont consommées
  avant le `TextEdit` ; Échap n'est capturé par son `EventFilter` que
  lorsque les suggestions sont ouvertes (sinon egui retire le focus avant).
- **Filtres** (`enigma_core::filter`) : ET entre critères, OU à l'intérieur
  (statuts, types de liens) ; catégories : toutes/au moins une + exclusion.
  La liste n'est recalculée que si le filtre, `generation` ou (filtres
  dépendant du contenu) `content_version` changent.
- **Édition de packs** : un pack du projet ou du dossier utilisateur est
  modifié sur place ; un pack fourni avec l'application est enregistré en
  copie dans `.enigma/packs/<id>` (prioritaire), l'original n'est jamais
  modifié. Les identifiants existants sont verrouillés.
- **egui** : dans une `Grid`, forcer la largeur d'un `TextEdit` avec
  `ui.add_sized` (sinon la colonne reste étroite). Dans les tests kittest,
  donner le focus (`.focus()` + `run()`) avant `type_text`.
- **Fichiers sans en-tête** : jamais réécrits tant qu'ils ne sont pas modifiés.
- **Écho de nos écritures** : `Project` garde l'empreinte du dernier contenu
  écrit/lu ; `on_file_changed` ignore les événements identiques.

## Tests
- `cargo test --workspace` : logique (core) + tests d'interface sans fenêtre
  (`egui_kittest`, dans `crates/app/src/workspace.rs`), sur une copie de
  `exemples/blue_prince_demo`.
- Captures PNG des tests d'interface :
  `ENIGMA_SNAPSHOT_DIR=<dossier> WGPU_BACKEND=dx12 cargo test -p enigma-app -- --test-threads=1`
  (le rendu GPU en parallèle plante sur certains pilotes).
- Lancer sur la démo : `cargo run -p enigma-app -- exemples/blue_prince_demo`
  (attention : modifie les fichiers de la démo ; travailler sur une copie).

## Méthode
Un jalon à la fois. Fin de jalon : tout compile, tests verts, résumé de ce qui
marche + comment tester, puis **arrêt en attendant le feu vert**. Rien des
jalons suivants n'est codé avant validation.

## Jalons
- [x] **1. Fondations et éditeur** — workspace, CI locale, ouverture de dossier,
  liste des fiches avec recherche, création/suppression/renommage ; éditeur avec
  en-tête masqué, pastilles de catégories, sélecteur de statut, champs dans un
  panneau latéral ; mise en forme pendant la frappe (layouter) ; sauvegarde
  auto, annuler/refaire, modifications externes (`notify`).
- [x] **2. Liens et rétroliens** — auto-complétion `[[` puis `|type`, panneau
  liens sortants / rétroliens / cassés, index incrémental (1000+ fiches).
- [ ] **3. Packs de jeu et filtres** — modèles, couleurs/icônes, changement et
  création de pack, filtres combinables.
- [ ] **4. Graphe 2D** — nœuds en secteurs multi-catégories, arêtes typées,
  layout en forces, épinglage, filtres, voisinage, aperçu au survol.
- [ ] **5. Graphe 3D** — caméra orbitale, perspective, billboards, mêmes
  filtres, bascule 2D/3D, axe Z optionnel sur un champ, 60 FPS à 2000 nœuds /
  5000 liens (proposer wgpu custom avant de migrer).
- [ ] **6. Liens à la souris** — tirer un fil, choisir le type, note, éditer /
  supprimer depuis le graphe.
- [ ] **7. Confort** — palette Ctrl+P, raccourcis configurables, thèmes,
  export image, tableau d'enquête libre.
