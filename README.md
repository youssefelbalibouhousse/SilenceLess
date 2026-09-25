# SilenceLess

> Convertit des fichiers WAV en MP3 en supprimant les silences — pensé pour les
> enregistrements de conférences destinés à Telegram et autres plateformes audio.

## Présentation

SilenceLess détecte et retire les silences (début, fin, et optionnellement à
l'intérieur) d'enregistrements audio, puis exporte le résultat en MP3. L'outil
existe sous deux formes :

- **Application de bureau** (Tauri + Rust) : interface graphique avec
  glisser-déposer, traitement par lot et réglages.
- **CLI** (Rust) et **prototype Python** : pour les traitements en ligne de
  commande et les scripts.

## Fonctionnalités

- Détection des silences par seuil RMS (dBFS), fenêtre glissante et pas
  configurable.
- Marge de silence conservée autour des zones utiles (évite de couper les mots).
- Coupe optionnelle des silences **internes**.
- Bitrate MP3 configurable (LAME).
- Traitement **en flux** : mémoire bornée (~7 Mo), adapté aux enregistrements de
  plusieurs heures.
- Traitement **parallèle** de plusieurs fichiers.

## Structure du projet

```
.
├── silence-less/                  # Projet Rust (workspace cargo)
│   ├── crates/silence-less-core/  #   Cœur : lecture WAV, détection, encodage MP3
│   ├── crates/silence-less/       #   CLI
│   ├── src-tauri/                 #   Application de bureau (backend Tauri)
│   └── ui/                        #   Interface (HTML/CSS/JS)
├── SilenceLess.py                 # Prototype Python (référence algorithmique)
└── requirements.txt               # Dépendances du prototype Python
```

## Prérequis

| Composant | Nécessaire pour | Installation |
|---|---|---|
| Rust (stable) | tout le projet Rust | `winget install Rustlang.Rustup` |
| MSVC Build Tools (Windows) | compilation Rust | Visual Studio Build Tools |
| WebView2 | application de bureau | préinstallé sur Windows 10/11 |
| Python 3.12+ + ffmpeg | prototype Python | `pip install -r requirements.txt` + `winget install Gyan.FFmpeg` |

## Construction et exécution

### Application de bureau

```powershell
cd silence-less
cargo tauri dev            # lancer en mode développement
cargo tauri build          # générer l'installeur (target/release/bundle/nsis/)
```

### CLI Rust

```powershell
cd silence-less
cargo run -p silence-less -- "C:\chemin\vers\wavs" -o "C:\sortie" -j 4
```

Options : `-o/--out`, `-j/--jobs`, `--seek-step`, `--min-silence`,
`--threshold`, `--padding`, `--bitrate`, `--trim-internal`.

### Prototype Python

```powershell
python SilenceLess.py "C:\chemin\vers\wavs" -o "C:\sortie"
```

## Tests

```powershell
cd silence-less
cargo test -p silence-less-core   # 6 tests, dont la parité flux/mémoire
```

## Licence

Logiciel **propriétaire** — tous droits réservés. Voir [`LICENSE`](LICENSE).

## Sécurité

Voir [`SECURITY.md`](SECURITY.md) pour signaler une vulnérabilité.
