//! CLI SilenceLess : tronque les silences des .wav d'un dossier et exporte en .mp3.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use silence_less_core::{process_file, Settings};

struct Args {
    folder: PathBuf,
    out: Option<PathBuf>,
    jobs: usize,
    settings: Settings,
}

fn parse_args() -> Result<Args, String> {
    let mut settings = Settings::default();
    let mut folder: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut jobs = 0_usize;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| -> Result<String, String> {
            args.next()
                .ok_or_else(|| format!("{name} attend une valeur"))
        };

        match arg.as_str() {
            "-o" | "--out" => out = Some(PathBuf::from(value("--out")?)),
            "-j" | "--jobs" => {
                jobs = value("--jobs")?.parse().map_err(|_| "--jobs invalide".to_string())?
            }
            "--seek-step" => {
                settings.seek_step_ms = value("--seek-step")?
                    .parse()
                    .map_err(|_| "--seek-step invalide".to_string())?
            }
            "--min-silence" => {
                settings.min_silence_len_ms = value("--min-silence")?
                    .parse()
                    .map_err(|_| "--min-silence invalide".to_string())?
            }
            "--threshold" => {
                settings.silence_threshold_db = value("--threshold")?
                    .parse()
                    .map_err(|_| "--threshold invalide".to_string())?
            }
            "--padding" => {
                settings.keep_padding_ms = value("--padding")?
                    .parse()
                    .map_err(|_| "--padding invalide".to_string())?
            }
            "--bitrate" => {
                settings.bitrate_kbps = value("--bitrate")?
                    .parse()
                    .map_err(|_| "--bitrate invalide".to_string())?
            }
            "--trim-internal" => settings.trim_internal_silence = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            s if s.starts_with('-') && s.len() > 1 => {
                return Err(format!("option inconnue : {s}"));
            }
            s => {
                if folder.is_some() {
                    return Err("un seul dossier attendu".to_string());
                }
                folder = Some(PathBuf::from(s));
            }
        }
    }

    Ok(Args {
        folder: folder.ok_or_else(|| "dossier manquant (ou --help)".to_string())?,
        out,
        jobs,
        settings,
    })
}

fn print_help() {
    println!(
        "SilenceLess — tronque les silences des .wav et exporte en .mp3\n\
         \n\
         Usage : silence-less [OPTIONS] <DOSSIER>\n\
         \n\
         Options :\n\
         \x20 -o, --out <DOSSIER>     dossier de sortie (défaut : <DOSSIER>/mp3_output)\n\
         \x20 -j, --jobs <N>          fichiers traités en parallèle (0 = auto)\n\
         \x20     --seek-step <ms>    pas de détection des silences (défaut : 10)\n\
         \x20     --min-silence <ms>  durée minimale d'un silence (défaut : 500)\n\
         \x20     --threshold <dB>    seuil de silence en dBFS (défaut : -40)\n\
         \x20     --padding <ms>      marge conservée autour du son (défaut : 100)\n\
         \x20     --bitrate <kbps>    bitrate MP3 (défaut : 192)\n\
         \x20     --trim-internal     coupe aussi les silences à l'intérieur\n\
         \x20 -h, --help              affiche cette aide"
    );
}

fn collect_wavs(folder: &Path) -> Vec<PathBuf> {
    let mut wavs: Vec<PathBuf> = Vec::new();
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

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("Erreur : {e}");
            return ExitCode::from(2);
        }
    };

    if !args.folder.is_dir() {
        eprintln!("Dossier introuvable : {}", args.folder.display());
        return ExitCode::from(1);
    }

    let out = args
        .out
        .unwrap_or_else(|| args.folder.join("mp3_output"));
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("Impossible de créer {} : {e}", out.display());
        return ExitCode::from(1);
    }

    let wavs = collect_wavs(&args.folder);
    if wavs.is_empty() {
        println!("Aucun fichier .wav trouvé dans {}", args.folder.display());
        return ExitCode::SUCCESS;
    }

    println!("{} fichier(s) .wav trouvé(s)", wavs.len());

    let workers = if args.jobs > 0 {
        args.jobs
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    }
    .min(wavs.len());
    if workers > 1 {
        println!("Traitement en parallèle : {workers} fichiers\n");
    } else {
        println!();
    }

    let settings = &args.settings;
    let out = &out;
    let results: Vec<Result<silence_less_core::ProcessStats, String>> =
        std::thread::scope(|scope| {
            let handles: Vec<_> = wavs
                .iter()
                .map(|wav| {
                    scope.spawn(move || {
                        let mp3 = out.join(format!(
                            "{}.mp3",
                            wav.file_stem().unwrap_or_default().to_string_lossy()
                        ));
                        process_file(wav, &mp3, settings)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

    let mut failures = 0_usize;
    for (wav, result) in wavs.iter().zip(results) {
        let name = wav.file_name().unwrap_or_default().to_string_lossy();
        match result {
            Ok(stats) if stats.all_silent => {
                println!("  {name} : entièrement silencieux, ignoré");
            }
            Ok(stats) => {
                println!(
                    "  {name} : {:.2}s (silence retiré : {:.2}s) -> {}",
                    stats.kept_ms as f64 / 1000.0,
                    (stats.original_ms.saturating_sub(stats.kept_ms)) as f64 / 1000.0,
                    stats.output_path
                );
            }
            Err(e) => {
                failures += 1;
                println!("  {name} : ERREUR — {e}");
            }
        }
    }

    if failures > 0 {
        println!("\nTerminé ({failures} fichier(s) en erreur).");
        ExitCode::from(1)
    } else {
        println!("\nTerminé.");
        ExitCode::SUCCESS
    }
}
