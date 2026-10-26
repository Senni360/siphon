//! Siphon's native-messaging helper. Chrome starts it with the extension origin
//! as its argument; stdout carries only length-prefixed protocol frames.

mod analysis_service;
mod analyze_request;
mod desktop;
mod errors;
mod framing;
mod host;
mod jobs;
mod logging;
mod media;
mod options;
mod pipeline;
mod process;
mod runtime;
mod sponsorblock;
mod stems;
mod stems_engine;
mod youtube;
mod ytdlp;
mod ytdlp_update;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!(
            "siphon-host {} (protocol {})",
            host::VERSION,
            host::PROTOCOL_VERSION
        );
        return;
    }
    logging::init(runtime::siphon_dir().join("logs"), "host.log");
    logging::info(format!(
        "Siphon host {} starting (pid {})",
        host::VERSION,
        std::process::id()
    ));
    let host = host::Host::new(framing::Writer::new(std::io::stdout()));
    host.read_loop(&mut std::io::stdin().lock());
    logging::info("Native messaging input ended; exiting");
    // Job objects stop any yt-dlp/FFmpeg still running when this process ends.
    std::process::exit(0);
}
