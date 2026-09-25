#!/usr/bin/env python3
"""
trim_silence_to_mp3.py
Récupère tous les .wav d'un dossier, tronque les silences au début/fin
(et éventuellement à l'intérieur), puis exporte en .mp3.
"""

import argparse
import os
import sys
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import cast

import numpy as np
from pydub import AudioSegment
from pydub.silence import detect_silence
from pydub.utils import db_to_float


# ---------- CONFIGURATION ----------
# Seuil de silence en dBFS (plus c'est proche de 0, plus c'est strict)
SILENCE_THRESHOLD_DB = -30

# Durée minimale d'un silence pour être considéré comme tel (ms)
MIN_SILENCE_LEN_MS = 500

# Marge de silence conservée autour de la zone utile (ms)
KEEP_PADDING_MS = 100

# Bitrate MP3
MP3_BITRATE = "192k"

# Couper AUSSI les silences à l'intérieur (True) ou seulement début/fin (False)
TRIM_INTERNAL_SILENCE = False

# Pas de recherche des silences, en ms.
# pydub vaut 1 par défaut : il teste une fenêtre de MIN_SILENCE_LEN_MS à CHAQUE
# milliseconde, ce qui recopie la fenêtre entière à chaque itération. Passer à
# 10 ms divise le temps de détection par ~9 sans conséquence pratique, puisque
# les bords trouvés sont de toute façon dilatés de KEEP_PADDING_MS.
SEEK_STEP_MS = 10

# Nombre de fichiers traités en parallèle (0 = automatique, 1 = séquentiel)
JOBS = 0
# -----------------------------------


def _cut(audio: AudioSegment, start: int, end: int) -> AudioSegment:
    """Découpe audio[start:end] en garantissant un AudioSegment.

    Le transtypage est nécessaire car le `__getitem__` de pydub est annoté comme
    pouvant renvoyer un générateur : c'est le cas uniquement quand un *pas* de
    slice est fourni (ex. audio[::100]), ce que l'on n'utilise jamais ici.
    """
    return cast(AudioSegment, audio[start:end])


@dataclass(frozen=True)
class Settings:
    """Paramètres de traitement.

    Regroupés dans un objet (plutôt que lus dans les globales) pour pouvoir être
    transmis explicitement aux processus enfants lors du traitement parallèle :
    sous Windows ceux-ci sont démarrés en « spawn » et réimportent le module.
    """

    silence_threshold_db: int = SILENCE_THRESHOLD_DB
    min_silence_len_ms: int = MIN_SILENCE_LEN_MS
    keep_padding_ms: int = KEEP_PADDING_MS
    seek_step_ms: int = SEEK_STEP_MS
    mp3_bitrate: str = MP3_BITRATE
    trim_internal_silence: bool = TRIM_INTERNAL_SILENCE


# Largeurs d'échantillon gérées par le chemin numpy (le WAV 8 bits et le 24 bits
# retombent sur l'implémentation de pydub).
_SAMPLE_DTYPES = {2: "<i2", 4: "<i4"}


def _silent_ranges(
    audio: AudioSegment,
    min_silence_len: int,
    silence_thresh: int,
    seek_step: int,
) -> list[list[int]]:
    """Équivalent numpy de `pydub.silence.detect_silence` (bien plus rapide).

    pydub évalue `audio[i:i + min_silence_len].rms` pour chaque i multiple de
    `seek_step` : chaque itération recopie toute la fenêtre (~0,9 Mo pour 500 ms
    en 16 bits stéréo 44,1 kHz) puis en recalcule l'énergie. Le coût est donc
    O(durée x min_silence_len) avec une allocation énorme.

    Ici les échantillons sont lus une seule fois, l'énergie est calculée par
    petits blocs de `seek_step` ms, puis chaque fenêtre est obtenue par
    différence de sommes cumulées : O(durée), quelle que soit la longueur de
    fenêtre.
    """
    dtype = _SAMPLE_DTYPES.get(audio.sample_width)
    raw = audio.raw_data
    if dtype is None or raw is None:
        # Format exotique : on garde la correction plutôt que la vitesse.
        return detect_silence(audio, min_silence_len, silence_thresh, seek_step)

    seg_len = len(audio)
    if seg_len < min_silence_len:
        return []

    block_frames = int(round(seek_step * audio.frame_rate / 1000))
    block = block_frames * audio.channels
    if block <= 0:
        return detect_silence(audio, min_silence_len, silence_thresh, seek_step)

    samples = np.frombuffer(raw, dtype=dtype)
    # Complète jusqu'à un nombre entier de blocs (zéros en fin, inoffensifs).
    n_blocks = -(-len(samples) // block)
    padded = np.zeros(n_blocks * block, dtype=dtype)
    padded[: len(samples)] = samples

    # Énergie par bloc. `einsum` évite de matérialiser le tableau des carrés.
    grid = padded.reshape(n_blocks, block)
    energy = np.einsum("ij,ij->i", grid, grid, dtype=np.float64)

    blocks_per_win = max(1, int(round(min_silence_len / seek_step)))
    n_starts = n_blocks - blocks_per_win + 1
    if n_starts <= 0:
        return []

    cumulative = np.concatenate(([0.0], np.cumsum(energy)))
    starts = np.arange(n_starts)
    window_energy = cumulative[starts + blocks_per_win] - cumulative[starts]
    rms = np.sqrt(window_energy / (blocks_per_win * block))

    threshold = db_to_float(silence_thresh) * audio.max_possible_amplitude
    silent_blocks = starts[rms <= threshold]
    if silent_blocks.size == 0:
        return []

    # Fusion des blocs contigus, puis conversion en millisecondes. On raisonne en
    # indices de blocs pour rester insensible aux arrondis flottants, ce qui
    # reproduit exactement la logique de pydub (`s == prev + seek_step`).
    ms_per_block = block_frames * 1000.0 / audio.frame_rate
    merged: list[list[int]] = []
    first = prev = int(silent_blocks[0])
    for value in silent_blocks[1:]:
        current = int(value)
        if current != prev + 1 and (current - prev) * ms_per_block > min_silence_len:
            merged.append([first, prev])
            first = current
        prev = current
    merged.append([first, prev])

    return [
        [
            int(round(start * ms_per_block)),
            int(round(end * ms_per_block + min_silence_len)),
        ]
        for start, end in merged
    ]


def _nonsilent_ranges(
    audio: AudioSegment,
    min_silence_len: int,
    silence_thresh: int,
    seek_step: int,
) -> list[list[int]]:
    """Équivalent numpy de `pydub.silence.detect_nonsilent`."""
    silent_ranges = _silent_ranges(audio, min_silence_len, silence_thresh, seek_step)
    seg_len = len(audio)

    if not silent_ranges:
        return [[0, seg_len]]

    if silent_ranges[0][0] == 0 and silent_ranges[0][1] == seg_len:
        return []

    nonsilent_ranges: list[list[int]] = []
    prev_end = 0
    for start, end in silent_ranges:
        nonsilent_ranges.append([prev_end, start])
        prev_end = end

    if silent_ranges[-1][1] != seg_len:
        nonsilent_ranges.append([prev_end, seg_len])

    if nonsilent_ranges and nonsilent_ranges[0] == [0, 0]:
        nonsilent_ranges.pop(0)

    return nonsilent_ranges


def trim_silence(audio: AudioSegment, settings: Settings) -> tuple[AudioSegment, bool]:
    """Retourne (audio tronqué, fichier entièrement silencieux)."""
    nonsilent_ranges = _nonsilent_ranges(
        audio,
        min_silence_len=settings.min_silence_len_ms,
        silence_thresh=settings.silence_threshold_db,
        seek_step=settings.seek_step_ms,
    )

    if not nonsilent_ranges:
        return audio, True

    if settings.trim_internal_silence:
        # Concatène chaque segment non-silencieux
        result = AudioSegment.empty()
        for start, end in nonsilent_ranges:
            start = max(0, start - settings.keep_padding_ms)
            end = min(len(audio), end + settings.keep_padding_ms)
            result += _cut(audio, start, end)
        return result, False

    # On garde seulement du premier au dernier son
    start = max(0, nonsilent_ranges[0][0] - settings.keep_padding_ms)
    end = min(len(audio), nonsilent_ranges[-1][1] + settings.keep_padding_ms)
    return _cut(audio, start, end), False


@dataclass(frozen=True)
class Result:
    """Compte rendu d'un fichier, renvoyé par les processus enfants."""

    name: str
    ok: bool
    message: str = ""
    original_ms: int = 0
    kept_ms: int = 0
    mp3_path: str = ""


def process_one(wav_path: Path, output_folder: Path, settings: Settings) -> Result:
    """Traite un seul fichier.

    Cette fonction s'exécute dans un processus enfant : elle ne doit rien
    afficher, sinon les sorties des différents processus s'entremêlent.
    """
    mp3_path = output_folder / (wav_path.stem + ".mp3")

    try:
        audio = AudioSegment.from_wav(str(wav_path))
        original_ms = len(audio)

        trimmed, all_silent = trim_silence(audio, settings)
        if all_silent:
            return Result(wav_path.name, True, "entièrement silencieux, ignoré", original_ms)

        kept_ms = len(trimmed)
        trimmed.export(str(mp3_path), format="mp3", bitrate=settings.mp3_bitrate)
        return Result(wav_path.name, True, "", original_ms, kept_ms, str(mp3_path))

    except Exception as exc:  # un fichier en erreur ne doit pas arrêter le lot
        return Result(wav_path.name, False, str(exc))


def _process_one_star(args: tuple[Path, Path, Settings]) -> Result:
    """Adaptateur pour `Executor.map`, qui ne transmet qu'un seul argument."""
    return process_one(*args)


def _report(result: Result) -> None:
    print(f"▶ {result.name}")
    if not result.ok:
        print(f"   ❌ Erreur : {result.message}\n")
    elif result.message:
        print(f"   ⚠️  {result.message}\n")
    else:
        removed_ms = result.original_ms - result.kept_ms
        print(f"   ✅ {result.kept_ms / 1000:.2f}s (silence retiré : {removed_ms / 1000:.2f}s)")
        print(f"   → {result.mp3_path}\n")


def find_wav_files(folder: Path) -> list[Path]:
    """Liste les .wav du dossier, sans doublon (le FS Windows ignore la casse)."""
    wav_files = sorted(folder.glob("*.wav")) + sorted(folder.glob("*.WAV"))
    return list({f.resolve(): f for f in wav_files}.values())


def process_folder(
    folder: Path,
    output_folder: Path | None = None,
    settings: Settings | None = None,
    jobs: int = JOBS,
) -> int:
    """Traite tous les .wav du dossier. Retourne le nombre de fichiers en échec."""
    settings = settings or Settings()

    if not folder.is_dir():
        print(f"❌ Dossier introuvable : {folder}")
        sys.exit(1)

    if output_folder is None:
        output_folder = folder / "mp3_output"
    output_folder.mkdir(parents=True, exist_ok=True)

    wav_files = find_wav_files(folder)
    if not wav_files:
        print(f"❌ Aucun fichier .wav trouvé dans {folder}")
        return 0

    print(f"🎵 {len(wav_files)} fichier(s) .wav trouvé(s)")

    workers = jobs if jobs > 0 else min(len(wav_files), os.cpu_count() or 1)
    if workers > 1:
        print(f"⚙️  {workers} fichiers traités en parallèle\n")
    else:
        print()

    work = [(path, output_folder, settings) for path in wav_files]

    if workers > 1:
        # Détection et encodage sont purement CPU : des processus séparés sont
        # préférables à des threads, que le GIL de CPython sérialiserait.
        with ProcessPoolExecutor(max_workers=workers) as pool:
            results = list(pool.map(_process_one_star, work))
    else:
        results = [process_one(*item) for item in work]

    for result in results:
        _report(result)

    return sum(1 for r in results if not r.ok)


def _force_utf8_output() -> None:
    """Force UTF-8 sur la sortie standard.

    Sans cela, dès que la sortie est redirigée (`> log.txt`, un pipe, une tâche
    planifiée...), Python encode avec la page de codes locale (cp1252 sous
    Windows) et le premier emoji lève UnicodeEncodeError. `errors="replace"`
    garantit qu'un terminal exotique dégrade l'affichage au lieu de planter.
    """
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            reconfigure(encoding="utf-8", errors="replace")


def main() -> None:
    _force_utf8_output()

    parser = argparse.ArgumentParser(
        description="Tronque les silences des .wav d'un dossier et exporte en .mp3."
    )
    parser.add_argument("folder", nargs="?", help="dossier contenant les .wav")
    parser.add_argument(
        "-o",
        "--out",
        help="dossier de sortie (défaut : <dossier>/mp3_output). Écrire sur le "
        "disque local est plus rapide que réécrire sur la clé USB.",
    )
    parser.add_argument(
        "-j",
        "--jobs",
        type=int,
        default=JOBS,
        help="nombre de fichiers traités en parallèle (0 = automatique, 1 = séquentiel)",
    )
    parser.add_argument(
        "--seek-step",
        type=int,
        default=SEEK_STEP_MS,
        help=f"pas de détection des silences en ms (défaut : {SEEK_STEP_MS})",
    )
    args = parser.parse_args()

    folder_str = args.folder
    if not folder_str:
        # Mode interactif si aucun argument fourni
        folder_str = input("Chemin du dossier contenant les .wav : ").strip().strip('"')

    settings = Settings(seek_step_ms=args.seek_step)
    output = Path(args.out).expanduser() if args.out else None

    failures = process_folder(Path(folder_str).expanduser(), output, settings, args.jobs)
    if failures:
        print(f"🏁 Terminé ({failures} fichier(s) en erreur).")
    else:
        print("🏁 Terminé.")


if __name__ == "__main__":
    main()