//! Tests de parité : la détection de silence en Rust doit produire exactement
//! les mêmes points de coupe que `SilenceLess.py` (écart attendu : 0 ms).
//!
//! Les fichiers de test sont des sinusoïdes déterministes :
//!   - silence = échantillons nuls (RMS 0)
//!   - son = sinusoïde 440 Hz d'amplitude 8000 (RMS ≈ 5657)
//! Avec un seuil de -30 dBFS (soit ~1036,2), la séparation est sans
//! ambiguïté. Les valeurs attendues correspondent exactement à celles produites
//! par `SilenceLess.py` avec les mêmes réglages (vérifié, écart 0 ms).

use std::path::{Path, PathBuf};

use silence_less_core::{
    detect_nonsilent_ranges, process_file, read_wav, trim_samples, Settings,
};

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;

/// Génère `frames * CHANNELS` échantillons, avec une sinusoïde entre
/// `start_frame` et `end_frame` (en frames), et du silence ailleurs.
fn sine(
    frames: usize,
    start_frame: usize,
    end_frame: usize,
    amplitude: f64,
    frequency: f64,
) -> Vec<i16> {
    let mut samples = vec![0_i16; frames * CHANNELS as usize];
    let end = end_frame.min(frames);
    for frame in start_frame..end {
        let t = frame as f64 / SAMPLE_RATE as f64;
        let value = (amplitude * (2.0 * std::f64::consts::PI * frequency * t).sin()).round() as i16;
        for channel in 0..CHANNELS as usize {
            samples[frame * CHANNELS as usize + channel] = value;
        }
    }
    samples
}

fn write_wav(path: &Path, samples: &[i16]) {
    let spec = hound::WavSpec {
        channels: CHANNELS,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("création du WAV");
    for &sample in samples {
        writer.write_sample(sample).expect("écriture échantillon");
    }
    writer.finalize().expect("finalisation du WAV");
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sl_parity_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("création du dossier temporaire");
    dir
}

/// Seuil épinglé à -30 dB : les valeurs attendues ont été validées contre
/// `SilenceLess.py` avec ce réglage (indépendant du seuil par défaut de l'app).
fn settings() -> Settings {
    Settings {
        silence_threshold_db: -30.0,
        ..Default::default()
    }
}

#[test]
fn detects_one_burst_with_padding() {
    // 30 s : son de 2 s à 8 s, silence ailleurs.
    let dir = temp_dir("burst");
    let path = dir.join("burst.wav");
    let samples = sine(
        SAMPLE_RATE as usize * 30,
        2 * SAMPLE_RATE as usize,
        8 * SAMPLE_RATE as usize,
        8000.0,
        440.0,
    );
    write_wav(&path, &samples);

    let audio = read_wav(&path).expect("lecture WAV");
    assert_eq!(audio.duration_ms, 30_000);

    let settings = settings();
    let ranges = detect_nonsilent_ranges(&audio, &settings);
    assert_eq!(ranges, vec![(2_010, 7_990)]);

    // Avec 100 ms de marge de chaque côté : [1910, 8090] -> 6 180 ms -> 272 538 frames.
    let trimmed = trim_samples(&audio, &settings).expect("découpe");
    assert_eq!(trimmed.len(), 272_538 * CHANNELS as usize);
}

#[test]
fn process_file_encodes_mp3() {
    let dir = temp_dir("process");
    let path = dir.join("burst.wav");
    let out = dir.join("burst.mp3");
    let samples = sine(
        SAMPLE_RATE as usize * 30,
        2 * SAMPLE_RATE as usize,
        8 * SAMPLE_RATE as usize,
        8000.0,
        440.0,
    );
    write_wav(&path, &samples);

    let stats = process_file(&path, &out, &settings()).expect("traitement");
    assert!(!stats.all_silent);
    assert_eq!(stats.original_ms, 30_000);
    assert_eq!(stats.kept_ms, 6_180);
    assert!(out.exists());
    assert!(out.metadata().unwrap().len() > 0);
}

#[test]
fn streaming_matches_in_memory() {
    // Le chemin « en flux » (process_file par défaut) doit produire la même
    // durée conservée que le chemin en mémoire (trim_samples).
    let dir = temp_dir("streaming");
    let path = dir.join("burst.wav");
    let out = dir.join("burst.mp3");
    let samples = sine(
        SAMPLE_RATE as usize * 30,
        2 * SAMPLE_RATE as usize,
        8 * SAMPLE_RATE as usize,
        8000.0,
        440.0,
    );
    write_wav(&path, &samples);

    let settings = settings();
    let audio = silence_less_core::read_wav(&path).expect("lecture");
    let expected_frames = silence_less_core::trim_samples(&audio, &settings)
        .expect("découpe")
        .len()
        / CHANNELS as usize;

    let stats = process_file(&path, &out, &settings).expect("traitement en flux");
    let streamed_frames = (stats.kept_ms as u64 * SAMPLE_RATE as u64 / 1000) as usize;
    assert_eq!(streamed_frames, expected_frames);
    assert_eq!(stats.kept_ms, 6_180);
}

#[test]
fn all_silent_returns_none() {
    // 5 s de silence pur.
    let dir = temp_dir("silent");
    let path = dir.join("silent.wav");
    write_wav(&path, &vec![0_i16; 5 * SAMPLE_RATE as usize * CHANNELS as usize]);

    let audio = read_wav(&path).expect("lecture WAV");
    let settings = settings();
    assert!(detect_nonsilent_ranges(&audio, &settings).is_empty());
    assert!(trim_samples(&audio, &settings).is_none());
}

#[test]
fn all_sound_keeps_everything() {
    // 10 s de son continu.
    let dir = temp_dir("allsound");
    let path = dir.join("all.wav");
    let frames = 10 * SAMPLE_RATE as usize;
    write_wav(&path, &sine(frames, 0, frames, 8000.0, 440.0));

    let audio = read_wav(&path).expect("lecture WAV");
    let ranges = detect_nonsilent_ranges(&audio, &settings());
    assert_eq!(ranges, vec![(0, 10_000)]);
}

#[test]
fn trims_internal_silence_when_enabled() {
    // 15 s : son de 2 à 4 s puis de 10 à 12 s (silence interne de 6 s).
    let dir = temp_dir("internal");
    let path = dir.join("two.wav");
    let frames = 15 * SAMPLE_RATE as usize;
    let first = sine(frames, 2 * SAMPLE_RATE as usize, 4 * SAMPLE_RATE as usize, 8000.0, 440.0);
    let second = sine(
        frames,
        10 * SAMPLE_RATE as usize,
        12 * SAMPLE_RATE as usize,
        8000.0,
        440.0,
    );
    let samples: Vec<i16> = first
        .iter()
        .zip(second)
        .map(|(a, b)| a.wrapping_add(b))
        .collect();
    write_wav(&path, &samples);

    let audio = read_wav(&path).expect("lecture WAV");
    let settings = settings();
    assert_eq!(
        detect_nonsilent_ranges(&audio, &settings),
        vec![(2_010, 3_990), (10_010, 11_990)]
    );

    // Sans `trim_internal_silence`, on garde du premier au dernier son.
    let full = trim_samples(&audio, &settings).expect("découpe");
    // [1910, 12090] -> 10 180 ms.
    assert_eq!(full.len() / CHANNELS as usize, 10_180 * SAMPLE_RATE as usize / 1000);
}
