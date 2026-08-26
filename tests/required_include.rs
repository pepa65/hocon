use hocon_::HoconLoader;

#[test]
fn required_include_of_existing_file_is_merged() {
    let doc = HoconLoader::new()
        .load_file("tests/data/include_required_test.conf")
        .expect("should load file with a required include")
        .hocon()
        .expect("should parse HOCON");

    assert_eq!(
        doc["a"].as_i64(),
        Some(5),
        "keys from the required include should be merged into the document"
    );
    assert_eq!(
        doc["additional"].as_string().as_deref(),
        Some("value"),
        "sibling keys should survive the include"
    );
}

#[test]
fn required_include_of_missing_file_fails() {
    let result = HoconLoader::new()
        // Not under tests/data/, because tests/load.rs generates a "must load"
        // test for every tests/data/*.conf and this file must fail to load.
        .load_file("tests/data_invalid/include_required_missing.conf")
        .and_then(|loader| loader.hocon());

    assert!(
        result.is_err(),
        "a required include of a missing file must fail, got {result:?}"
    );
}

#[test]
fn plain_include_of_missing_file_still_succeeds() {
    let doc = HoconLoader::new()
        .load_file("tests/data/include_optional_missing.conf")
        .expect("a plain include of a missing file must not fail")
        .hocon()
        .expect("should parse HOCON");

    assert_eq!(
        doc["a"].as_i64(),
        Some(5),
        "the rest of the document should still be readable"
    );
}

#[test]
fn required_include_inside_braces_is_merged() {
    let doc = HoconLoader::new()
        .load_file("tests/data/include_required_nested.conf")
        .expect("should load a required include nested in an object")
        .hocon()
        .expect("should parse HOCON");

    assert_eq!(
        doc["modules"]["first"]["a"].as_i64(),
        Some(5),
        "the included keys should land under the enclosing key"
    );
    assert_eq!(doc["other"].as_string().as_deref(), Some("kept"));
}

#[test]
fn required_include_after_a_comment_is_merged() {
    let doc = HoconLoader::new()
        .load_file("tests/data/include_required_after_comment.conf")
        .expect("should load a required include preceded by a comment")
        .hocon()
        .expect("should parse HOCON");

    assert_eq!(
        doc["a"].as_i64(),
        Some(5),
        "the include should still resolve"
    );
    assert_eq!(
        doc["additional_key"].as_string().as_deref(),
        Some("test_value")
    );
}
