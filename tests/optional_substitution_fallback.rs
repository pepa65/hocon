use hocon_::*;

// A missing optional substitution keeps the previous value of the key. That
// previous value can be a concatenation or another substitution, which needs a
// real resolution and not a string rendering.

fn load(s: &str) -> Hocon {
    HoconLoader::new()
        .load_str(s)
        .expect("during test")
        .hocon()
        .expect("during test")
}

#[test]
fn fallback_to_a_substitution_keeps_the_previous_value() {
    assert_eq!(
        load("a = ${?x}\na = ${?missing}\n")["a"],
        load("a = ${?x}\n")["a"]
    );
}

#[test]
fn fallback_to_a_concatenation_keeps_the_previous_value() {
    assert_eq!(
        load("a = x ${?gone} y\na = ${?missing}\n")["a"],
        Hocon::String(String::from("x  y"))
    );
}
