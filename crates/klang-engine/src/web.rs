//! Engine im Browser – dieselbe Schnittstelle wie auf dem Desktop
//! (`device.rs`), aber ohne Gerät und ohne Threads:
//!
//! * Gemischt wird in einem AudioWorklet: Dort läuft derselbe [`Renderer`]
//!   als [`crate::Mixer`] (eigene WebAssembly-Instanz).
//! * Die Engine schickt ihm dieselben Nachrichten ([`RtMsg`]) als Zahlenfolge
//!   ([`crate::wire`]); Klangpuffer gehen einmal je Puffer hinüber und werden
//!   danach nur noch über ihre Kennung genannt.
//! * Status (Position, Pegel) und Mikrofon-Samples meldet das Worklet zurück;
//!   der Web-Kern reicht sie mit [`report`] und [`record_input`] herein.
//! * Was auf dem Desktop der Engine-Thread nebenbei erledigt (gestreamtes
//!   Vorhören nachfüllen), macht hier [`Engine::tick`], vom Web-Kern
//!   regelmäßig aufgerufen.
//!
//! Die Verbindung zum Worklet ([`WebPort`]) setzt der Web-Kern mit
//! [`connect`], bevor er die Engine startet.

use crate::wire::{self, Kind};
use klang_i18n::tr;
use crate::{update_peak, Arrangement, AudioDevices, EngineError, EngineInfo, EngineStatus, PlacedClip, PreviewSource, PreviewStream, Recording, RtMsg, Shared, STREAM_AHEAD_SECS};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Verbindung zur Audio-Ausgabe im Browser.
#[derive(Clone)]
pub struct WebPort {
    pub sample_rate: u32,
    /// Name für die Oberfläche („Web Audio …“).
    pub device: String,
    /// Nachricht an das Worklet.
    pub post: fn(Kind, &[f64]),
    /// Klangpuffer an das Worklet (Kennung, Samples).
    pub buffer: fn(u32, &[f32]),
    /// Monotone Uhr in Sekunden.
    pub now: fn() -> f64,
}

static PORT: Mutex<Option<WebPort>> = Mutex::new(None);

static SHARED: Shared = Shared {
    playing: AtomicBool::new(false),
    previewing: AtomicBool::new(false),
    recording: AtomicBool::new(false),
    position: AtomicU64::new(0),
    peak_out: AtomicU32::new(0),
    peak_in: AtomicU32::new(0),
    muted: AtomicBool::new(false),
    sample_rate: AtomicU32::new(0),
};

/// Mikrofon-Samples der laufenden Aufnahme.
static REC: Mutex<Option<Vec<f32>>> = Mutex::new(None);

/// Verbindung zum Worklet herstellen (vor [`Engine::start`]).
pub fn connect(port: WebPort) {
    *PORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(port);
}

fn port() -> Option<WebPort> {
    PORT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Status aus dem Worklet (wie die Atomics, die auf dem Desktop der
/// Audio-Callback setzt). Spitzenpegel sammeln sich bis zum nächsten Lesen.
pub fn report(playing: bool, previewing: bool, position_frames: u64, peak_out: f32, peak_in: f32) {
    SHARED.playing.store(playing, Ordering::Relaxed);
    SHARED.previewing.store(previewing, Ordering::Relaxed);
    SHARED.position.store(position_frames, Ordering::Relaxed);
    update_peak(&SHARED.peak_out, peak_out);
    update_peak(&SHARED.peak_in, peak_in);
}

/// Mikrofon-Samples (mono, Ausgaberate) aus dem Worklet.
pub fn record_input(samples: &[f32]) {
    if let Some(rec) = REC.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        rec.extend_from_slice(samples);
    }
}

#[derive(Default)]
struct Inner {
    /// Puffer, die das Worklet hat: Adresse → (Kennung, Puffer). Der Puffer
    /// wird festgehalten, damit seine Adresse nicht neu vergeben wird.
    known: HashMap<usize, (u32, Arc<[f32]>)>,
    next_id: u32,
    /// Adressen der Puffer im aktuellen Arrangement bzw. Vorhören.
    in_arrangement: HashSet<usize>,
    in_preview: HashSet<usize>,
    stream: Option<PreviewStream>,
}

fn addr(b: &Arc<[f32]>) -> usize {
    Arc::as_ptr(b) as *const f32 as usize
}

impl Inner {
    /// Kennung eines Puffers; ein neuer geht dabei ans Worklet.
    fn id_for(&mut self, port: &WebPort, b: &Arc<[f32]>) -> u32 {
        let a = addr(b);
        if let Some((id, _)) = self.known.get(&a) {
            return *id;
        }
        self.next_id += 1;
        let id = self.next_id;
        (port.buffer)(id, b);
        self.known.insert(a, (id, b.clone()));
        id
    }

    /// Clips ans Worklet (nach dem Kopf `head`); liefert die Adressen der
    /// benutzten Puffer.
    fn send_clips(&mut self, port: &WebPort, kind: Kind, head: &[f64], arr: &Arrangement) -> HashSet<usize> {
        let mut used = HashSet::new();
        let mut d = head.to_vec();
        d.extend(wire::encode_clips(arr, |b| {
            used.insert(addr(b));
            self.id_for(port, b)
        }));
        (port.post)(kind, &d);
        used
    }

    /// Puffer, die weder das Arrangement noch das Vorhören braucht, freigeben.
    fn collect(&mut self, port: &WebPort) {
        let keep = |a: &usize| self.in_arrangement.contains(a) || self.in_preview.contains(a);
        let gone: Vec<usize> = self.known.keys().copied().filter(|a| !keep(a)).collect();
        if gone.is_empty() {
            return;
        }
        let ids: Vec<f64> = gone.iter().filter_map(|a| self.known.remove(a)).map(|(id, _)| id as f64).collect();
        (port.post)(Kind::Free, &ids);
    }
}

pub struct Engine {
    info: EngineInfo,
    port: WebPort,
    inner: Mutex<Inner>,
}

impl Engine {
    /// Startet die Engine an der mit [`connect`] gesetzten Ausgabe.
    pub fn start() -> Result<Self, EngineError> {
        let port = port().ok_or_else(|| EngineError(tr!("No audio output in the browser (AudioWorklet not available)")))?;
        let info = EngineInfo { output_device: port.device.clone(), sample_rate: port.sample_rate, channels: 2, ..Default::default() };
        Ok(Self { info, port, inner: Mutex::new(Inner::default()) })
    }

    pub fn info(&self) -> EngineInfo {
        self.info.clone()
    }

    /// Im Browser wählt das System die Geräte (Ausgabe des AudioContext,
    /// Mikrofon über die Freigabe) – gelistet wird nur dieses eine Paar.
    pub fn devices(&self) -> Result<AudioDevices, EngineError> {
        Ok(AudioDevices {
            outputs: vec![self.info.output_device.clone()],
            inputs: vec!["Mikrofon".into()],
            default_output: Some(self.info.output_device.clone()),
            default_input: Some("Mikrofon".into()),
        })
    }

    /// Gerätewahl gibt es im Browser nicht; nur der Systemstandard geht.
    pub fn set_output_device(&self, name: Option<String>) -> Result<EngineInfo, EngineError> {
        match name {
            None => Ok(self.info()),
            Some(_) => Err(EngineError(tr!("In the browser, the system chooses the output device."))),
        }
    }

    pub fn set_input_device(&self, name: Option<String>) -> Result<EngineInfo, EngineError> {
        match name {
            None => Ok(self.info()),
            Some(_) => Err(EngineError(tr!("In the browser, the system chooses the microphone (grant access in the browser)."))),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, msg: RtMsg) {
        if let Some((k, d)) = wire::encode(&msg) {
            (self.port.post)(k, &d);
        }
    }

    pub fn play(&self) {
        self.send(RtMsg::Play);
    }

    pub fn stop(&self) {
        self.send(RtMsg::Stop);
    }

    pub fn seek_frames(&self, frame: u64) {
        self.send(RtMsg::Seek(frame));
    }

    /// Bei laufender Wiedergabe bleibt die Position in Schlägen erhalten.
    pub fn set_tempo(&self, bpm: f64, beats_per_bar: u32) {
        self.send(RtMsg::Tempo { bpm, beats_per_bar });
    }

    pub fn set_metronome(&self, on: bool) {
        self.send(RtMsg::Metronome(on));
    }

    /// Ausgabe stumm schalten: Wiedergabe, Position und Pegel laufen weiter.
    pub fn set_muted(&self, on: bool) {
        SHARED.muted.store(on, Ordering::Relaxed);
        (self.port.post)(Kind::Mute, &[if on { 1.0 } else { 0.0 }]);
    }

    pub fn set_loop(&self, region: Option<(f64, f64)>) {
        self.send(RtMsg::Loop(region.filter(|(a, b)| b > a)));
    }

    pub fn set_stop_at(&self, at: Option<(f64, f64)>) {
        self.send(RtMsg::StopAt(at.filter(|(s, _)| *s > 0.0)));
    }

    /// Neues Arrangement; während der Wiedergabe wird übergeblendet.
    pub fn set_arrangement(&self, arrangement: Arrangement) {
        self.send_arrangement(arrangement, None);
    }

    /// Neues Arrangement mit neuem Tempo in einem Schritt (eine Nachricht ans
    /// Worklet): Position in Schlägen und Klang wechseln im selben Block.
    pub fn set_arrangement_with_tempo(&self, arrangement: Arrangement, bpm: f64, beats_per_bar: u32) {
        self.send_arrangement(arrangement, Some((bpm, beats_per_bar)));
    }

    fn send_arrangement(&self, arrangement: Arrangement, tempo: Option<(f64, u32)>) {
        let mut g = self.inner();
        g.in_arrangement = g.send_clips(&self.port, Kind::Arrangement, &wire::arrangement_head(tempo), &arrangement);
        g.collect(&self.port);
    }

    /// Spielt einen Mono-Puffer (Ausgaberate) sofort ab; ersetzt ein laufendes Vorhören.
    pub fn preview(&self, data: Arc<[f32]>, gain: f32) {
        self.preview_send(data, gain, 0.0);
    }

    pub fn preview_send(&self, data: Arc<[f32]>, gain: f32, send: f32) {
        self.preview_stereo(data, None, gain, send);
    }

    pub fn preview_stereo(&self, data: Arc<[f32]>, side: Option<Arc<[f32]>>, gain: f32, send: f32) {
        let arr = Arrangement { clips: vec![PlacedClip { start_frame: 0, data, side, gain_l: gain, gain_r: gain, send }] };
        let mut g = self.inner();
        g.stream = None;
        g.in_preview = g.send_clips(&self.port, Kind::Preview, &[], &arr);
        g.collect(&self.port);
    }

    /// Spielt ein Vorhören ab, das `source` nach und nach liefert (nachgefüllt
    /// in [`Engine::tick`]).
    pub fn preview_stream(&self, source: Box<dyn PreviewSource>) {
        let ahead = (STREAM_AHEAD_SECS * self.info.sample_rate as f64) as u64;
        let mut st = PreviewStream::new(source);
        st.fill(0, ahead);
        st.started = (self.port.now)();
        let mut g = self.inner();
        g.in_preview = g.send_clips(&self.port, Kind::Preview, &[], &st.arrangement());
        g.stream = Some(st);
        g.collect(&self.port);
    }

    /// Was auf dem Desktop der Engine-Thread nebenbei tut: gestreamtes
    /// Vorhören nachfüllen. Vom Web-Kern alle paar Millisekunden aufgerufen.
    pub fn tick(&self) {
        let sr = self.info.sample_rate as f64;
        let ahead = (STREAM_AHEAD_SECS * sr) as u64;
        let mut g = self.inner();
        let Some(st) = g.stream.as_mut() else { return };
        let now = (((self.port.now)() - st.started).max(0.0) * sr) as u64;
        if st.fill(now.saturating_sub((sr * 0.5) as u64), ahead) {
            let arr = st.arrangement();
            let done = st.done;
            g.in_preview = g.send_clips(&self.port, Kind::PreviewMore, &[], &arr);
            if done {
                g.stream = None;
            }
            g.collect(&self.port);
        } else if st.done {
            g.stream = None;
        }
    }

    /// Aufnahme starten: das Worklet schickt ab jetzt die Mikrofon-Samples
    /// (der Web-Kern verbindet das Mikrofon vorher).
    pub fn start_recording(&self) -> Result<u32, EngineError> {
        *REC.lock().unwrap_or_else(|e| e.into_inner()) = Some(Vec::with_capacity(self.info.sample_rate as usize * 30));
        SHARED.recording.store(true, Ordering::Relaxed);
        REC_START.store(SHARED.position.load(Ordering::Relaxed), Ordering::Relaxed);
        (self.port.post)(Kind::Record, &[1.0]);
        Ok(self.info.sample_rate)
    }

    /// Aufnahme beenden. Der Web-Kern holt vorher die letzten Samples aus dem
    /// Worklet ab (auf dem Desktop: das Leeren des Ringpuffers).
    pub fn stop_recording(&self) -> Result<Recording, EngineError> {
        (self.port.post)(Kind::Record, &[0.0]);
        SHARED.recording.store(false, Ordering::Relaxed);
        let samples = REC.lock().unwrap_or_else(|e| e.into_inner()).take().ok_or_else(|| EngineError(tr!("No recording is running")))?;
        Ok(Recording { samples, sample_rate: self.info.sample_rate, start_frame: REC_START.load(Ordering::Relaxed), device: "Mikrofon".into() })
    }

    /// Liest den Status; Spitzenpegel werden dabei zurückgesetzt.
    pub fn status(&self) -> EngineStatus {
        SHARED.status(self.info.sample_rate)
    }
}

/// Wiedergabeposition beim Start der Aufnahme.
static REC_START: AtomicU64 = AtomicU64::new(0);
