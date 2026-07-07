//! Sanity check that StreamingPty actually delivers bytes via drain().
use std::thread;
use std::time::Duration;
use tako_term::pty::StreamingPty;

#[test]
fn streaming_pty_drains_output() {
    let mut pty = StreamingPty::spawn_shell(80, 24).expect("spawn");
    pty.write(b"echo streaming-marker-Z\n").unwrap();
    let mut got = Vec::new();
    for _ in 0..25 {
        thread::sleep(Duration::from_millis(40));
        got.extend(pty.drain());
        if got.windows(20).any(|w| w == b"streaming-marker-Z") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&got);
    assert!(
        text.contains("streaming-marker-Z"),
        "StreamingPty produced no marker; got {} bytes: {:?}",
        got.len(),
        text.chars().take(200).collect::<String>()
    );
}
