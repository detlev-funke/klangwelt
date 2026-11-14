//! Shows suggestions, cadences and a four-part setting:
//! `cargo run -p klang-music --example harmonie -- "C-Dur" I IV V7 I`
use klang_music::harmony::{cadences, describe, parse_chord, parse_key, suggest, voice_progression};
use klang_music::theory::Key;

fn name(m: u8) -> String {
    const N: [&str; 12] = ["c", "cis", "d", "es", "e", "f", "fis", "g", "gis", "a", "b", "h"];
    format!("{}{}", N[m as usize % 12], m as i32 / 12 - 1)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let key: Key = args.first().and_then(|k| parse_key(k)).unwrap_or(Key { tonic: 0, minor: false, confidence: 1.0 });
    let chords: Vec<_> = args.iter().skip(1).filter_map(|s| parse_chord(s, &key)).collect();
    println!("Tonart {}", key.name_de());
    let voices = voice_progression(&chords, Some(&key), None);
    for (c, v) in chords.iter().zip(&voices) {
        let i = describe(c, &key);
        println!("{:8} {:8} {:10} {}", i.name, i.roman, i.function, v.iter().map(|m| name(*m)).collect::<Vec<_>>().join(" "));
    }
    println!("\nFortsetzungen:");
    for s in suggest(&key, &chords, 10) {
        println!("  {:5.2} {:10?} {:7} {:8} {:10} {}{}", s.score, s.category, s.info.name, s.info.roman, s.info.function, s.reason,
            s.to_key.map(|k| format!(" → {k}")).unwrap_or_default());
    }
    println!("\nKadenzen:");
    for t in cadences(&key) {
        println!("  {:28} {}", t.name, t.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(" – "));
    }
}
