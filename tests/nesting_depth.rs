use hocon_::{Hocon, HoconLoader};

// The parser recurses once per level of nesting. Without a bound, a deeply
// nested document overflows the stack and aborts the process. A Rust stack
// overflow is not catchable, so the caller cannot defend against it. The
// parser therefore rejects a document that nests deeper than MAX_DEPTH.

fn load(s: &str) -> Result<Hocon, hocon_::Error> {
    HoconLoader::new().load_str(s).and_then(|l| l.hocon())
}

fn nested_object(depth: usize) -> String {
    format!("x = {}1{}", "{y:".repeat(depth), "}".repeat(depth))
}

#[test]
fn an_object_nested_past_the_limit_is_an_error() {
    assert!(load(&nested_object(129)).is_err());
}

#[test]
fn a_substitution_nested_past_the_limit_is_an_error() {
    let text = format!("a = {}b{}", "${".repeat(129), "}".repeat(129));
    assert!(load(&text).is_err());
}

#[test]
fn an_array_nested_past_the_limit_is_an_error() {
    let text = format!("a = {}1{}", "[".repeat(129), "]".repeat(129));
    assert!(load(&text).is_err());
}

#[test]
fn an_object_nested_to_the_limit_still_parses() {
    assert!(load(&nested_object(128)).is_ok());
}

#[test]
fn an_array_nested_to_the_limit_still_parses() {
    let text = format!("a = {}1{}", "[".repeat(128), "]".repeat(128));
    assert!(load(&text).is_ok());
}

// The depth of one value must not count against its siblings. Real
// configuration files are wide and shallow, so a counter that never goes back
// down would reject them.
#[test]
fn many_siblings_do_not_add_up_to_the_limit() {
    let text: String = (0..500).map(|i| format!("k{i} = [[[{i}]]]\n")).collect();
    let doc = load(&text).expect("during test");
    assert_eq!(doc["k499"][0][0][0], Hocon::Integer(499));
}

// A run of `include` statements at the top of a document used to recurse: the
// root parser read one include, then parsed the rest of the document as a new
// document. About 1000 includes aborted the process.
//
// Past the limit the root parser stops recursing, and the remaining includes
// go through the ordinary key parser, which loops instead of recursing. The
// document therefore still parses. Only the abort is gone.
#[test]
fn a_long_run_of_includes_parses_instead_of_aborting() {
    let text = format!("{}a = 1\n", "include \"nope\"\n".repeat(20_000));
    assert_eq!(load(&text).expect("during test")["a"], Hocon::Integer(1));
}

#[test]
fn a_short_run_of_includes_still_parses() {
    let text = format!("{}a = 1\n", "include \"nope\"\n".repeat(3));
    assert_eq!(load(&text).expect("during test")["a"], Hocon::Integer(1));
}

// Before the limit existed, this input overflowed the stack and aborted the
// process. The test passing at all is the point: the parse returns.
#[test]
fn a_pathological_depth_returns_an_error_instead_of_aborting() {
    let text = format!("a = {}1{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(load(&text).is_err());
}
