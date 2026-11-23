//! Dictation: the default microphone, recorded here (cpal), transcribed on this Mac by the Whisper
//! model the app bundles (whisper.cpp, Metal). Nothing leaves the machine.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tauri::{AppHandle, Emitter, Manager};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::CmdResult;

const MODEL: &str = "ggml-large-v3-turbo-q5_0.bin";
const RATE: u32 = 16_000;
/// Ten minutes of speech is a long spec; past it the recording stops growing.
const MAX_SECONDS: usize = 600;

#[derive(Default)]
pub struct Dictation {
    rec: Mutex<Option<Recording>>,
    model: Arc<Mutex<Option<Arc<WhisperContext>>>>,
}

struct Recording {
    stop: mpsc::Sender<()>,
    done: mpsc::Receiver<Result<Captured>>,
}

struct Captured {
    samples: Vec<f32>,
    rate: u32,
}

/// The bundled model; under `tauri dev` the resources are copied next to the binary too, and a
/// debug build falls back to the source tree.
fn model_path(app: &AppHandle) -> Result<PathBuf> {
    let mut tries = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        tries.push(dir.join("whisper").join(MODEL));
    }
    if cfg!(debug_assertions) {
        tries.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/whisper").join(MODEL));
    }
    tries
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow!("The Whisper model is not in this build; run `node scripts/whisper-model.mjs` and build again"))
}

fn load(app: &AppHandle, slot: &Mutex<Option<Arc<WhisperContext>>>) -> Result<Arc<WhisperContext>> {
    let mut g = slot.lock().unwrap();
    if let Some(m) = g.as_ref() {
        return Ok(m.clone());
    }
    let path = model_path(app)?;
    let ctx = WhisperContext::new_with_params(&path, WhisperContextParameters::default())
        .with_context(|| format!("loading {}", path.display()))?;
    let ctx = Arc::new(ctx);
    *g = Some(ctx.clone());
    Ok(ctx)
}

impl Dictation {
    /// Opens the microphone. The model loads meanwhile, so stopping goes straight to the words.
    pub fn start(&self, app: &AppHandle) -> Result<()> {
        let mut rec = self.rec.lock().unwrap();
        if rec.is_some() {
            bail!("Already listening");
        }
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<Result<Captured>>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let app2 = app.clone();
        // cpal's stream cannot cross threads on macOS: one thread owns it from open to close.
        std::thread::spawn(move || {
            let opened = open_mic();
            let (stream, buf, level, rate) = match opened {
                Ok(x) => x,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            loop {
                match stop_rx.recv_timeout(Duration::from_millis(80)) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let _ = app2.emit("dictate-level", f32::from_bits(level.load(Ordering::Relaxed)));
                    }
                    _ => break,
                }
            }
            drop(stream);
            let samples = std::mem::take(&mut *buf.lock().unwrap());
            let _ = done_tx.send(Ok(Captured { samples, rate }));
        });
        ready_rx.recv().map_err(|_| anyhow!("The microphone thread ended"))??;
        *rec = Some(Recording { stop: stop_tx, done: done_rx });

        let (app3, slot) = (app.clone(), self.model.clone());
        std::thread::spawn(move || {
            let _ = load(&app3, &slot);
        });
        Ok(())
    }

    /// Closes the microphone and returns what was said, or "" when nothing was heard.
    pub fn stop(&self, app: &AppHandle) -> Result<String> {
        let rec = self.rec.lock().unwrap().take().ok_or_else(|| anyhow!("Not listening"))?;
        let _ = rec.stop.send(());
        let cap = rec.done.recv().map_err(|_| anyhow!("The microphone thread ended"))??;
        let audio = resample(&cap.samples, cap.rate);
        // Silence (or a microphone macOS keeps muted) makes Whisper invent words: say nothing.
        if audio.len() < RATE as usize / 4 || rms(&audio) < 0.004 {
            return Ok(String::new());
        }
        let ctx = load(app, &self.model)?;
        transcribe(&ctx, &audio)
    }

    pub fn cancel(&self) {
        if let Some(rec) = self.rec.lock().unwrap().take() {
            let _ = rec.stop.send(());
        }
    }

    /// Frees the model's Metal buffers. ggml aborts the process when its Metal device is torn down
    /// at exit with buffers still alive, which macOS reports as a crash. False when a transcription
    /// still holds the model, so it could not be freed.
    pub fn release(&self) -> bool {
        self.cancel();
        let taken = self.model.lock().map(|mut g| g.take()).unwrap_or(None);
        match taken {
            Some(ctx) => Arc::try_unwrap(ctx).is_ok(),
            None => true,
        }
    }
}

type Mic = (cpal::Stream, Arc<Mutex<Vec<f32>>>, Arc<AtomicU32>, u32);

fn open_mic() -> Result<Mic> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or_else(|| anyhow!("No microphone found"))?;
    let supported = device.default_input_config().context("reading the microphone's format")?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();
    let (rate, channels) = (config.sample_rate, config.channels as usize);
    let buf = Arc::new(Mutex::new(Vec::<f32>::with_capacity(rate as usize * 30)));
    let level = Arc::new(AtomicU32::new(0));
    let cap = rate as usize * MAX_SECONDS;

    fn sink<T: cpal::Sample + 'static>(
        channels: usize,
        cap: usize,
        buf: Arc<Mutex<Vec<f32>>>,
        level: Arc<AtomicU32>,
        to_f32: fn(T) -> f32,
    ) -> impl FnMut(&[T], &cpal::InputCallbackInfo) + Send + 'static {
        move |data, _| {
            let mut b = buf.lock().unwrap();
            let mut sum = 0.0f32;
            let mut n = 0usize;
            for frame in data.chunks(channels.max(1)) {
                let v = frame.iter().map(|s| to_f32(*s)).sum::<f32>() / frame.len() as f32;
                sum += v * v;
                n += 1;
                if b.len() < cap {
                    b.push(v);
                }
            }
            if n > 0 {
                level.store((sum / n as f32).sqrt().to_bits(), Ordering::Relaxed);
            }
        }
    }
    let err = |e: cpal::Error| eprintln!("dictate: {e}");
    let stream = match format {
        cpal::SampleFormat::F32 => device.build_input_stream(config, sink::<f32>(channels, cap, buf.clone(), level.clone(), |s| s), err, None),
        cpal::SampleFormat::I16 => device.build_input_stream(config, sink::<i16>(channels, cap, buf.clone(), level.clone(), |s| s as f32 / 32768.0), err, None),
        cpal::SampleFormat::I32 => device.build_input_stream(config, sink::<i32>(channels, cap, buf.clone(), level.clone(), |s| s as f32 / 2_147_483_648.0), err, None),
        cpal::SampleFormat::U16 => device.build_input_stream(config, sink::<u16>(channels, cap, buf.clone(), level.clone(), |s| (s as f32 - 32768.0) / 32768.0), err, None),
        f => bail!("The microphone sends {f:?} samples, which dictation does not read"),
    }
    .context("opening the microphone")?;
    stream.play().context("starting the microphone")?;
    Ok((stream, buf, level, rate))
}

/// Linear resampling to Whisper's 16 kHz: plenty for speech.
fn resample(x: &[f32], from: u32) -> Vec<f32> {
    if from == RATE || x.is_empty() {
        return x.to_vec();
    }
    let ratio = from as f64 / RATE as f64;
    let n = (x.len() as f64 / ratio) as usize;
    (0..n)
        .map(|i| {
            let p = i as f64 * ratio;
            let j = p as usize;
            let t = (p - j as f64) as f32;
            let a = x[j];
            let b = *x.get(j + 1).unwrap_or(&a);
            a + (b - a) * t
        })
        .collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn transcribe(ctx: &WhisperContext, audio: &[f32]) -> Result<String> {
    let mut state = ctx.create_state().context("starting Whisper")?;
    let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    p.set_language(Some("auto"));
    p.set_n_threads(std::thread::available_parallelism().map(|n| n.get().min(8) as i32).unwrap_or(4));
    p.set_no_context(true);
    p.set_print_progress(false);
    p.set_print_realtime(false);
    p.set_print_timestamps(false);
    p.set_print_special(false);
    state.full(p, audio).context("transcribing")?;
    let text = state
        .as_iter()
        .filter_map(|s| s.to_str_lossy().ok().map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Ok(text)
}

/// Opening the microphone waits on the device (and, the first time, on macOS asking for it), so
/// off the main thread: the window keeps drawing meanwhile.
#[tauri::command]
pub(crate) async fn dictate_start(app: tauri::AppHandle) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || app.state::<Dictation>().start(&app))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

/// Blocking work (the transcription), so off the main thread.
#[tauri::command]
pub(crate) async fn dictate_stop(app: tauri::AppHandle) -> CmdResult<String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<Dictation>().stop(&app))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub(crate) fn dictate_cancel(d: tauri::State<'_, Dictation>) {
    d.cancel()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p pitlane-desktop dictate -- --ignored`: macOS `say` speaks, Whisper writes it back.
    #[test]
    #[ignore = "needs the bundled model and macOS say"]
    fn transcribes_spoken_portuguese() {
        let dir = std::env::temp_dir().join("pitlane-dictate-test");
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("say.wav");
        let ok = std::process::Command::new("say")
            .args(["-v", "Luciana", "-o"])
            .arg(&wav)
            .args(["--data-format=LEF32@16000", "Adicionar um botão para rodar todos os cards do backlog."])
            .status()
            .unwrap();
        assert!(ok.success());
        let bytes = std::fs::read(&wav).unwrap();
        let at = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
        let audio: Vec<f32> = bytes[at..].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let model = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/whisper").join(MODEL);
        let ctx = WhisperContext::new_with_params(&model, WhisperContextParameters::default()).unwrap();
        let t0 = std::time::Instant::now();
        let text = transcribe(&ctx, &audio).unwrap();
        eprintln!("{:.2}s: {text}", t0.elapsed().as_secs_f32());
        assert!(text.to_lowercase().contains("backlog"), "{text}");
    }
}
