//! Engine am Audiogerät (Desktop): cpal-Streams in einem eigenen Thread.
//!
//! * Ein eigener Engine-Thread besitzt die cpal-Streams (die nicht `Send` sind).
//! * Die App spricht mit ihm über einen normalen Kanal ([`Engine`]-Methoden).
//! * Der Engine-Thread reicht Nachrichten über einen sperrfreien Ringpuffer
//!   an den Audio-Callback ([`Renderer`]) weiter.
//!
//! Gegenstück im Browser: `web.rs` (gleiche Schnittstelle, das Worklet statt
//! des Audio-Callbacks).

use crate::{update_peak, Arrangement, AudioDevices, EngineError, EngineInfo, EngineStatus, OutSample, PlacedClip, PreviewSource, PreviewStream, Recording, Renderer, RtMsg, Shared, STREAM_AHEAD_SECS};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use klang_i18n::tr;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn err(e: impl std::fmt::Display) -> EngineError {
    EngineError(e.to_string())
}

enum Control {
    Rt(RtMsg),
    PreviewStream(Box<dyn PreviewSource>),
    StartRecording(mpsc::Sender<Result<u32, EngineError>>),
    StopRecording(mpsc::Sender<Result<Recording, EngineError>>),
    Devices(mpsc::Sender<AudioDevices>),
    SetOutput(Option<String>, mpsc::Sender<Result<EngineInfo, EngineError>>),
    SetInput(Option<String>, mpsc::Sender<Result<EngineInfo, EngineError>>),
    Shutdown,
}

pub struct Engine {
    tx: Mutex<mpsc::Sender<Control>>,
    shared: Arc<Shared>,
    /// Vom Engine-Thread gepflegt (ändert sich beim Gerätewechsel).
    info: Arc<Mutex<EngineInfo>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

fn no_answer() -> EngineError {
    EngineError(tr!("No response from the audio thread"))
}

impl Engine {
    /// Öffnet das Standard-Ausgabegerät und startet die Wiedergabe-Schleife.
    pub fn start() -> Result<Self, EngineError> {
        let shared = Arc::new(Shared::default());
        let info = Arc::new(Mutex::new(EngineInfo::default()));
        let (tx, rx) = mpsc::channel();
        let (init_tx, init_rx) = mpsc::channel();
        let (sh, inf) = (shared.clone(), info.clone());
        let thread = std::thread::Builder::new()
            .name("klang-engine".into())
            .spawn(move || engine_thread(rx, sh, inf, init_tx))
            .map_err(err)?;
        match init_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx: Mutex::new(tx), shared, info, thread: Mutex::new(Some(thread)) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err(EngineError(tr!("Audio thread ended unexpectedly"))),
        }
    }

    pub fn info(&self) -> EngineInfo {
        self.info.lock().map(|i| i.clone()).unwrap_or_default()
    }

    /// Ausgabe- und Aufnahmegeräte des Systems.
    pub fn devices(&self) -> Result<AudioDevices, EngineError> {
        let (t, r) = mpsc::channel();
        self.send(Control::Devices(t));
        r.recv_timeout(Duration::from_secs(10)).map_err(|_| no_answer())
    }

    /// Ausgabegerät wechseln (`None` = Systemstandard). Der Stream wird mit dem
    /// neuen Gerät neu aufgebaut; Wiedergabe, Position (in Schlägen), Tempo,
    /// Metronom und Schleife laufen dort weiter, ein Vorhören endet. Hat das
    /// neue Gerät eine andere Abtastrate, fehlt danach das Arrangement – die
    /// App rendert es für `info().sample_rate` neu.
    pub fn set_output_device(&self, name: Option<String>) -> Result<EngineInfo, EngineError> {
        let (t, r) = mpsc::channel();
        self.send(Control::SetOutput(name, t));
        r.recv_timeout(Duration::from_secs(15)).map_err(|_| no_answer())?
    }

    /// Aufnahmegerät für die nächsten Aufnahmen (`None` = Systemstandard).
    pub fn set_input_device(&self, name: Option<String>) -> Result<EngineInfo, EngineError> {
        let (t, r) = mpsc::channel();
        self.send(Control::SetInput(name, t));
        r.recv_timeout(Duration::from_secs(10)).map_err(|_| no_answer())?
    }

    fn send(&self, c: Control) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(c);
        }
    }

    pub fn play(&self) {
        self.send(Control::Rt(RtMsg::Play));
    }

    pub fn stop(&self) {
        self.send(Control::Rt(RtMsg::Stop));
    }

    pub fn seek_frames(&self, frame: u64) {
        self.send(Control::Rt(RtMsg::Seek(frame)));
    }

    /// Bei laufender Wiedergabe bleibt die Position in Schlägen erhalten.
    pub fn set_tempo(&self, bpm: f64, beats_per_bar: u32) {
        self.send(Control::Rt(RtMsg::Tempo { bpm, beats_per_bar }));
    }

    pub fn set_metronome(&self, on: bool) {
        self.send(Control::Rt(RtMsg::Metronome(on)));
    }

    /// Ausgabe stumm schalten: Wiedergabe, Position und Pegel laufen weiter,
    /// an das Gerät gehen nur Nullen (automatische Tests).
    pub fn set_muted(&self, on: bool) {
        self.shared.muted.store(on, Ordering::Relaxed);
    }

    /// Wiedergabe wiederholen: erreicht die Position `end` (Schläge), geht es bei
    /// `start` weiter. Wer hinter dem Ende startet, spielt ungebremst weiter.
    pub fn set_loop(&self, region: Option<(f64, f64)>) {
        self.send(Control::Rt(RtMsg::Loop(region.filter(|(a, b)| b > a))));
    }

    /// Wiedergabe ohne Schleife an Schlag `stop` beenden und den Abspielkopf
    /// auf Schlag `back` setzen (z. B. Ende des markierten Bereichs → sein Anfang).
    pub fn set_stop_at(&self, at: Option<(f64, f64)>) {
        self.send(Control::Rt(RtMsg::StopAt(at.filter(|(s, _)| *s > 0.0))));
    }

    /// Neues Arrangement; während der Wiedergabe wird übergeblendet.
    pub fn set_arrangement(&self, arrangement: Arrangement) {
        self.send(Control::Rt(RtMsg::Arrangement { arrangement: Arc::new(arrangement), tempo: None }));
    }

    /// Neues Arrangement mit neuem Tempo in einem Schritt (z. B. nach einer
    /// Tempoänderung neu gerendert): Position in Schlägen und Klang wechseln
    /// im selben Audio-Block, übergeblendet.
    pub fn set_arrangement_with_tempo(&self, arrangement: Arrangement, bpm: f64, beats_per_bar: u32) {
        self.send(Control::Rt(RtMsg::Arrangement { arrangement: Arc::new(arrangement), tempo: Some((bpm, beats_per_bar)) }));
    }

    /// Spielt einen Mono-Puffer (Ausgaberate) sofort ab; ersetzt ein laufendes Vorhören.
    pub fn preview(&self, data: Arc<[f32]>, gain: f32) {
        self.preview_send(data, gain, 0.0);
    }

    /// Wie [`Engine::preview`], mit Hall-Anteil `send` (Raum des Instruments).
    pub fn preview_send(&self, data: Arc<[f32]>, gain: f32, send: f32) {
        self.preview_stereo(data, None, gain, send);
    }

    /// Wie [`Engine::preview_send`], mit Seitensignal für Stereo-Breite.
    pub fn preview_stereo(&self, data: Arc<[f32]>, side: Option<Arc<[f32]>>, gain: f32, send: f32) {
        let clip = PlacedClip { start_frame: 0, data, side, gain_l: gain, gain_r: gain, send };
        self.send(Control::Rt(RtMsg::Preview(Arc::new(Arrangement { clips: vec![clip] }))));
    }

    /// Spielt ein Vorhören ab, das `source` nach und nach liefert – so lange, bis
    /// es endet oder ein anderes Vorhören (auch ein leeres = Stopp) es ersetzt.
    pub fn preview_stream(&self, source: Box<dyn PreviewSource>) {
        self.send(Control::PreviewStream(source));
    }

    pub fn start_recording(&self) -> Result<u32, EngineError> {
        let (t, r) = mpsc::channel();
        self.send(Control::StartRecording(t));
        r.recv_timeout(Duration::from_secs(5)).map_err(|_| no_answer())?
    }

    pub fn stop_recording(&self) -> Result<Recording, EngineError> {
        let (t, r) = mpsc::channel();
        self.send(Control::StopRecording(t));
        r.recv_timeout(Duration::from_secs(5)).map_err(|_| no_answer())?
    }

    /// Liest den Status; Spitzenpegel werden dabei zurückgesetzt.
    pub fn status(&self) -> EngineStatus {
        self.shared.status(self.shared.sample_rate.load(Ordering::Relaxed))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.send(Control::Shutdown);
        if let Ok(mut t) = self.thread.lock() {
            if let Some(t) = t.take() {
                let _ = t.join();
            }
        }
    }
}

// ------------------------------------------------------------------ Engine-Thread

/// Ein offener Ausgabe-Stream samt den Ringpuffern zu seinem Callback.
struct Output {
    stream: cpal::Stream,
    rt_tx: Producer<RtMsg>,
    garbage_rx: Consumer<Arc<Arrangement>>,
    name: String,
    sample_rate: u32,
    channels: u16,
}

fn find_device<I: Iterator<Item = cpal::Device>>(devices: Result<I, cpal::DevicesError>, name: &str) -> Option<cpal::Device> {
    devices.ok()?.find(|d| d.name().map(|n| n == name).unwrap_or(false))
}

fn device_names<I: Iterator<Item = cpal::Device>>(devices: Result<I, cpal::DevicesError>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in devices.into_iter().flatten().filter_map(|d| d.name().ok()) {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

fn list_devices(host: &cpal::Host) -> AudioDevices {
    AudioDevices {
        outputs: device_names(host.output_devices()),
        inputs: device_names(host.input_devices()),
        default_output: host.default_output_device().and_then(|d| d.name().ok()),
        default_input: host.default_input_device().and_then(|d| d.name().ok()),
    }
}

/// Öffnet ein Ausgabegerät (`None` = Systemstandard) mit frischem Callback;
/// der Stream läuft noch nicht.
fn open_output(host: &cpal::Host, choice: Option<&str>, shared: &Arc<Shared>) -> Result<Output, EngineError> {
    let device = match choice {
        None => host.default_output_device().ok_or_else(|| EngineError(tr!("No audio output device found")))?,
        Some(n) => find_device(host.output_devices(), n)
            .ok_or_else(|| EngineError(tr!("Output device \"{name}\" not found – is it connected?", name = n)))?,
    };
    let name = device.name().unwrap_or_else(|_| "Unbekanntes Gerät".into());
    let supported = device.default_output_config().map_err(err)?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let (rt_tx, rt_rx) = RingBuffer::<RtMsg>::new(512);
    let (garbage_tx, garbage_rx) = RingBuffer::<Arc<Arrangement>>::new(64);
    let renderer = Renderer::new(config.sample_rate.0 as f32, config.channels as usize, rt_rx, garbage_tx, shared.clone());
    let stream = match format {
        cpal::SampleFormat::F32 => build_output::<f32>(&device, &config, renderer),
        cpal::SampleFormat::I16 => build_output::<i16>(&device, &config, renderer),
        cpal::SampleFormat::U16 => build_output::<u16>(&device, &config, renderer),
        cpal::SampleFormat::I32 => build_output::<i32>(&device, &config, renderer),
        other => Err(EngineError(tr!("Unsupported sample format: {format}", format = format!("{other:?}")))),
    }?;
    Ok(Output { stream, rt_tx, garbage_rx, name, sample_rate: config.sample_rate.0, channels: config.channels })
}

/// Nachricht an den Callback; ist der Ringpuffer voll, kurz warten.
fn push_rt(rt_tx: &mut Producer<RtMsg>, msg: RtMsg) {
    let mut m = msg;
    while let Err(rtrb::PushError::Full(back)) = rt_tx.push(m) {
        m = back;
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Was der Callback vom Transport weiß – damit ein neuer Stream nach einem
/// Gerätewechsel genauso weiterläuft.
#[derive(Default)]
pub(crate) struct Mirror {
    tempo: Option<(f64, u32)>,
    metronome: bool,
    looping: Option<(f64, f64)>,
    stop_at: Option<(f64, f64)>,
    /// Arrangement und die Abtastrate, für die es gerendert ist.
    arrangement: Option<(Arc<Arrangement>, u32)>,
}

impl Mirror {
    pub(crate) fn note(&mut self, msg: &RtMsg, sample_rate: u32) {
        match msg {
            RtMsg::Tempo { bpm, beats_per_bar } => self.tempo = Some((*bpm, *beats_per_bar)),
            RtMsg::Metronome(on) => self.metronome = *on,
            RtMsg::Loop(r) => self.looping = *r,
            RtMsg::StopAt(a) => self.stop_at = *a,
            RtMsg::Arrangement { arrangement, tempo } => {
                // Tempo kommt mit dem Arrangement (kein Block mit neuem Tempo und altem Klang)
                if let Some(t) = tempo {
                    self.tempo = Some(*t);
                }
                self.arrangement = Some((arrangement.clone(), sample_rate));
            }
            _ => {}
        }
    }

    /// Nachrichten, die einen frischen Callback mit `sample_rate` auf diesen
    /// Stand bringen; das Arrangement nur, wenn es für diese Rate gerendert ist.
    pub(crate) fn replay(&self, sample_rate: u32) -> Vec<RtMsg> {
        let mut out = Vec::new();
        if let Some((bpm, beats_per_bar)) = self.tempo {
            out.push(RtMsg::Tempo { bpm, beats_per_bar });
        }
        out.push(RtMsg::Metronome(self.metronome));
        out.push(RtMsg::Loop(self.looping));
        out.push(RtMsg::StopAt(self.stop_at));
        if let Some((a, sr)) = &self.arrangement {
            if *sr == sample_rate {
                out.push(RtMsg::Arrangement { arrangement: a.clone(), tempo: None });
            }
        }
        out
    }
}

/// Startet `next` an Stelle von `out`: alten Stream beenden, den neuen
/// Callback auf den Stand bringen (Position in Schlägen bleibt), losspielen.
fn hand_over(out: &mut Output, next: Output, shared: &Shared, mirror: &Mirror) -> Result<(), EngineError> {
    let old_sr = out.sample_rate.max(1);
    let old = std::mem::replace(out, next);
    let _ = old.stream.pause();
    drop(old); // Callback steht; sein Stand liegt in `shared` und `mirror`
    let pos = shared.position.load(Ordering::Relaxed);
    let pos = (pos as f64 * out.sample_rate as f64 / old_sr as f64).round() as u64;
    for m in mirror.replay(out.sample_rate) {
        push_rt(&mut out.rt_tx, m);
    }
    push_rt(&mut out.rt_tx, RtMsg::Seek(pos));
    if shared.playing.load(Ordering::Relaxed) {
        push_rt(&mut out.rt_tx, RtMsg::Play);
    }
    shared.position.store(pos, Ordering::Relaxed);
    shared.previewing.store(false, Ordering::Relaxed);
    shared.sample_rate.store(out.sample_rate, Ordering::Relaxed);
    out.stream.play().map_err(err)
}

fn engine_thread(rx: mpsc::Receiver<Control>, shared: Arc<Shared>, info: Arc<Mutex<EngineInfo>>, init: mpsc::Sender<Result<(), EngineError>>) {
    let host = cpal::default_host();
    let mut out = match open_output(&host, None, &shared).and_then(|o| o.stream.play().map(|_| o).map_err(err)) {
        Ok(o) => o,
        Err(e) => {
            let _ = init.send(Err(e));
            return;
        }
    };
    let mut output_choice: Option<String> = None;
    let mut input_choice: Option<String> = None;
    let publish = |out: &Output, output_choice: &Option<String>, input_choice: &Option<String>| {
        let i = EngineInfo {
            output_device: out.name.clone(),
            sample_rate: out.sample_rate,
            channels: out.channels,
            output_choice: output_choice.clone(),
            input_choice: input_choice.clone(),
        };
        if let Ok(mut g) = info.lock() {
            *g = i.clone();
        }
        i
    };
    shared.sample_rate.store(out.sample_rate, Ordering::Relaxed);
    publish(&out, &output_choice, &input_choice);
    let _ = init.send(Ok(()));

    let mut mirror = Mirror::default();
    // Uhr für gestreamtes Vorhören (Sekunden seit dem Start des Threads)
    let clock = Instant::now();
    let mut input: Option<InputCapture> = None;
    let mut stream_preview: Option<PreviewStream> = None;
    let busy = || EngineError(tr!("The device can't be changed during a recording."));
    loop {
        let sr = out.sample_rate as f64;
        let ahead = (STREAM_AHEAD_SECS * sr) as u64;
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(Control::Rt(msg)) => {
                // jedes andere Vorhören (oder Stopp) beendet ein gestreamtes
                if matches!(msg, RtMsg::Preview(_)) {
                    stream_preview = None;
                }
                mirror.note(&msg, out.sample_rate);
                push_rt(&mut out.rt_tx, msg);
            }
            Ok(Control::PreviewStream(source)) => {
                let mut st = PreviewStream::new(source);
                st.fill(0, ahead);
                st.started = clock.elapsed().as_secs_f64();
                push_rt(&mut out.rt_tx, RtMsg::Preview(Arc::new(st.arrangement())));
                stream_preview = Some(st);
            }
            Ok(Control::Devices(reply)) => {
                let _ = reply.send(list_devices(&host));
            }
            Ok(Control::SetOutput(choice, reply)) => {
                let res = if input.is_some() {
                    Err(busy())
                } else {
                    match open_output(&host, choice.as_deref(), &shared) {
                        Err(e) => Err(e),
                        Ok(next) => {
                            stream_preview = None;
                            match hand_over(&mut out, next, &shared, &mirror) {
                                Ok(()) => {
                                    output_choice = choice;
                                    Ok(())
                                }
                                Err(e) => {
                                    // neues Gerät spielt nicht: zurück zum bisherigen
                                    if let Ok(back) = open_output(&host, output_choice.as_deref(), &shared) {
                                        let _ = hand_over(&mut out, back, &shared, &mirror);
                                    }
                                    Err(e)
                                }
                            }
                        }
                    }
                };
                let i = publish(&out, &output_choice, &input_choice);
                let _ = reply.send(res.map(|_| i));
            }
            Ok(Control::SetInput(choice, reply)) => {
                let res = match &choice {
                    _ if input.is_some() => Err(busy()),
                    Some(n) if find_device(host.input_devices(), n).is_none() => {
                        Err(EngineError(tr!("Input device \"{name}\" not found – is it connected?", name = n)))
                    }
                    _ => {
                        input_choice = choice;
                        Ok(())
                    }
                };
                let i = publish(&out, &output_choice, &input_choice);
                let _ = reply.send(res.map(|_| i));
            }
            Ok(Control::StartRecording(reply)) => {
                input = None;
                match InputCapture::start(&host, input_choice.as_deref(), shared.clone()) {
                    Ok(cap) => {
                        let sr = cap.sample_rate;
                        input = Some(cap);
                        shared.recording.store(true, Ordering::Relaxed);
                        let _ = reply.send(Ok(sr));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }
            Ok(Control::StopRecording(reply)) => {
                shared.recording.store(false, Ordering::Relaxed);
                let res = match input.take() {
                    Some(cap) => Ok(cap.finish()),
                    None => Err(EngineError(tr!("No recording is running"))),
                };
                let _ = reply.send(res);
            }
            Ok(Control::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some(st) = stream_preview.as_mut() {
            // Uhrzeit statt Audio-Position: die meldet der Callback erst nach dem Umschalten
            let now = ((clock.elapsed().as_secs_f64() - st.started) * sr) as u64;
            if st.fill(now.saturating_sub((sr * 0.5) as u64), ahead) {
                push_rt(&mut out.rt_tx, RtMsg::PreviewMore(Arc::new(st.arrangement())));
            }
            if st.done {
                stream_preview = None;
            }
        }
        while let Ok(old) = out.garbage_rx.pop() {
            drop(old);
        }
        if let Some(cap) = input.as_mut() {
            cap.drain();
        }
    }
    drop(input);
    drop(out);
}

/// Audio-Thread vor allen normalen Threads einplanen, damit Rechenlast auf
/// allen Kernen (Vorab-Rendern nach einer Tempoänderung) ihn nicht verdrängt –
/// sonst fehlen Blöcke und es knackt. cpal 0.15 versucht das selbst, übergibt
/// aber eine Thread-Kennung statt eines Handles; der Aufruf bleibt wirkungslos.
#[cfg(windows)]
fn boost_audio_thread() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;
    // SAFETY: Pseudo-Handle des eigenen Threads, nur dessen Priorität ändert sich.
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
}

#[cfg(not(windows))]
fn boost_audio_thread() {}

fn build_output<T>(device: &cpal::Device, config: &cpal::StreamConfig, mut r: Renderer) -> Result<cpal::Stream, EngineError>
where
    T: SizedSample + FromSample<f32> + OutSample,
{
    let mut boosted = false;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                if !boosted {
                    boosted = true;
                    boost_audio_thread();
                }
                r.process(data)
            },
            |e| eprintln!("Audio-Ausgabefehler: {e}"),
            None,
        )
        .map_err(err)
}

// ------------------------------------------------------------------ Aufnahme

struct InputCapture {
    stream: cpal::Stream,
    consumer: Consumer<f32>,
    samples: Vec<f32>,
    sample_rate: u32,
    start_frame: u64,
    device: String,
}

impl InputCapture {
    /// Nimmt vom gewählten Gerät auf (`None` = Systemstandard).
    fn start(host: &cpal::Host, choice: Option<&str>, shared: Arc<Shared>) -> Result<Self, EngineError> {
        let device = match choice {
            None => host.default_input_device().ok_or_else(|| EngineError(tr!("No microphone found")))?,
            Some(n) => find_device(host.input_devices(), n)
                .ok_or_else(|| EngineError(tr!("Input device \"{name}\" not found – choose another one in the microphone menu at the top", name = n)))?,
        };
        let name = device.name().unwrap_or_else(|_| "Mikrofon".into());
        let supported = device.default_input_config().map_err(err)?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let sample_rate = config.sample_rate.0;
        let (producer, consumer) = RingBuffer::<f32>::new(sample_rate as usize * 8);
        let stream = match format {
            cpal::SampleFormat::F32 => build_input::<f32>(&device, &config, producer, shared.clone()),
            cpal::SampleFormat::I16 => build_input::<i16>(&device, &config, producer, shared.clone()),
            cpal::SampleFormat::U16 => build_input::<u16>(&device, &config, producer, shared.clone()),
            cpal::SampleFormat::I32 => build_input::<i32>(&device, &config, producer, shared.clone()),
            other => Err(EngineError(tr!("Unsupported microphone format: {format}", format = format!("{other:?}")))),
        }?;
        stream.play().map_err(err)?;
        Ok(Self {
            stream,
            consumer,
            samples: Vec::with_capacity(sample_rate as usize * 60),
            sample_rate,
            start_frame: shared.position.load(Ordering::Relaxed),
            device: name,
        })
    }

    fn drain(&mut self) {
        while let Ok(v) = self.consumer.pop() {
            self.samples.push(v);
        }
    }

    fn finish(self) -> Recording {
        let InputCapture { stream, mut consumer, mut samples, sample_rate, start_frame, device } = self;
        drop(stream);
        while let Ok(v) = consumer.pop() {
            samples.push(v);
        }
        Recording { samples, sample_rate, start_frame, device }
    }
}

fn build_input<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut producer: Producer<f32>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, EngineError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = (config.channels as usize).max(1);
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mut peak = 0f32;
                for frame in data.chunks(channels) {
                    let sum: f32 = frame.iter().map(|&s| s.to_sample::<f32>()).sum();
                    let v = sum / channels as f32;
                    peak = peak.max(v.abs());
                    let _ = producer.push(v);
                }
                update_peak(&shared.peak_in, peak);
            },
            |e| eprintln!("Audio-Eingabefehler: {e}"),
            None,
        )
        .map_err(err)
}

