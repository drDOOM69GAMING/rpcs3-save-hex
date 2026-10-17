//! A short "done" chime for the GUI.
//!
//! The sound is generated in-process (a small WAV written to the temporary
//! directory), so the binary stays self-contained: no audio assets are bundled
//! and no extra crates are needed. Playback is handed to whichever system player
//! is installed and is always best-effort: if there is no audio output the save
//! still gets written and the UI still shows the confirmation, only the chime is
//! skipped.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;

/// Sample rate of the generated chime.
const SAMPLE_RATE: u32 = 44_100;

/// Players to try, in order. The first one found on `PATH` is used.
static PLAYERS: &[(&str, &[&str])] = &[
    ("paplay", &[]),
    ("pw-play", &[]),
    ("aplay", &["-q"]),
    ("play", &["-q"]),
    ("canberra-gtk-play", &["-f"]),
    ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
];

/// Play the short confirmation chime.
///
/// Returns immediately: the file is written and the player is launched on a
/// detached thread, and the child is reaped there so a long-running GUI does not
/// collect zombie processes. Every failure is swallowed on purpose.
pub fn play_done() {
    std::thread::spawn(|| {
        let Some(wav) = chime_path() else {
            return;
        };
        let Some((program, args)) = player() else {
            return;
        };
        let mut cmd = Command::new(program);
        cmd.args(args)
            .arg(&wav)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(mut child) = cmd.spawn() {
            reap(&mut child);
        }
    });
}

fn reap(child: &mut Child) {
    let _ = child.wait();
}

/// The first player present on `PATH`, resolved once.
fn player() -> Option<(&'static str, &'static [&'static str])> {
    static PLAYER: OnceLock<Option<(&'static str, &'static [&'static str])>> = OnceLock::new();
    *PLAYER.get_or_init(|| {
        PLAYERS
            .iter()
            .find(|(program, _)| on_path(program))
            .copied()
    })
}

/// Whether `program` is an executable on `PATH`.
fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file()
    })
}

/// Path to the generated chime, writing it out on first use.
fn chime_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = std::env::temp_dir().join("rpcs3-save-editor-done.wav");
        let want = chime_wav();
        if std::fs::read(&path).map(|got| got == want).unwrap_or(false) {
            return Some(path);
        }
        std::fs::write(&path, &want).ok().map(|_| path)
    })
    .clone()
}

/// Two rising notes with a soft bell envelope, as a 16-bit mono WAV.
fn chime_wav() -> Vec<u8> {
    // (frequency in Hz, duration in milliseconds)
    let notes = [(880.0_f64, 95.0_f64), (1318.51_f64, 140.0_f64)];
    let total: usize = notes
        .iter()
        .map(|(_, ms)| (SAMPLE_RATE as f64 * ms / 1000.0) as usize)
        .sum();
    let mut samples: Vec<i16> = Vec::with_capacity(total);
    for (freq, ms) in notes {
        let count = (SAMPLE_RATE as f64 * ms / 1000.0) as usize;
        for i in 0..count {
            let t = i as f64 / SAMPLE_RATE as f64;
            let pos = i as f64 / count as f64;
            // Quick attack, smooth decay: a soft bell instead of a click.
            let attack = (pos / 0.02).min(1.0);
            let decay = (-4.0 * pos).exp();
            let value = (2.0 * std::f64::consts::PI * freq * t).sin() * attack * decay * 0.28;
            samples.push((value * i16::MAX as f64) as i16);
        }
    }
    wav_bytes(&samples)
}

/// Wrap samples in a minimal RIFF/WAVE container.
fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_is_a_valid_wav() {
        let wav = chime_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        let declared = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as usize;
        assert_eq!(declared, wav.len() - 44, "data chunk must cover the payload");
        assert!(declared > 0);
        assert_eq!(declared % 2, 0, "16-bit samples should be an even byte count");
    }

    #[test]
    fn chime_file_is_written_once() {
        let a = chime_path().expect("temp dir should be writable");
        assert!(a.is_file());
        let bytes = std::fs::read(&a).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        // A second call reuses the same path.
        assert_eq!(chime_path().as_deref(), Some(a.as_path()));
    }

    #[test]
    fn chime_starts_and_ends_quietly() {
        let wav = chime_wav();
        let declared = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as usize;
        let samples: Vec<i16> = wav[44..44 + declared]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        // The envelope means the first and last samples are near silence, which
        // is what keeps the chime from clicking.
        assert!(samples[0].abs() < 200);
        assert!(samples[samples.len() - 1].abs() < 200);
        // And it does get loud in the middle.
        assert!(samples.iter().map(|s| s.abs()).max().unwrap() > 3_000);
    }
}
