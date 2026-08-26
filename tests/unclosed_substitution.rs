use hocon_::{Hocon, HoconLoader};

// HOCON needs a `}` after `${` or `${?`. A document without it is a syntax
// error. The parser used to try the same text again as a plain string, which
// dropped the rest of the document without a message.

fn load(s: &str) -> Result<Hocon, hocon_::Error> {
    HoconLoader::new().load_str(s).and_then(|l| l.hocon())
}

#[test]
fn an_unclosed_substitution_after_text_is_an_error() {
    assert!(load("a = x${?foo").is_err());
}

#[test]
fn an_unclosed_optional_substitution_is_an_error() {
    assert!(load("a = ${?foo").is_err());
}

#[test]
fn an_unclosed_required_substitution_is_an_error() {
    assert!(load("a = ${foo").is_err());
}

#[test]
fn an_unclosed_substitution_with_a_space_in_the_path_is_an_error() {
    assert!(load("a = ${foo bar").is_err());
}

// `a = [1, ${foo` is not here. The array itself is unclosed, and an unclosed
// array leaves its text over for the caller, which only `strict` mode rejects.
// `a = [1, 2` behaves the same way, so that shape does not measure this fix.

#[test]
fn a_closed_substitution_still_resolves() {
    assert_eq!(
        load("a = 1\nb = ${a}\n").expect("during test")["b"],
        Hocon::Integer(1)
    );
}

#[test]
fn a_closed_optional_substitution_after_text_still_resolves() {
    assert_eq!(
        load("a = x${?gone}y").expect("during test")["a"],
        Hocon::String(String::from("xy"))
    );
}

#[test]
fn an_unclosed_brace_in_a_quoted_string_is_text() {
    assert_eq!(
        load("a = \"${foo\"").expect("during test")["a"],
        Hocon::String(String::from("${foo"))
    );
}
