//! `silence-less-core` — lecture WAV, détection de silence, encodage MP3.
//!
//! Portage Rust de l'algorithme validé dans `SilenceLess.py` :
//! l'énergie est calculée par petits blocs puis sommée par fenêtres glissantes
//! (sommes cumulées), pour un coût O(durée) au lieu de O(durée × fenêtre).

use std::path::Path;

use hound::{SampleFormat, WavReader};
use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, InterleavedPcm, MonoPcm, Quality};

/// Paramètres du traitement.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Seuil de silence en dBFS (ex. `-30.0`). Plus c'est proche de 0, plus c'est strict.
    pub silence_threshold_db: f64,
    /// Durée minimale d'un silence (ms).
    pub min_silence_len_ms: u32,
    /// Marge de silence conservée autour de la zone utile (ms).
    pub keep_padding_ms: u32,
    /// Pas de recherche des silences (ms).
    pub seek_step_ms: u32,
    /// Bitrate MP3 cible (kbps).
    pub bitrate_kbps: u32,
    /// Couper aussi les silences à l'intérieur.
    pub trim_internal_silence: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            silence_threshold_db: -45.0,
            min_silence_len_ms: 500,
            keep_padding_ms: 100,
            seek_step_ms: 10,
            bitrate_kbps: 192,
            trim_internal_silence: false,
        }
    }
}

/// Audio PCM chargé en mémoire (échantillons entrelacés).
#[derive(Debug, Clone)]
pub struct Audio {
    /// Échantillons entrelacés (L R L R ...), normalisés en `i32`.
    pub samples: Vec<i32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    /// Durée totale en millisecondes.
    pub duration_ms: u32,
}

/// Compte rendu du traitement d'un fichier.
#[derive(Debug, Clone)]
pub struct ProcessStats {
    pub original_ms: u32,
    pub kept_ms: u32,
    /// `true` si le fichier est entièrement silencieux (aucun MP3 produit).
    pub all_silent: bool,
    pub output_path: String,
}

/// Lit un fichier WAV à échantillons entiers 16, 24 ou 32 bits.
pub fn read_wav(path: &Path) -> Result<Audio, String> {
    let mut reader = WavReader::open(path).map_err(|e| format!("lecture de {path:?} : {e}"))?;
    let spec = reader.spec();

    let samples: Vec<i32> = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(i32::from))
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        (SampleFormat::Int, 24 | 32) => reader
            .samples::<i32>()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        _ => {
            return Err(format!(
                "format non géré : {:?} sur {} bits (WAV entiers 16/24/32 bits attendu)",
                spec.sample_format, spec.bits_per_sample
            ))
        }
    };

    let n = samples.len();
    let duration_ms = if spec.sample_rate > 0 && spec.channels > 0 {
        (n as u64 * 1000 / (u64::from(spec.sample_rate) * u64::from(spec.channels))) as u32
    } else {
        0
    };

    Ok(Audio {
        samples,
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        bits_per_sample: spec.bits_per_sample,
        duration_ms,
    })
}

/// Convertit un seuil en dBFS en amplitude linéaire.
fn db_to_amp(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

/// Amplitude maximale possible pour une profondeur de bits donnée.
fn max_possible_amplitude(bits: u16) -> f64 {
    2.0_f64.powi(i32::from(bits) - 1)
}

/// Liste les fichiers `.wav` d'un dossier (insensible à la casse), triés.
pub fn find_wav_files(folder: &Path) -> Vec<std::path::PathBuf> {
    let mut wavs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_wav = path
                .extension()
                .map(|e| e.eq_ignore_ascii_case("wav"))
                .unwrap_or(false);
            if is_wav {
                wavs.push(path);
            }
        }
    }
    wavs.sort();
    wavs
}

/// Retourne les plages [début, fin] en ms où le signal est silencieux.
///
/// Équivalent de `pydub.silence.detect_silence`, vectorisé : l'énergie est
/// calculée par blocs de `seek_step_ms`, puis chaque fenêtre de
/// `min_silence_len_ms` est obtenue par différence de sommes cumulées.
pub fn detect_silent_ranges(audio: &Audio, settings: &Settings) -> Vec<(u32, u32)> {
    let seg_len = audio.duration_ms;
    if seg_len < settings.min_silence_len_ms {
        return Vec::new();
    }

    let frame_rate = f64::from(audio.sample_rate);
    let block_frames = (settings.seek_step_ms as f64 * frame_rate / 1000.0).round() as usize;
    let block = block_frames * audio.channels as usize;
    if block == 0 {
        return Vec::new();
    }

    let n = audio.samples.len();
    let n_blocks = n.div_ceil(block);

    // Énergie par bloc (les échantillons manquants en fin comptent pour zéro).
    let mut energy = vec![0.0_f64; n_blocks];
    for (b, energy) in energy.iter_mut().enumerate() {
        let start = b * block;
        let end = ((b + 1) * block).min(n);
        let mut sum = 0.0_f64;
        for &x in &audio.samples[start..end] {
            let v = x as f64;
            sum += v * v;
        }
        *energy = sum;
    }

    let blocks_per_win =
        ((settings.min_silence_len_ms as f64 / settings.seek_step_ms as f64).round() as usize)
            .max(1);
    if n_blocks < blocks_per_win {
        return Vec::new();
    }
    let n_starts = n_blocks - blocks_per_win + 1;

    // Sommes cumulées -> énergie de chaque fenêtre en O(1).
    let mut cumulative = vec![0.0_f64; n_blocks + 1];
    for i in 0..n_blocks {
        cumulative[i + 1] = cumulative[i] + energy[i];
    }

    let threshold =
        db_to_amp(settings.silence_threshold_db) * max_possible_amplitude(audio.bits_per_sample);
    let window_samples = (blocks_per_win * block) as f64;

    let mut silent_blocks: Vec<usize> = Vec::new();
    for i in 0..n_starts {
        let window_energy = cumulative[i + blocks_per_win] - cumulative[i];
        let rms = (window_energy / window_samples).sqrt();
        if rms <= threshold {
            silent_blocks.push(i);
        }
    }

    if silent_blocks.is_empty() {
        return Vec::new();
    }

    // Fusion des blocs contigus puis conversion en millisecondes
    // (logique identique à pydub : `s == prev + seek_step`).
    let ms_per_block = block_frames as f64 * 1000.0 / frame_rate;
    let mut merged: Vec<(usize, usize)> = Vec::new();
    let mut first = silent_blocks[0];
    let mut prev = first;
    for &current in &silent_blocks[1..] {
        if current != prev + 1
            && (current - prev) as f64 * ms_per_block > settings.min_silence_len_ms as f64
        {
            merged.push((first, prev));
            first = current;
        }
        prev = current;
    }
    merged.push((first, prev));

    merged
        .into_iter()
        .map(|(start, end)| {
            (
                (start as f64 * ms_per_block).round() as u32,
                (end as f64 * ms_per_block + settings.min_silence_len_ms as f64).round() as u32,
            )
        })
        .collect()
}

/// Retourne les plages [début, fin] en ms de signal non silencieux.
pub fn detect_nonsilent_ranges(audio: &Audio, settings: &Settings) -> Vec<(u32, u32)> {
    let silent = detect_silent_ranges(audio, settings);
    let seg_len = audio.duration_ms;

    if silent.is_empty() {
        return vec![(0, seg_len)];
    }
    if silent[0].0 == 0 && silent[0].1 == seg_len {
        return Vec::new();
    }

    let mut out: Vec<(u32, u32)> = Vec::new();
    let mut prev_end = 0_u32;
    for &(start, end) in &silent {
        out.push((prev_end, start));
        prev_end = end;
    }
    if silent.last().map(|r| r.1).unwrap_or(0) != seg_len {
        out.push((prev_end, seg_len));
    }
    if out.first() == Some(&(0, 0)) {
        out.remove(0);
    }
    out
}

/// Découpe le signal en retirant les silences.
///
/// Retourne `None` si le fichier est entièrement silencieux (rien à garder).
pub fn trim_samples(audio: &Audio, settings: &Settings) -> Option<Vec<i32>> {
    let ranges = detect_nonsilent_ranges(audio, settings);
    if ranges.is_empty() {
        return None;
    }

    let channels = audio.channels as usize;
    let seg_len_ms = audio.duration_ms;
    let total_frames = audio.samples.len() / channels;

    let ms_to_frame = |ms: u32| {
        let frames = (ms as f64 * audio.sample_rate as f64 / 1000.0).round() as usize;
        frames.min(total_frames)
    };

    let mut out: Vec<i32> = Vec::new();
    if settings.trim_internal_silence {
        for &(start_ms, end_ms) in &ranges {
            let a_ms = start_ms.saturating_sub(settings.keep_padding_ms);
            let b_ms = (end_ms + settings.keep_padding_ms).min(seg_len_ms);
            let (a, b) = (ms_to_frame(a_ms), ms_to_frame(b_ms));
            if a < b {
                out.extend_from_slice(&audio.samples[a * channels..b * channels]);
            }
        }
    } else {
        let a_ms = ranges[0].0.saturating_sub(settings.keep_padding_ms);
        let b_ms = (ranges.last().map(|r| r.1).unwrap_or(seg_len_ms) + settings.keep_padding_ms)
            .min(seg_len_ms);
        let (a, b) = (ms_to_frame(a_ms), ms_to_frame(b_ms));
        if a < b {
            out.extend_from_slice(&audio.samples[a * channels..b * channels]);
        }
    }

    Some(out)
}

/// Nombre de millisecondes pour un certain nombre d'échantillons entrelacés.
fn samples_to_ms(sample_count: usize, sample_rate: u32, channels: u16) -> u32 {
    if sample_rate == 0 || channels == 0 {
        return 0;
    }
    (sample_count as u64 * 1000 / (u64::from(sample_rate) * u64::from(channels))) as u32
}

fn to_i16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Encode des échantillons PCM en MP3 via LAME.
pub fn encode_mp3(
    samples: &[i32],
    sample_rate: u32,
    channels: u16,
    bitrate_kbps: u32,
    output: &Path,
) -> Result<(), String> {
    if samples.is_empty() {
        return Err("aucun échantillon à encoder".to_string());
    }
    if channels != 1 && channels != 2 {
        return Err(format!("nombre de canaux non géré : {channels}"));
    }

    let pcm: Vec<i16> = samples.iter().map(|&s| to_i16(s)).collect();

    let bitrate = match bitrate_kbps {
        320 => Bitrate::Kbps320,
        256 => Bitrate::Kbps256,
        224 => Bitrate::Kbps224,
        192 => Bitrate::Kbps192,
        160 => Bitrate::Kbps160,
        128 => Bitrate::Kbps128,
        64 => Bitrate::Kbps64,
        _ => Bitrate::Kbps192,
    };

    let mut encoder = Builder::new()
        .ok_or_else(|| "impossible de créer le builder LAME".to_string())?
        .with_num_channels(channels as u8)
        .map_err(|e| format!("nombre de canaux : {e}"))?
        .with_sample_rate(sample_rate)
        .map_err(|e| format!("fréquence d'échantillonnage : {e}"))?
        .with_brate(bitrate)
        .map_err(|e| format!("bitrate : {e}"))?
        .with_quality(Quality::Good)
        .map_err(|e| format!("qualité : {e}"))?
        .build()
        .map_err(|e| format!("initialisation LAME : {e}"))?;

    let mut mp3: Vec<u8> = Vec::new();
    let per_channel = pcm.len() / channels as usize;
    mp3.reserve(mp3lame_encoder::max_required_buffer_size(per_channel.max(1)));

    let encoded = match channels {
        1 => encoder.encode(MonoPcm(&pcm), mp3.spare_capacity_mut()),
        _ => encoder.encode(InterleavedPcm(&pcm), mp3.spare_capacity_mut()),
    };
    let n = encoded.map_err(|e| format!("encodage : {e}"))?;
    unsafe {
        mp3.set_len(mp3.len() + n);
    }

    let flushed = encoder.flush::<FlushNoGap>(mp3.spare_capacity_mut());
    let n = flushed.map_err(|e| format!("finalisation : {e}"))?;
    unsafe {
        mp3.set_len(mp3.len() + n);
    }

    std::fs::write(output, &mp3).map_err(|e| format!("écriture de {output:?} : {e}"))?;
    Ok(())
}

/// Informations sur un fichier WAV, sans charger les échantillons en mémoire.
#[derive(Debug, Clone)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    /// Nombre de frames (échantillons par canal).
    pub total_frames: u64,
    pub duration_ms: u64,
}

/// Lit l'en-tête d'un fichier WAV (aucun échantillon chargé).
pub fn probe_wav(path: &Path) -> Result<WavInfo, String> {
    let reader = WavReader::open(path).map_err(|e| format!("lecture de {path:?} : {e}"))?;
    let spec = reader.spec();
    // `duration()` renvoie le nombre de frames (échantillons par canal).
    let total_frames = u64::from(reader.duration());
    let duration_ms = if spec.sample_rate > 0 {
        total_frames * 1000 / u64::from(spec.sample_rate)
    } else {
        0
    };
    Ok(WavInfo {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        bits_per_sample: spec.bits_per_sample,
        total_frames,
        duration_ms,
    })
}

/// Calcule l'énergie par bloc (somme des carrés) en lisant le fichier en flux.
///
/// Équivalent de `block_energies` mais sans jamais charger tout le fichier :
/// la mémoire utilisée est bornée par `O(nombre de blocs)`.
fn stream_block_energies(
    path: &Path,
    info: &WavInfo,
    block_frames: usize,
) -> Result<Vec<f64>, String> {
    let mut reader = WavReader::open(path).map_err(|e| e.to_string())?;
    let block = block_frames * info.channels as usize;
    let n_blocks = (info.total_frames as usize).div_ceil(block_frames);
    let mut energy = vec![0.0_f64; n_blocks];

    match info.bits_per_sample {
        16 => {
            let mut samples = reader.samples::<i16>();
            for e in energy.iter_mut() {
                let mut sum = 0.0_f64;
                for _ in 0..block {
                    match samples.next() {
                        Some(Ok(s)) => {
                            let v = f64::from(s);
                            sum += v * v;
                        }
                        _ => break,
                    }
                }
                *e = sum;
            }
        }
        24 | 32 => {
            let mut samples = reader.samples::<i32>();
            for e in energy.iter_mut() {
                let mut sum = 0.0_f64;
                for _ in 0..block {
                    match samples.next() {
                        Some(Ok(s)) => {
                            let v = s as f64;
                            sum += v * v;
                        }
                        _ => break,
                    }
                }
                *e = sum;
            }
        }
        _ => return Err(format!("format non géré : {} bits", info.bits_per_sample)),
    }

    Ok(energy)
}

/// Calcule les plages silencieuses [début, fin] (ms) à partir d'énergies par bloc.
/// (Même logique que `detect_silent_ranges`, sur des énergies déjà calculées.)
fn silent_ranges_from_energy(
    energy: &[f64],
    block_frames: usize,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    settings: &Settings,
) -> Vec<(u32, u32)> {
    let n_blocks = energy.len();
    let blocks_per_win =
        ((settings.min_silence_len_ms as f64 / settings.seek_step_ms as f64).round() as usize)
            .max(1);
    if n_blocks < blocks_per_win {
        return Vec::new();
    }
    let n_starts = n_blocks - blocks_per_win + 1;

    let mut cumulative = vec![0.0_f64; n_blocks + 1];
    for i in 0..n_blocks {
        cumulative[i + 1] = cumulative[i] + energy[i];
    }

    let block = block_frames * channels as usize;
    let threshold =
        db_to_amp(settings.silence_threshold_db) * max_possible_amplitude(bits_per_sample);
    let window_samples = (blocks_per_win * block) as f64;

    let mut silent_blocks: Vec<usize> = Vec::new();
    for i in 0..n_starts {
        let window_energy = cumulative[i + blocks_per_win] - cumulative[i];
        let rms = (window_energy / window_samples).sqrt();
        if rms <= threshold {
            silent_blocks.push(i);
        }
    }
    if silent_blocks.is_empty() {
        return Vec::new();
    }

    let ms_per_block = block_frames as f64 * 1000.0 / sample_rate as f64;
    let mut merged: Vec<(usize, usize)> = Vec::new();
    let mut first = silent_blocks[0];
    let mut prev = first;
    for &current in &silent_blocks[1..] {
        if current != prev + 1
            && (current - prev) as f64 * ms_per_block > settings.min_silence_len_ms as f64
        {
            merged.push((first, prev));
            first = current;
        }
        prev = current;
    }
    merged.push((first, prev));

    merged
        .into_iter()
        .map(|(start, end)| {
            (
                (start as f64 * ms_per_block).round() as u32,
                (end as f64 * ms_per_block + settings.min_silence_len_ms as f64).round() as u32,
            )
        })
        .collect()
}

/// Inverse des plages silencieuses : renvoie les plages non silencieuses.
fn nonsilent_from_silent(silent: &[(u32, u32)], seg_len: u32) -> Vec<(u32, u32)> {
    if silent.is_empty() {
        return vec![(0, seg_len)];
    }
    if silent[0].0 == 0 && silent[0].1 == seg_len {
        return Vec::new();
    }

    let mut out: Vec<(u32, u32)> = Vec::new();
    let mut prev_end = 0_u32;
    for &(start, end) in silent {
        out.push((prev_end, start));
        prev_end = end;
    }
    if silent.last().map(|r| r.1).unwrap_or(0) != seg_len {
        out.push((prev_end, seg_len));
    }
    if out.first() == Some(&(0, 0)) {
        out.remove(0);
    }
    out
}

/// Fenêtre [frame_début, frame_fin) à conserver, ou `None` si tout est silencieux.
fn trim_window_streaming(
    energy: &[f64],
    info: &WavInfo,
    block_frames: usize,
    settings: &Settings,
) -> Option<(u64, u64)> {
    let seg_len_ms = info.duration_ms as u32;
    if seg_len_ms < settings.min_silence_len_ms {
        return Some((0, info.total_frames));
    }

    let silent = silent_ranges_from_energy(
        energy,
        block_frames,
        info.sample_rate,
        info.channels,
        info.bits_per_sample,
        settings,
    );
    let ranges = nonsilent_from_silent(&silent, seg_len_ms);
    if ranges.is_empty() {
        return None;
    }

    let frame_rate = f64::from(info.sample_rate);
    let ms_to_frame = |ms: u32| {
        let frames = (ms as f64 * frame_rate / 1000.0).round() as u64;
        frames.min(info.total_frames)
    };

    let a_ms = ranges[0].0.saturating_sub(settings.keep_padding_ms);
    let b_ms = (ranges.last().map(|r| r.1).unwrap_or(seg_len_ms) + settings.keep_padding_ms)
        .min(seg_len_ms);
    let (a, b) = (ms_to_frame(a_ms), ms_to_frame(b_ms));
    if a < b {
        Some((a, b))
    } else {
        None
    }
}

fn bitrate(kbps: u32) -> Bitrate {
    match kbps {
        320 => Bitrate::Kbps320,
        256 => Bitrate::Kbps256,
        224 => Bitrate::Kbps224,
        192 => Bitrate::Kbps192,
        160 => Bitrate::Kbps160,
        128 => Bitrate::Kbps128,
        64 => Bitrate::Kbps64,
        _ => Bitrate::Kbps192,
    }
}

fn build_encoder(
    sample_rate: u32,
    channels: u16,
    brate: Bitrate,
) -> Result<mp3lame_encoder::Encoder, String> {
    Builder::new()
        .ok_or_else(|| "impossible de créer le builder LAME".to_string())?
        .with_num_channels(channels as u8)
        .map_err(|e| format!("nombre de canaux : {e}"))?
        .with_sample_rate(sample_rate)
        .map_err(|e| format!("fréquence d'échantillonnage : {e}"))?
        .with_brate(brate)
        .map_err(|e| format!("bitrate : {e}"))?
        .with_quality(Quality::Good)
        .map_err(|e| format!("qualité : {e}"))?
        .build()
        .map_err(|e| format!("initialisation LAME : {e}"))
}

/// Encode un bloc PCM en MP3 et l'écrit dans le fichier de sortie.
fn encode_chunk(
    encoder: &mut mp3lame_encoder::Encoder,
    writer: &mut std::io::BufWriter<std::fs::File>,
    pcm: &[i16],
    channels: usize,
) -> Result<(), String> {
    if pcm.is_empty() {
        return Ok(());
    }
    let mut out = Vec::new();
    out.reserve(mp3lame_encoder::max_required_buffer_size(pcm.len() / channels).max(1));
    let encoded = match channels {
        1 => encoder.encode(MonoPcm(pcm), out.spare_capacity_mut()),
        _ => encoder.encode(InterleavedPcm(pcm), out.spare_capacity_mut()),
    };
    let n = encoded.map_err(|e| format!("encodage : {e}"))?;
    unsafe {
        out.set_len(n);
    }
    std::io::Write::write_all(writer, &out).map_err(|e| format!("écriture : {e}"))
}

/// Encode la région [start_frame, end_frame) du fichier en MP3, en flux.
fn encode_region_streaming(
    path: &Path,
    info: &WavInfo,
    start_frame: u64,
    end_frame: u64,
    settings: &Settings,
    output: &Path,
) -> Result<(), String> {
    const FRAMES_PER_CHUNK: usize = 23_040; // 1152 * 20

    let mut reader = WavReader::open(path).map_err(|e| e.to_string())?;
    // Positionne directement le lecteur au début de la zone à encoder (au lieu
    // de relire tout le début du fichier via `.skip`).
    reader
        .seek(start_frame as u32)
        .map_err(|e| format!("positionnement dans le WAV : {e}"))?;
    let mut encoder = build_encoder(info.sample_rate, info.channels, bitrate(settings.bitrate_kbps))?;
    let file = std::fs::File::create(output).map_err(|e| format!("écriture de {output:?} : {e}"))?;
    let mut writer = std::io::BufWriter::new(file);

    let channels = info.channels as usize;
    let total = ((end_frame - start_frame) as usize) * channels;
    let chunk = FRAMES_PER_CHUNK * channels;
    let mut buf: Vec<i16> = Vec::with_capacity(chunk);
    let mut remaining = total;

    match info.bits_per_sample {
        16 => {
            for sample in reader.samples::<i16>() {
                if remaining == 0 {
                    break;
                }
                buf.push(sample.map_err(|e| e.to_string())?);
                remaining -= 1;
                if buf.len() == chunk {
                    encode_chunk(&mut encoder, &mut writer, &buf, channels)?;
                    buf.clear();
                }
            }
        }
        24 | 32 => {
            for sample in reader.samples::<i32>() {
                if remaining == 0 {
                    break;
                }
                buf.push(to_i16(sample.map_err(|e| e.to_string())?));
                remaining -= 1;
                if buf.len() == chunk {
                    encode_chunk(&mut encoder, &mut writer, &buf, channels)?;
                    buf.clear();
                }
            }
        }
        _ => return Err(format!("format non géré : {} bits", info.bits_per_sample)),
    }

    if !buf.is_empty() {
        encode_chunk(&mut encoder, &mut writer, &buf, channels)?;
    }

    let mut out = Vec::new();
    out.reserve(7200);
    let n = encoder
        .flush::<FlushNoGap>(out.spare_capacity_mut())
        .map_err(|e| format!("finalisation : {e}"))?;
    unsafe {
        out.set_len(n);
    }
    std::io::Write::write_all(&mut writer, &out).map_err(|e| format!("écriture : {e}"))?;
    std::io::Write::flush(&mut writer).map_err(|e| format!("écriture : {e}"))?;

    Ok(())
}

/// Traite un fichier WAV en flux (mémoire bornée), sauf pour la coupe des
/// silences internes qui repasse par le chemin en mémoire.
fn process_file_streaming(
    input: &Path,
    output: &Path,
    settings: &Settings,
) -> Result<ProcessStats, String> {
    let info = probe_wav(input)?;
    let original_ms = info.duration_ms as u32;

    if info.channels != 1 && info.channels != 2 {
        return Err(format!("nombre de canaux non géré : {}", info.channels));
    }
    if !matches!(info.bits_per_sample, 16 | 24 | 32) {
        return Err(format!("format non géré : {} bits", info.bits_per_sample));
    }

    let frame_rate = f64::from(info.sample_rate);
    let block_frames = (settings.seek_step_ms as f64 * frame_rate / 1000.0).round() as usize;
    if block_frames == 0 {
        return Err("seek_step_ms invalide".to_string());
    }

    let energy = stream_block_energies(input, &info, block_frames)?;
    let (start_frame, end_frame) =
        match trim_window_streaming(&energy, &info, block_frames, settings) {
            Some(window) => window,
            None => {
                return Ok(ProcessStats {
                    original_ms,
                    kept_ms: original_ms,
                    all_silent: true,
                    output_path: output.display().to_string(),
                })
            }
        };

    encode_region_streaming(input, &info, start_frame, end_frame, settings, output)?;

    let kept_ms = if info.sample_rate > 0 {
        ((end_frame - start_frame) * 1000 / u64::from(info.sample_rate)) as u32
    } else {
        0
    };
    Ok(ProcessStats {
        original_ms,
        kept_ms,
        all_silent: false,
        output_path: output.display().to_string(),
    })
}

/// Traite un fichier WAV : lit, tronque les silences, exporte en MP3.
///
/// Le chemin par défaut est **en flux** (mémoire bornée, adapté aux longs
/// enregistrements). La coupe des silences internes, plus rare, repasse par le
/// chemin en mémoire (plus simple à concaténer correctement).
pub fn process_file(input: &Path, output: &Path, settings: &Settings) -> Result<ProcessStats, String> {
    if settings.trim_internal_silence {
        process_file_in_memory(input, output, settings)
    } else {
        process_file_streaming(input, output, settings)
    }
}

/// Chemin en mémoire (utilisé pour `trim_internal_silence` et pour les tests).
fn process_file_in_memory(
    input: &Path,
    output: &Path,
    settings: &Settings,
) -> Result<ProcessStats, String> {
    let audio = read_wav(input)?;
    let original_ms = audio.duration_ms;

    match trim_samples(&audio, settings) {
        None => Ok(ProcessStats {
            original_ms,
            kept_ms: original_ms,
            all_silent: true,
            output_path: output.display().to_string(),
        }),
        Some(trimmed) => {
            encode_mp3(
                &trimmed,
                audio.sample_rate,
                audio.channels,
                settings.bitrate_kbps,
                output,
            )?;
            let kept_ms = samples_to_ms(trimmed.len(), audio.sample_rate, audio.channels);
            Ok(ProcessStats {
                original_ms,
                kept_ms,
                all_silent: false,
                output_path: output.display().to_string(),
            })
        }
    }
}
