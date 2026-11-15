//! Engine on the audio device (desktop): cpal streams in a dedicated thread.
//!
//! * A dedicated engine thread owns the cpal streams (which are not `Send`).
//! * The app talks to it over a normal channel ([`Engine`] methods).
//! * The engine thread passes messages on to the audio callback ([`Renderer`])
//!   through a lock-free ring buffer.
//!
//! Counterpart in the browser: `web.rs` (same interface, the worklet instead
//! of the audio callback).

use crate::{update_peak, Arrangement, AudioDevices, EngineError, EngineInfo, EngineStatus, Garbage, OutSample, PlacedClip, PreviewSource, PreviewStream, Recording, Renderer, RtMsg, Shared, STREAM_AHEAD_SECS};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use klang_i18n::tr;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, Ordering};
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
    SetFakeInput(Option<FakeInput>),
    Shutdown,
}

/// Stand-in for the microphone in automated tests: recordings take these
/// samples (in a loop, at the pace of real time) instead of a device.
#[derive(Clone)]
pub struct FakeInput {
    pub samples: Arc<[f32]>,
    pub sample_rate: u32,
    /// Shown as the recording device.
    pub name: String,
}

pub struct Engine {
    tx: Mutex<mpsc::Sender<Control>>,
    shared: Arc<Shared>,
    /// Maintained by the engine thread (changes on a device switch).
    info: Arc<Mutex<EngineInfo>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

fn no_answer() -> EngineError {
    EngineError(tr!("No response from the audio thread"))
}

impl Engine {
    /// Opens the default output device and starts the playback loop.
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

    /// Output and recording devices of the system.
    pub fn devices(&self) -> Result<AudioDevices, EngineError> {
        let (t, r) = mpsc::channel();
        self.send(Control::Devices(t));
        r.recv_timeout(Duration::from_secs(10)).map_err(|_| no_answer())
    }

    /// Switches the output device (`None` = system default). The stream is rebuilt
    /// with the new device; playback, position (in beats), tempo, metronome and
    /// loop continue there, a preview ends. If the new device has a different
    /// sample rate, the arrangement is missing afterwards – the app renders it
    /// again for `info().sample_rate`.
    pub fn set_output_device(&self, name: Option<String>) -> Result<EngineInfo, EngineError> {
        let (t, r) = mpsc::channel();
        self.send(Control::SetOutput(name, t));
        r.recv_timeout(Duration::from_secs(15)).map_err(|_| no_answer())?
    }

    /// Recording device for the next recordings (`None` = system default).
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

    /// While playback is running, the position in beats is preserved.
    pub fn set_tempo(&self, bpm: f64, beats_per_bar: u32) {
        self.send(Control::Rt(RtMsg::Tempo { bpm, beats_per_bar }));
    }

    pub fn set_metronome(&self, on: bool) {
        self.send(Control::Rt(RtMsg::Metronome(on)));
    }

    /// Mutes the output: playback, position and level keep running, only zeros
    /// go to the device (automated tests).
    pub fn set_muted(&self, on: bool) {
        self.shared.muted.store(on, Ordering::Relaxed);
    }

    /// Repeats playback: when the position reaches `end` (beats), it continues at
    /// `start`. Anyone starting past the end keeps playing unrestrained.
    pub fn set_loop(&self, region: Option<(f64, f64)>) {
        self.send(Control::Rt(RtMsg::Loop(region.filter(|(a, b)| b > a))));
    }

    /// Ends playback without a loop at beat `stop` and sets the playhead
    /// to beat `back` (e.g. end of the selected range → its start).
    pub fn set_stop_at(&self, at: Option<(f64, f64)>) {
        self.send(Control::Rt(RtMsg::StopAt(at.filter(|(s, _)| *s > 0.0))));
    }

    /// New arrangement; crossfaded during playback.
    pub fn set_arrangement(&self, arrangement: Arrangement) {
        self.send(Control::Rt(RtMsg::Arrangement { arrangement: Arc::new(arrangement), tempo: None }));
    }

    /// New arrangement with a new tempo in one step (e.g. re-rendered after a
    /// tempo change): position in beats and sound switch in the same audio
    /// block, crossfaded.
    pub fn set_arrangement_with_tempo(&self, arrangement: Arrangement, bpm: f64, beats_per_bar: u32) {
        self.send(Control::Rt(RtMsg::Arrangement { arrangement: Arc::new(arrangement), tempo: Some((bpm, beats_per_bar)) }));
    }

    /// Plays a mono buffer (output rate) immediately; replaces a running preview.
    pub fn preview(&self, data: Arc<[f32]>, gain: f32) {
        self.preview_send(data, gain, 0.0);
    }

    /// Like [`Engine::preview`], with reverb amount `send` (the instrument's room).
    pub fn preview_send(&self, data: Arc<[f32]>, gain: f32, send: f32) {
        self.preview_stereo(data, None, gain, send);
    }

    /// Like [`Engine::preview_send`], with a side signal for stereo width.
    pub fn preview_stereo(&self, data: Arc<[f32]>, side: Option<Arc<[f32]>>, gain: f32, send: f32) {
        let clip = PlacedClip { start_frame: 0, data, side, gain_l: gain, gain_r: gain, send };
        self.send(Control::Rt(RtMsg::Preview(Arc::new(Arrangement { clips: vec![clip], notes: Vec::new() }))));
    }

    /// Plays a preview that `source` supplies piece by piece – until it ends
    /// or another preview (even an empty one = stop) replaces it.
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

    /// From now on the recording's samples are also collected for the live
    /// analysis ([`Engine::take_input_tap`]); `false` stops that.
    pub fn set_input_tap(&self, on: bool) {
        self.shared.set_tap(on);
    }

    /// Recording samples since the last call (mono, at the rate
    /// [`Engine::start_recording`] returned).
    pub fn take_input_tap(&self) -> Vec<f32> {
        self.shared.take_tap()
    }

    /// Records from `input` instead of a device from the next recording on
    /// (`None`: the device again) – for automated tests, which must not
    /// listen to a real microphone.
    pub fn set_fake_input(&self, input: Option<FakeInput>) {
        self.send(Control::SetFakeInput(input));
    }

    /// Reads the status; peak levels are reset in the process.
    pub fn status(&self) -> EngineStatus {
        self.shared.status(self.shared.sample_rate.load(Ordering::Relaxed))
    }

    /// The status without resetting the peak levels (for work on the side;
    /// the levels belong to the UI).
    pub fn peek_status(&self) -> EngineStatus {
        self.shared.peek(self.shared.sample_rate.load(Ordering::Relaxed))
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

// ------------------------------------------------------------------ Engine thread

/// An open output stream together with the ring buffers to its callback.
struct Output {
    stream: cpal::Stream,
    rt_tx: Producer<RtMsg>,
    garbage_rx: Consumer<Garbage>,
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

/// Opens an output device (`None` = system default) with a fresh callback;
/// the stream is not running yet.
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
    let (garbage_tx, garbage_rx) = RingBuffer::<Garbage>::new(crate::GARBAGE);
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

/// Message to the callback; if the ring buffer is full, wait briefly.
fn push_rt(rt_tx: &mut Producer<RtMsg>, msg: RtMsg) {
    let mut m = msg;
    while let Err(rtrb::PushError::Full(back)) = rt_tx.push(m) {
        m = back;
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// What the callback knows about the transport – so that a new stream after a
/// device switch continues exactly the same way.
#[derive(Default)]
pub(crate) struct Mirror {
    tempo: Option<(f64, u32)>,
    metronome: bool,
    looping: Option<(f64, f64)>,
    stop_at: Option<(f64, f64)>,
    /// Arrangement and the sample rate it is rendered for.
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
                // Tempo comes with the arrangement (no block with new tempo and old sound)
                if let Some(t) = tempo {
                    self.tempo = Some(*t);
                }
                self.arrangement = Some((arrangement.clone(), sample_rate));
            }
            _ => {}
        }
    }

    /// Messages that bring a fresh callback with `sample_rate` up to this
    /// state; the arrangement only if it is rendered for this rate.
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

/// Starts `next` in place of `out`: end the old stream, bring the new
/// callback up to date (position in beats is kept), start playing.
fn hand_over(out: &mut Output, next: Output, shared: &Shared, mirror: &Mirror) -> Result<(), EngineError> {
    let old_sr = out.sample_rate.max(1);
    let old = std::mem::replace(out, next);
    let _ = old.stream.pause();
    drop(old); // Callback is in place; its state lives in `shared` and `mirror`
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
    // Clock for streamed preview (seconds since the thread started)
    let clock = Instant::now();
    let mut input: Option<InputCapture> = None;
    let mut fake_input: Option<FakeInput> = None;
    let mut stream_preview: Option<PreviewStream> = None;
    let busy = || EngineError(tr!("The device can't be changed during a recording."));
    loop {
        let sr = out.sample_rate as f64;
        let ahead = (STREAM_AHEAD_SECS * sr) as u64;
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(Control::Rt(msg)) => {
                // any other preview (or stop) ends a streamed one
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
                                    // new device does not play: back to the previous one
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
            Ok(Control::SetFakeInput(f)) => fake_input = f,
            Ok(Control::StartRecording(reply)) => {
                input = None;
                let started = match &fake_input {
                    Some(f) => Ok(InputCapture::fake(f, shared.clone())),
                    None => InputCapture::start(&host, input_choice.as_deref(), shared.clone()),
                };
                match started {
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
            // Clock time instead of audio position: the callback only reports that after switching
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

/// Schedules the audio thread ahead of all normal threads so that CPU load on
/// all cores (pre-rendering after a tempo change) does not displace it –
/// otherwise blocks are missing and it crackles. cpal 0.15 tries this itself,
/// but passes a thread identifier instead of a handle; the call stays ineffective.
#[cfg(windows)]
fn boost_audio_thread() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;
    // SAFETY: pseudo handle of the own thread, only its priority changes.
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
                // how much of the real-time budget the mixer needs (status → pre-rendering when it gets tight)
                let t = std::time::Instant::now();
                r.process(data);
                let frames = data.len() / r.channels;
                r.note_time(t.elapsed().as_secs_f32(), frames);
            },
            |e| eprintln!("Audio-Ausgabefehler: {e}"),
            None,
        )
        .map_err(err)
}

// ------------------------------------------------------------------ Recording

struct InputCapture {
    source: InputSource,
    consumer: Consumer<f32>,
    samples: Vec<f32>,
    sample_rate: u32,
    start_frame: u64,
    device: String,
    shared: Arc<Shared>,
}

/// Where the samples come from: the device's stream or a stand-in – only
/// held so that dropping it ends the recording.
#[allow(dead_code)]
enum InputSource {
    Device(cpal::Stream),
    Fake(FakeFeed),
}

/// Plays a [`FakeInput`] into the ring buffer at the pace of real time.
struct FakeFeed {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for FakeFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl InputCapture {
    /// Records from the selected device (`None` = system default).
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
            source: InputSource::Device(stream),
            consumer,
            samples: Vec::with_capacity(sample_rate as usize * 60),
            sample_rate,
            start_frame: shared.position.load(Ordering::Relaxed),
            device: name,
            shared,
        })
    }

    /// Records from the stand-in: a thread hands its samples over every few
    /// milliseconds, as many as real time has advanced (in a loop).
    fn fake(input: &FakeInput, shared: Arc<Shared>) -> Self {
        let sample_rate = input.sample_rate.max(1);
        let (mut producer, consumer) = RingBuffer::<f32>::new(sample_rate as usize * 8);
        let stop = Arc::new(AtomicBool::new(false));
        let (data, flag, sh) = (input.samples.clone(), stop.clone(), shared.clone());
        let thread = std::thread::Builder::new()
            .name("klang-fake-input".into())
            .spawn(move || {
                let start = Instant::now();
                let mut sent = 0usize;
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                    let due = (start.elapsed().as_secs_f64() * sample_rate as f64) as usize;
                    let mut peak = 0f32;
                    while sent < due {
                        let v = if data.is_empty() { 0.0 } else { data[sent % data.len()] };
                        peak = peak.max(v.abs());
                        let _ = producer.push(v);
                        sent += 1;
                    }
                    update_peak(&sh.peak_in, peak);
                }
            })
            .ok();
        Self {
            source: InputSource::Fake(FakeFeed { stop, thread }),
            consumer,
            samples: Vec::with_capacity(sample_rate as usize * 60),
            sample_rate,
            start_frame: shared.position.load(Ordering::Relaxed),
            device: input.name.clone(),
            shared,
        }
    }

    fn drain(&mut self) {
        let from = self.samples.len();
        while let Ok(v) = self.consumer.pop() {
            self.samples.push(v);
        }
        self.shared.tap_push(&self.samples[from..]);
    }

    fn finish(self) -> Recording {
        let InputCapture { source, mut consumer, mut samples, sample_rate, start_frame, device, .. } = self;
        drop(source);
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

