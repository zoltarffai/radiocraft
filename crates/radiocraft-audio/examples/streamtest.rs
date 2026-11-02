//! Smoke helper: open a stream URL (or plain file), decode for a few
//! seconds, and print live metadata titles and sample counts
//! (see docs/06 §9 smoke infra).

use radiocraft_audio::decoder::{FileSource, Source};
use radiocraft_audio::StreamSource;

fn main() {
    let url = std::env::args().nth(1).expect("usage: streamtest <url-or-file>");
    let t0 = std::time::Instant::now();
    if !url.contains("://") {
        let mut src = match FileSource::open(std::path::Path::new(&url)) {
            Ok(s) => s,
            Err(e) => {
                println!("file open failed: {e}");
                std::process::exit(1);
            }
        };
        let mut frames = 0usize;
        loop {
            let n = src.read(&mut [0.0f32; 2048]);
            if n == 0 {
                break;
            }
            frames += n;
        }
        println!("file decoded {frames} frames in {:?}", t0.elapsed());
        return;
    }
    let mut src = match StreamSource::open(&url) {
        Ok(s) => {
            println!("open ok in {:?}", t0.elapsed());
            s
        }
        Err(e) => {
            println!("open failed after {:?}: {e}", t0.elapsed());
            std::process::exit(1);
        }
    };
    let title = src.live_title();
    let mut seen_version = 0u64;
    let mut frames = 0usize;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    while std::time::Instant::now() < deadline {
        let n = src.read(&mut [0.0f32; 2048]);
        if n == 0 {
            println!("EOF/restarting");
            continue;
        }
        frames += n;
        if let Some(cell) = &title {
            if let Some((v, t)) = cell.changed_since(seen_version) {
                seen_version = v;
                println!("title v{v}: {t:?}");
            }
        }
    }
    println!(
        "decoded {frames} frames, duration {:?}, in {:?}",
        src.duration(),
        t0.elapsed()
    );
}
