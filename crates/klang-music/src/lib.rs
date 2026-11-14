//! Klangwelt-Musikbibliothek (symbolische Musik).
//!
//! * [`score`]    – eigenes Partiturmodell + MusicXML/MXL-Leser
//! * [`kern`]     – Humdrum-**kern-Leser (KernScores)
//! * [`dcml`]     – DCML-Notentabellen mit Akkordanalyse von Fachleuten
//! * [`melody`]   – Melodielinien und transpositions-/tempo-invariante Merkmale
//! * [`index`]    – n-Gramm-Index für die Melodiesuche („Summ mal was“)
//! * [`theory`]   – Tonarten, Bereinigung aufgenommener Noten
//! * [`analysis`] – Stimmfunktionen, Kadenzen, Stimmung (Heuristik, Stufe 1)
//! * [`source`]   – byte sources: file, memory, ranges of a file on a web server
//! * [`webpack`]  – the library package for the web (text index, facet table, chord statistics)
//! * [`scorezip`] – the ZIP with the scores of a package (written and read entry by entry)

pub mod analysis;
pub mod chordstats;
pub mod counterpoint;
pub mod dcml;
pub mod diskindex;
pub mod facets;
pub mod fetch;
pub mod fugue;
pub mod harmony;
pub mod index;
pub mod kern;
pub mod library;
pub mod melody;
pub mod pdmx;
pub mod score;
pub mod scorezip;
pub mod source;
pub mod theory;
pub mod webpack;

use std::fmt;

#[derive(Debug)]
pub enum MusicError {
    Io(std::io::Error),
    Xml(String),
    Zip(String),
    Unsupported(String),
}

impl fmt::Display for MusicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use klang_i18n::tr;
        f.write_str(&match self {
            MusicError::Io(e) => tr!("File error: {e}", e = e),
            MusicError::Xml(e) => tr!("XML error: {e}", e = e),
            MusicError::Zip(e) => tr!("MXL archive error: {e}", e = e),
            MusicError::Unsupported(e) => tr!("Not supported: {e}", e = e),
        })
    }
}

impl std::error::Error for MusicError {}

impl From<std::io::Error> for MusicError {
    fn from(e: std::io::Error) -> Self {
        MusicError::Io(e)
    }
}
