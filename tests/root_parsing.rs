use hocon_::{Hocon, HoconLoader};

fn load(input: &str) -> Hocon {
    HoconLoader::new()
        .load_str(input)
        .expect("load_str should succeed")
        .hocon()
        .expect("the document should parse")
}

#[test]
fn document_starting_with_blank_lines_is_parsed() {
    assert_eq!(load("\n\n  \n{ \"a\": 5 }")["a"].as_i64(), Some(5));
}

#[test]
fn top_level_array_is_parsed() {
    let doc = load("[1, 2, 3]");
    assert_eq!(doc[0].as_i64(), Some(1));
    assert_eq!(doc[2].as_i64(), Some(3));
}

#[test]
fn top_level_array_after_blank_lines_is_parsed() {
    assert_eq!(load("\n\n  \n[1, 2, 3]")[0].as_i64(), Some(1));
}

#[test]
fn empty_document_is_an_empty_object() {
    for input in ["", "\n", "\n  \n", "# just a comment\n"] {
        match load(input) {
            Hocon::Hash(hash) => assert!(hash.is_empty(), "{input:?} => {hash:?}"),
            other => panic!("{input:?} should be an empty object, got {other:?}"),
        }
    }
}
