//! Messung (nur Ausgabe, stumm): Hält der Audio-Callback mit, wenn alle Kerne
//! rechnen (z. B. Vorab-Rendern nach einer Tempoänderung)? Fällt der Callback
//! aus, spielt das Gerät eine Lücke und die Wiedergabeposition bleibt hinter
//! der Uhr zurück – gemessen wird diese Drift (Rauschen etwa ±10 ms, je nach
//! Puffergröße des Geräts).
//! `cargo test --release -p klang-engine --test audio_unter_last -- --ignored --nocapture`
//!
//! Gemessen (8 Kerne, Windows): ohne die Prioritätskorrektur in `device.rs`
//! fehlten bei 32 rechnenden Threads 24–172 ms je 3 s, bei 8 vereinzelt bis
//! 41 ms; mit ihr bleibt alles im Rauschen.

#![cfg(not(target_arch = "wasm32"))]

use klang_engine::{Arrangement, Engine, PlacedClip};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Drift (ms) der Position gegenüber der Uhr über `secs` Sekunden, während
/// `busy` Threads rechnen.
fn drift_ms(engine: &Engine, secs: f64, busy: usize) -> f64 {
    // Ablesen jeweils im Leerlauf: unter Last würde auch der messende Thread verdrängt
    let sr = engine.info().sample_rate as f64;
    std::thread::sleep(Duration::from_millis(500));
    let (t0, p0) = (Instant::now(), engine.status().position_frames);
    let stop = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..busy)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut x = 1.0f64;
                while !stop.load(Ordering::Relaxed) {
                    for _ in 0..10_000 {
                        x = (x * 1.000_001 + 0.5).sin();
                    }
                }
                x
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs_f64(secs));
    stop.store(true, Ordering::Relaxed);
    for w in workers {
        let _ = w.join();
    }
    std::thread::sleep(Duration::from_millis(500));
    let (t1, p1) = (t0.elapsed().as_secs_f64(), engine.status().position_frames);
    (t1 - (p1 - p0) as f64 / sr) * 1e3
}

#[test]
#[ignore = "Messung am Audiogerät"]
fn audio_unter_last() {
    let Ok(engine) = Engine::start() else {
        println!("kein Audiogerät – übersprungen");
        return;
    };
    engine.set_muted(true);
    let sr = engine.info().sample_rate;
    let data: Arc<[f32]> = vec![0.1f32; sr as usize * 120].into();
    engine.set_arrangement(Arrangement { clips: vec![PlacedClip { start_frame: 0, data, side: None, gain_l: 1.0, gain_r: 1.0, send: 0.3 }] });
    engine.play();
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    println!("{} ({} Hz), {cores} Kerne", engine.info().output_device, sr);
    for busy in [0, cores, 4 * cores] {
        let mut d = Vec::new();
        for _ in 0..5 {
            d.push(drift_ms(&engine, 3.0, busy));
        }
        println!("{busy:>2} rechnende Threads: Drift je 3 s {:?} ms", d.iter().map(|v| (v * 10.0).round() / 10.0).collect::<Vec<_>>());
    }
    engine.stop();
}
