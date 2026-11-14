//! Builds libopus (Xiph, BSD license, sources under `opus/`) directly with the
//! C compiler – without CMake. Which files belong to it is listed in libopus's
//! own `*_sources.mk`; the floating-point version is built, without
//! processor-specific paths and without neural networks.

use std::path::Path;

/// Files of a variable from a libopus `.mk` file (`NAME = \ a.c \ b.c`).
fn list(mk: &str, var: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in mk.lines() {
        let t = line.trim();
        if !inside {
            if let Some(rest) = t.strip_prefix(var) {
                if rest.trim_start().starts_with('=') {
                    inside = true;
                    out.extend(rest.trim_start()[1..].split_whitespace().filter(|w| w.ends_with(".c")).map(String::from));
                    if !t.ends_with('\\') {
                        break;
                    }
                }
            }
            continue;
        }
        out.extend(t.split_whitespace().filter(|w| w.ends_with(".c")).map(String::from));
        if !t.ends_with('\\') {
            break;
        }
    }
    assert!(!out.is_empty(), "{var} nicht gefunden");
    out
}

fn main() {
    let root = Path::new("opus");
    let read = |f: &str| std::fs::read_to_string(root.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
    let (celt, silk, opus) = (read("celt_sources.mk"), read("silk_sources.mk"), read("opus_sources.mk"));
    let mut files = list(&celt, "CELT_SOURCES");
    files.extend(list(&silk, "SILK_SOURCES"));
    files.extend(list(&silk, "SILK_SOURCES_FLOAT"));
    files.extend(list(&opus, "OPUS_SOURCES"));
    files.extend(list(&opus, "OPUS_SOURCES_FLOAT"));

    let mut b = cc::Build::new();
    for dir in ["include", "celt", "silk", "silk/float", "src", "."] {
        b.include(root.join(dir));
    }
    b.define("OPUS_BUILD", None)
        .define("HAVE_LRINT", Some("1"))
        .define("HAVE_LRINTF", Some("1"))
        .warnings(false)
        .opt_level(2);
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        wasm(&mut b);
    } else {
        b.define("USE_ALLOCA", None);
    }
    for f in &files {
        b.file(root.join(f));
    }
    b.compile("opus");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=opus");
    println!("cargo:rerun-if-changed=wasm");
}

/// Browser (wasm32-unknown-unknown): libopus with clang from LLVM, without libc.
/// The few headers libopus needs from the standard library
/// live under `wasm/include`; math and `mem*` come from Rust's
/// compiler_builtins, memory from `src/wasm_libc.rs`. Scratch buffers as
/// C99 variable-length arrays instead of `alloca`.
fn wasm(b: &mut cc::Build) {
    b.define("VAR_ARRAYS", None).include("wasm/include").flag("-ffreestanding").flag("-fno-stack-protector");
    for var in ["CC_wasm32_unknown_unknown", "TARGET_CC", "CC", "AR_wasm32_unknown_unknown", "TARGET_AR", "AR"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    if std::env::var_os("CC_wasm32_unknown_unknown").is_some() || std::env::var_os("TARGET_CC").is_some() {
        return; // explicitly set – cc uses this compiler
    }
    let exe = if cfg!(windows) { ".exe" } else { "" };
    let on_path = |name: &str| {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).map(|d| d.join(format!("{name}{exe}"))).find(|f| f.is_file()))
            .unwrap_or(None)
    };
    let clang = on_path("clang").or_else(|| {
        let f = Path::new(r"C:\Program Files\LLVM\bin\clang.exe");
        (cfg!(windows) && f.is_file()).then(|| f.to_path_buf())
    });
    let Some(clang) = clang else {
        panic!("Für die Browser-Version braucht libopus clang aus LLVM (Windows: winget install LLVM.LLVM)");
    };
    let ar = clang.with_file_name(format!("llvm-ar{exe}"));
    b.compiler(&clang);
    if ar.is_file() {
        b.archiver(ar);
    }
}
