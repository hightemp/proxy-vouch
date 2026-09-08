use proxy_vouch_core::{parser::ImportOptions, session::Session};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for count in [10_000, 100_000] {
        let input = (0..count)
            .map(|i| format!("proxy-{i}.example:1080:user:pass socks5"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut session = Session::default();
        let started = Instant::now();
        let preview = session.preview(&input, &ImportOptions::default())?;
        assert_eq!(preview.valid, count);
        session.commit_import(false, false, true)?;
        let import_ms = started.elapsed().as_millis();
        let serialization = Instant::now();
        let snapshot = serde_json::to_vec(&session.snapshot(0))?;
        println!("{{\"records\":{count},\"import_ms\":{import_ms},\"snapshot_ms\":{},\"snapshot_bytes\":{}}}",serialization.elapsed().as_millis(),snapshot.len());
    }
    Ok(())
}
