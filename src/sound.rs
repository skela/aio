use std::io::Write;
use std::io::{Read, Result as IoResult};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const AUDIO_PLAYER_TIMEOUT: Duration = Duration::from_secs(15);
const AUDIO_PLAYER_POLL_INTERVAL: Duration = Duration::from_millis(25);

static SOUND_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static SOUND_DONE: &[u8] = include_bytes!("../assets/sounds/done.mp3");

/// Play the done sound in a background thread.
/// Silently does nothing if no audio player is available.
pub fn play() {
    std::thread::spawn(|| {
        let _ = play_bytes(SOUND_DONE);
    });
}

fn play_bytes(data: &[u8]) -> Result<(), String> {
    let tmp = temp_sound_path();
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(data).map_err(|e| e.to_string())?;
    drop(file);

    let result = run_player(&tmp);
    let _ = std::fs::remove_file(&tmp);

    match result {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(playback_error(&output)),
        Err(e) => Err(e),
    }
}

fn temp_sound_path() -> PathBuf {
    let id = SOUND_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("aio-sound-{}-{id}.mp3", std::process::id()))
}

fn playback_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("player exited with {}", output.status)
    } else {
        format!("player exited with {}: {stderr}", output.status)
    }
}

#[derive(Debug, Clone, Copy)]
struct AudioPlayer {
    program: &'static str,
    args: &'static [&'static str],
}

impl AudioPlayer {
    fn output(self, path: &Path) -> std::io::Result<Output> {
        let mut child = Command::new(self.program)
            .args(self.args)
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let Some(stdout) = child.stdout.take() else {
            terminate_and_reap(&mut child)?;
            return Err(std::io::Error::other("audio player stdout was not piped"));
        };
        let Some(stderr) = child.stderr.take() else {
            terminate_and_reap(&mut child)?;
            return Err(std::io::Error::other("audio player stderr was not piped"));
        };

        let stdout_reader = read_output(stdout);
        let stderr_reader = read_output(stderr);
        let deadline = Instant::now() + AUDIO_PLAYER_TIMEOUT;

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let (stdout, stderr) = finish_output(stdout_reader, stderr_reader)?;
                    return Ok(Output { status, stdout, stderr });
                }
                Ok(None) => {}
                Err(wait_err) => {
                    let cleanup_result = terminate_and_reap(&mut child);
                    let _ = finish_output(stdout_reader, stderr_reader);
                    cleanup_result?;
                    return Err(wait_err);
                }
            }

            let now = Instant::now();
            if now >= deadline {
                let cleanup_result = terminate_and_reap(&mut child);
                let _ = finish_output(stdout_reader, stderr_reader);
                cleanup_result?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{} playback timed out after {AUDIO_PLAYER_TIMEOUT:?}", self.program),
                ));
            }

            std::thread::sleep((deadline - now).min(AUDIO_PLAYER_POLL_INTERVAL));
        }
    }
}

fn read_output<R>(mut reader: R) -> std::thread::JoinHandle<IoResult<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        Ok(output)
    })
}

fn finish_output(
    stdout_reader: std::thread::JoinHandle<IoResult<Vec<u8>>>,
    stderr_reader: std::thread::JoinHandle<IoResult<Vec<u8>>>,
) -> IoResult<(Vec<u8>, Vec<u8>)> {
    let stdout = stdout_reader
        .join()
        .map_err(|_| std::io::Error::other("audio player stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| std::io::Error::other("audio player stderr reader panicked"))??;
    Ok((stdout, stderr))
}

fn terminate_and_reap(child: &mut std::process::Child) -> std::io::Result<()> {
    if let Err(kill_err) = child.kill() {
        if child.try_wait()?.is_none() {
            return Err(kill_err);
        }
    }
    child.wait().map(|_| ())
}

fn linux_audio_players() -> &'static [AudioPlayer] {
    // Do not add bare aplay — it does not decode MP3 and plays bytes as raw PCM.
    &[
        AudioPlayer { program: "paplay",  args: &[] },
        AudioPlayer { program: "pw-play", args: &[] },
        AudioPlayer { program: "ffplay",  args: &["-nodisp", "-autoexit", "-loglevel", "quiet"] },
        AudioPlayer { program: "mpg123",  args: &["-q"] },
        AudioPlayer { program: "mpv",     args: &["--no-video", "--really-quiet"] },
    ]
}

fn run_player(path: &Path) -> Result<Output, String> {
    let mut errors = Vec::new();

    for player in linux_audio_players() {
        match player.output(path) {
            Ok(output) if output.status.success() => return Ok(output),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stderr = stderr.trim();
                if stderr.is_empty() {
                    errors.push(format!("{} exited with {}", player.program, output.status));
                } else {
                    errors.push(format!("{} exited with {}: {stderr}", player.program, output.status));
                }
            }
            Err(err) => errors.push(format!("{} failed: {err}", player.program)),
        }
    }

    Err(format!(
        "no mp3-capable audio player available: {}",
        errors.join("; ")
    ))
}
