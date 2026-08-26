use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use hocon_::HoconLoader;

// An unclosed `${?` used to make the parser try the substitution, fail on the
// missing `}`, and then try the same text again as a plain string. The work
// doubled per `${?`, so a few dozen of them never finished. A missing `}` is
// now a hard parse error, so no second attempt follows. These tests measure
// only that the parse finishes, not that it succeeds. See
// `tests/unclosed_substitution.rs` for the result of each shape.

/// Parses `text` on a worker thread and reports whether it finished in time.
/// A parse that runs too long leaves its thread behind, so the test can fail
/// instead of hanging the whole suite.
fn parses_within(text: String, limit: Duration) -> bool {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = HoconLoader::new()
            .load_str(&text)
            .and_then(|loader| loader.hocon());
        let _ = sender.send(result.is_ok());
    });
    receiver.recv_timeout(limit).is_ok()
}

#[test]
fn many_unclosed_optional_substitutions_do_not_hang() {
    let text = format!("a = {}", "e${? ".repeat(64));

    assert!(
        parses_within(text, Duration::from_secs(10)),
        "parsing 64 unclosed optional substitutions must finish"
    );
}

#[test]
fn many_unclosed_substitutions_do_not_hang() {
    let text = format!("a = {}", "e${ ".repeat(64));

    assert!(
        parses_within(text, Duration::from_secs(10)),
        "parsing 64 unclosed substitutions must finish"
    );
}

#[test]
fn an_unclosed_substitution_around_closed_ones_does_not_hang() {
    let text = format!("a = ${{ {}", "${b} ".repeat(64));

    assert!(
        parses_within(text, Duration::from_secs(10)),
        "parsing an unclosed substitution over closed ones must finish"
    );
}
