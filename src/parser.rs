use std::borrow::Cow;
use std::rc::Rc;

use nom::Err as NomErr;
use nom::IResult;
use nom::Parser;
use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::bytes::complete::take_until;
use nom::character::complete::char;
use nom::character::complete::digit1;
use nom::character::complete::multispace0;
use nom::character::complete::newline;
use nom::character::complete::none_of;
use nom::character::complete::one_of;
use nom::combinator::not;
use nom::combinator::opt;
use nom::combinator::recognize;
use nom::combinator::value as nom_value;
use nom::error::Error as NomError;
use nom::error::ErrorKind;
use nom::error::ParseError;
use nom::multi::many0;
use nom::multi::many1;
use nom::multi::separated_list0;
use nom::sequence::delimited;
use nom::sequence::pair;

use crate::HoconLoaderConfig;
use crate::Result;
use crate::helper;
use crate::internals::Hash;
use crate::internals::HoconInternal;
use crate::internals::HoconValue;
use crate::internals::Include;
use crate::internals::unescape;

/// Root parser - the main entry point for parsing HOCON documents.
pub(crate) fn root<'a>(
    config: &'a HoconLoaderConfig,
) -> impl FnMut(&'a str) -> IResult<&'a str, Result<HoconInternal>> {
    move |input| {
        let (input, _) = possible_comment(input)?;

        // Try root_include first
        if let Ok((remaining, result)) = root_include(config)(input) {
            let (remaining, _) = possible_comment(remaining)?;
            return Ok((remaining, result));
        }

        // Try root_hash (object without braces)
        if let Ok((remaining, h)) = root_hash(config)(input) {
            let (remaining, _) = possible_comment(remaining)?;
            return Ok((remaining, h.map(HoconInternal::from_object)));
        }

        // Try hash (object with braces)
        if let Ok((remaining, h)) = hash(config)(input) {
            let (remaining, _) = possible_comment(remaining)?;
            return Ok((remaining, h.map(HoconInternal::from_object)));
        }

        // Try array
        if let Ok((remaining, a)) = array(config)(input) {
            let (remaining, _) = possible_comment(remaining)?;
            return Ok((remaining, a.map(HoconInternal::from_array)));
        }

        Err(NomErr::Error(NomError::new(input, ErrorKind::Alt)))
    }
}

// ============================================================================
// Basic whitespace and comment parsers
// ============================================================================

fn space(input: &str) -> IResult<&str, ()> {
    let (remaining, _) = many0(alt((
        tag(" "),
        tag("\t"),
        tag("\u{feff}"),
        tag("\u{00a0}"),
        tag("\u{2007}"),
        tag("\u{202f}"),
    )))
    .parse(input)?;
    Ok((remaining, ()))
}

fn sp<'a, O, F>(mut f: F) -> impl FnMut(&'a str) -> IResult<&'a str, O>
where
    F: Parser<&'a str, Output = O, Error = NomError<&'a str>>,
{
    move |input| {
        let (input, _) = space(input)?;
        let (input, parsed) = f.parse(input)?;
        let (input, _) = space(input)?;
        Ok((input, parsed))
    }
}

fn possible_comment(input: &str) -> IResult<&str, Option<()>> {
    opt(multiline_comment).parse(input)
}

fn multiline_comment(input: &str) -> IResult<&str, ()> {
    let (remaining, _) = many0(newline).parse(input)?;
    let (remaining, _) = space(remaining)?;
    let (remaining, _) = comment(remaining)?;
    let (remaining, _) = many0(alt((newline.map(|_| ()), space_then_comment))).parse(remaining)?;
    let (remaining, _) = multispace0(remaining)?;
    Ok((remaining, ()))
}

fn space_then_comment(input: &str) -> IResult<&str, ()> {
    let (remaining, _) = space(input)?;
    comment(remaining)
}

fn comment(input: &str) -> IResult<&str, ()> {
    let (remaining, _) = alt((tag("//"), tag("#"))).parse(input)?;
    let (remaining, _) = take_until("\n").parse(remaining)?;
    Ok((remaining, ()))
}

// ============================================================================
// Primitive value parsers
// ============================================================================

/// Recognizes a number that conforms to JSON/HOCON spec.
/// Requires at least one digit before the decimal point (so `.33` is NOT valid, but `0.33` is).
/// Format: [-]digits[.digits][e[+-]digits]
fn recognize_number(input: &str) -> IResult<&str, &str> {
    recognize((
        opt(char('-')),
        digit1,
        opt(pair(char('.'), digit1)),
        opt((one_of("eE"), opt(one_of("+-")), digit1)),
    ))
    .parse(input)
}

fn integer(input: &str) -> IResult<&str, i64> {
    let (remaining, parsed) = recognize_number(input)?;
    match parsed.parse::<i64>() {
        Ok(val) => Ok((remaining, val)),
        Err(_) => Err(NomErr::Error(NomError::new(input, ErrorKind::Digit))),
    }
}

fn float(input: &str) -> IResult<&str, f64> {
    let (remaining, parsed) = recognize_number(input)?;
    match parsed.parse::<f64>() {
        Ok(val) => Ok((remaining, val)),
        Err(_) => Err(NomErr::Error(NomError::new(input, ErrorKind::Float))),
    }
}

fn boolean(input: &str) -> IResult<&str, bool> {
    alt((nom_value(true, tag("true")), nom_value(false, tag("false")))).parse(input)
}

// ============================================================================
// String parsers
// ============================================================================

fn take_while_m_n<F>(min: usize, max: usize, cond: F) -> impl Fn(&str) -> IResult<&str, &str>
where
    F: Fn(char) -> bool,
{
    move |input: &str| {
        let mut count = 0;
        let mut end_idx = 0;
        for (idx, c) in input.char_indices() {
            if count >= max {
                break;
            }
            if cond(c) {
                count += 1;
                end_idx = idx + c.len_utf8();
            } else {
                break;
            }
        }
        if count >= min {
            Ok((&input[end_idx..], &input[..end_idx]))
        } else {
            Err(NomErr::Error(NomError::new(input, ErrorKind::TakeWhileMN)))
        }
    }
}

fn string(input: &str) -> IResult<&str, Cow<'_, str>> {
    fn escaped_char(input: &str) -> IResult<&str, &str> {
        alt((
            recognize(none_of("\\\"\n")),
            recognize(pair(char('\\'), one_of(r#""\/bfnrtu"#))),
            recognize((
                tag("\\u"),
                take_while_m_n(0, 4, |c: char| c.is_ascii_hexdigit()),
            )),
        ))
        .parse(input)
    }

    let (remaining, _) = char('"').parse(input)?;
    let (remaining, content) = recognize(many0(escaped_char)).parse(remaining)?;
    let (remaining, _) = char('"').parse(remaining)?;

    Ok((remaining, unescape(content)))
}

fn multiline_string(input: &str) -> IResult<&str, &str> {
    // Multiline strings start with """ and end with """
    // According to HOCON spec, if there are more than 3 consecutive closing quotes,
    // the extras are part of the string content. For example:
    // """foo"""" parses as foo" (4 quotes at end = 1 content quote + 3 closing)
    // """foo""""" parses as foo"" (5 quotes at end = 2 content quotes + 3 closing)
    let (remaining, _) = tag("\"\"\"").parse(input)?;

    // Find the position where 3+ consecutive quotes end
    // We need to find a sequence of 3+ quotes where what follows is NOT a quote
    let mut i = 0;
    let bytes = remaining.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'"' {
            // Count consecutive quotes
            let quote_start = i;
            while i < bytes.len() && bytes[i] == b'"' {
                i += 1;
            }
            let quote_count = i - quote_start;
            if quote_count >= 3 {
                // Found at least 3 consecutive quotes followed by non-quote or end
                // Content is everything before the last 3 quotes
                let content_end = quote_start + quote_count - 3;
                let content = &remaining[..content_end];
                let after_content = &remaining[content_end + 3..];
                return Ok((after_content, content));
            }
        } else {
            i += 1;
        }
    }

    // No closing """ found
    Err(NomErr::Error(NomError::new(input, ErrorKind::TakeUntil)))
}

fn unquoted_string(input: &str) -> IResult<&str, &str> {
    fn is_special_char(c: char) -> bool {
        matches!(
            c,
            '$' | '"'
                | '{'
                | '}'
                | '['
                | ']'
                | ':'
                | '='
                | ','
                | '+'
                | '#'
                | '`'
                | '^'
                | '?'
                | '!'
                | '@'
                | '*'
                | '&'
                | '\''
                | '\\'
                | '\t'
                | '\n'
        )
    }

    let mut end = 0;
    let mut chars = input.char_indices().peekable();
    while let Some((idx, c)) = chars.next() {
        if is_special_char(c) {
            break;
        }
        if c == '/'
            && let Some((_, '/')) = chars.peek()
        {
            break;
        }
        end = idx + c.len_utf8();
    }

    if end == 0 {
        return Err(NomErr::Error(NomError::new(input, ErrorKind::TakeWhile1)));
    }

    Ok((&input[end..], &input[..end]))
}

// ============================================================================
// Substitution parsers
// ============================================================================

fn path_substitution(input: &str) -> IResult<&str, HoconValue> {
    let (input, _) = alt((tag("${?"), tag("${"))).parse(input)?;
    let (input, val) = hocon_value(input)?;
    let (input, _) = char('}').parse(input)?;
    Ok((input, val))
}

fn optional_path_substitution(input: &str) -> IResult<&str, HoconValue> {
    let (input, _) = tag("${?").parse(input)?;
    let (input, val) = hocon_value(input)?;
    let (input, _) = char('}').parse(input)?;
    Ok((input, val))
}

// ============================================================================
// Value parsers
// ============================================================================

fn single_value(input: &str) -> IResult<&str, HoconValue> {
    alt((
        multiline_string.map(|s| HoconValue::String(Rc::from(s))),
        string.map(|s: Cow<str>| HoconValue::String(Rc::from(s.as_ref()))),
        integer.map(HoconValue::Integer),
        float.map(HoconValue::Real),
        boolean.map(HoconValue::Boolean),
        optional_path_substitution.map(|p| HoconValue::PathSubstitution {
            target: Box::new(p),
            optional: true,
            original: None,
        }),
        path_substitution.map(|p| HoconValue::PathSubstitution {
            target: Box::new(p),
            optional: false,
            original: None,
        }),
        unquoted_string.map(|s| HoconValue::UnquotedString(Rc::from(s))),
    ))
    .parse(input)
}

fn hocon_value(input: &str) -> IResult<&str, HoconValue> {
    let (input, _) = possible_comment(input)?;
    let (input, first_value) = single_value(input)?;
    let (input, remaining_values) = many0(single_value).parse(input)?;

    let result = if remaining_values.is_empty() {
        first_value
    } else {
        let mut values = vec![first_value];
        values.extend(remaining_values);
        HoconValue::maybe_concat(values)
    };

    Ok((input, result))
}

// ============================================================================
// Separator and utility parsers
// ============================================================================

fn ws<'a, O, E, F>(f: F) -> impl Parser<&'a str, Output = O, Error = E>
where
    E: ParseError<&'a str>,
    F: Parser<&'a str, Output = O, Error = E>,
{
    delimited(multispace0, f, multispace0)
}

fn separators(input: &str) -> IResult<&str, ()> {
    // Try multiline comment first
    if let Ok((remaining, _)) = sp(multiline_comment)(input) {
        let (remaining, _) = possible_comment(remaining)?;
        let (remaining, _) = multispace0(remaining)?;
        return Ok((remaining, ()));
    }

    // Try multiple newlines
    let newlines_parser = many1(newline).map(|_: Vec<_>| ());
    if let Ok((remaining, _)) = sp(newlines_parser)(input) {
        let (remaining, _) = possible_comment(remaining)?;
        let (remaining, _) = multispace0(remaining)?;
        return Ok((remaining, ()));
    }

    // Try comma with whitespace
    let (input, _) = multispace0(input)?;
    let (input, _) = char(',').parse(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = possible_comment(input)?;
    Ok((input, ()))
}

fn closing(input: &str, closing_char: char) -> IResult<&str, ()> {
    let (input, _) = opt(separators).parse(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = char(closing_char).parse(input)?;
    Ok((input, ()))
}

/// Helper function to parse colon or equals separator
fn colon_or_equals(input: &str) -> IResult<&str, char> {
    let (input, _) = multispace0(input)?;
    let result = alt((char::<&str, NomError<&str>>(':'), char('='))).parse(input);
    match result {
        Ok((remaining, c)) => {
            let (remaining, _) = multispace0(remaining)?;
            Ok((remaining, c))
        }
        Err(e) => Err(e),
    }
}

// ============================================================================
// Include parser
// ============================================================================

/// Parses the target of an `include`: `file(...)`, `url(...)`, or a bare quoted string.
fn include_target(input: &str) -> IResult<&str, Include<'_>> {
    alt((
        |i| {
            let (i, _) = tag("file(").parse(i)?;
            let (i, file_name) = string(i)?;
            let (i, _) = tag(")").parse(i)?;
            Ok((i, Include::File(file_name)))
        },
        |i| {
            let (i, _) = tag("url(").parse(i)?;
            let (i, url) = string(i)?;
            let (i, _) = tag(")").parse(i)?;
            Ok((i, Include::Url(url)))
        },
        string.map(Include::File),
    ))
    .parse(input)
}

fn include_parser(input: &str) -> IResult<&str, Include<'_>> {
    let (input, _) = tag("include ").parse(input)?;
    let (input, _) = ws(many0(newline)).parse(input)?;

    // Parse the include target with surrounding spaces
    let (input, _) = space(input)?;
    let (input, included) = alt((
        |i| {
            let (i, _) = tag("required(").parse(i)?;
            let (i, inner) = include_target(i)?;
            let (i, _) = tag(")").parse(i)?;
            Ok((i, inner.into_required()))
        },
        include_target,
    ))
    .parse(input)?;
    let (input, _) = space(input)?;

    Ok((input, included))
}

// ============================================================================
// Key-value parser (one of the most complex parsers)
// ============================================================================

fn key_value<'a>(
    config: &'a HoconLoaderConfig,
) -> impl FnMut(&'a str) -> IResult<&'a str, Result<Hash>> {
    move |input| {
        let (input, _) = ws(possible_comment).parse(input)?;

        // Try include first
        if let Ok((remaining, included)) = sp(include_parser)(input) {
            return Ok((
                remaining,
                HoconInternal::from_include(included, config).map(|h| h.internal),
            ));
        }

        // Try quoted string key with separator (:, =, or +=)
        if let Ok((remaining, key)) = ws(string).parse(input) {
            let key_str: Rc<str> = Rc::from(key.as_ref());

            // Check for +=
            if let Ok((remaining, _)) = ws(tag::<&str, &str, NomError<&str>>("+=")).parse(remaining)
            {
                let (remaining, val) = wrapper(config)(remaining)?;
                let item_id: Rc<str> = Rc::from(uuid::Uuid::new_v4().hyphenated().to_string());
                return Ok((
                    remaining,
                    val.map(|h| {
                        HoconInternal::from_object(h.internal)
                            .transform(|k, v| {
                                // Rc<[T]> makes subsequent clones cheap (ref count bump vs heap alloc)
                                let original_path: Rc<[HoconValue]> = k.clone().into();
                                (
                                    k,
                                    HoconValue::ToConcatToArray {
                                        value: Box::new(v),
                                        original_path,
                                        item_id: Rc::clone(&item_id),
                                    },
                                )
                            })
                            .add_to_path(vec![HoconValue::String(Rc::clone(&key_str))])
                            .internal
                    }),
                ));
            }

            // Check for : or =
            if let Ok((remaining, _)) = colon_or_equals(remaining) {
                let (remaining, val) = wrapper(config)(remaining)?;
                return Ok((
                    remaining,
                    val.map(|h| {
                        HoconInternal::from_object(h.internal)
                            .add_to_path(vec![HoconValue::String(Rc::clone(&key_str))])
                            .internal
                    }),
                ));
            }

            // Check for direct hash (no separator)
            if let Ok((remaining, h)) = hashes(config)(remaining) {
                return Ok((
                    remaining,
                    h.map(|hash| {
                        HoconInternal::from_object(hash)
                            .add_to_path(vec![HoconValue::String(Rc::clone(&key_str))])
                            .internal
                    }),
                ));
            }
        }

        // Try unquoted string key with separator (:, =, or +=)
        if let Ok((remaining, key)) = ws(unquoted_string).parse(input) {
            let key_str: Rc<str> = Rc::from(key);

            // Check for +=
            if let Ok((remaining, _)) = ws(tag::<&str, &str, NomError<&str>>("+=")).parse(remaining)
            {
                let (remaining, val) = wrapper(config)(remaining)?;
                let item_id: Rc<str> = Rc::from(uuid::Uuid::new_v4().hyphenated().to_string());
                return Ok((
                    remaining,
                    val.map(|h| {
                        HoconInternal::from_object(h.internal)
                            .transform(|k, v| {
                                // Rc<[T]> makes subsequent clones cheap (ref count bump vs heap alloc)
                                let original_path: Rc<[HoconValue]> = k.clone().into();
                                (
                                    k,
                                    HoconValue::ToConcatToArray {
                                        value: Box::new(v),
                                        original_path,
                                        item_id: Rc::clone(&item_id),
                                    },
                                )
                            })
                            .add_to_path(vec![HoconValue::UnquotedString(Rc::clone(&key_str))])
                            .internal
                    }),
                ));
            }

            // Check for : or =
            if let Ok((remaining, _)) = colon_or_equals(remaining) {
                let (remaining, val) = wrapper(config)(remaining)?;
                return Ok((
                    remaining,
                    val.map(|h| {
                        HoconInternal::from_object(h.internal)
                            .add_to_path(vec![HoconValue::UnquotedString(Rc::clone(&key_str))])
                            .internal
                    }),
                ));
            }

            // Check for direct hash (no separator)
            if let Ok((remaining, h)) = hashes(config)(remaining) {
                return Ok((
                    remaining,
                    h.map(|hash| {
                        HoconInternal::from_object(hash)
                            .add_to_path(vec![HoconValue::UnquotedString(Rc::clone(&key_str))])
                            .internal
                    }),
                ));
            }
        }

        Err(NomErr::Error(NomError::new(input, ErrorKind::Alt)))
    }
}

// ============================================================================
// Hash/Object parsers
// ============================================================================

fn separated_hashlist<'a>(
    config: &'a HoconLoaderConfig,
) -> impl FnMut(&'a str) -> IResult<&'a str, Result<Vec<Hash>>> {
    move |input| {
        let (input, parsed) = separated_list0(separators, key_value(config)).parse(input)?;
        Ok((input, helper::extract_result(parsed)))
    }
}

fn hash<'a>(
    config: &'a HoconLoaderConfig,
) -> impl FnMut(&'a str) -> IResult<&'a str, Result<Hash>> {
    move |input| {
        let (input, _) = space(input)?;
        let (input, _) = char('{').parse(input)?;
        let (input, hashlist) = separated_hashlist(config)(input)?;
        let (input, _) = closing(input, '}')?;
        let (input, _) = space(input)?;

        Ok((
            input,
            hashlist.map(|vec| vec.into_iter().flatten().collect()),
        ))
    }
}

fn hashes<'a>(
    config: &'a HoconLoaderConfig,
) -> impl FnMut(&'a str) -> IResult<&'a str, Result<Hash>> {
    move |input| {
        let (input, maybe_substitution) = opt(path_substitution).parse(input)?;
        let (input, first_hash) = hash(config)(input)?;
        let (input, remaining_hashes) = many0(hash(config)).parse(input)?;

        let result = match (maybe_substitution, remaining_hashes.is_empty()) {
            (None, true) => first_hash,
            (None, false) => match (first_hash, helper::extract_result(remaining_hashes)) {
                (Ok(mut values), Ok(hashes)) => {
                    hashes.into_iter().for_each(|mut h| values.append(&mut h));
                    Ok(values)
                }
                (Err(e), _) | (_, Err(e)) => Err(e),
            },
            (Some(subst), _) => {
                let mut values = vec![(
                    vec![],
                    HoconValue::PathSubstitution {
                        target: Box::new(subst),
                        optional: false,
                        original: None,
                    },
                )];
                match (first_hash, helper::extract_result(remaining_hashes)) {
                    (Ok(mut fh), Ok(hashes)) => {
                        values.append(&mut fh);
                        hashes.into_iter().for_each(|mut h| values.append(&mut h));
                        Ok(values)
                    }
                    (Err(e), _) | (_, Err(e)) => Err(e),
                }
            }
        };

        Ok((input, result))
    }
}

fn root_hash<'a>(
    config: &'a HoconLoaderConfig,
) -> impl FnMut(&'a str) -> IResult<&'a str, Result<Hash>> {
    move |input| {
        let (input, _) = space(input)?;
        // Make sure it doesn't start with '{'
        let (input, _) = not(char('{')).parse(input)?;
        let (input, hashlist) = separated_hashlist(config)(input)?;
        let (input, _) = space(input)?;

        Ok((
            input,
            hashlist.map(|vec| vec.into_iter().flatten().collect()),
        ))
    }
}

// ============================================================================
// Array parsers
// ============================================================================

fn array<'a>(
    config: &'a HoconLoaderConfig,
) -> impl FnMut(&'a str) -> IResult<&'a str, Result<Vec<HoconInternal>>> {
    move |input| {
        let (input, _) = sp(char('[')).parse(input)?;
        let (input, _) = multispace0(input)?;
        let (input, items) = separated_list0(separators, wrapper(config)).parse(input)?;
        let (input, _) = closing(input, ']')?;

        Ok((input, helper::extract_result(items)))
    }
}

fn arrays<'a>(
    config: &'a HoconLoaderConfig,
) -> impl FnMut(&'a str) -> IResult<&'a str, Result<Vec<HoconInternal>>> {
    move |input| {
        let (input, maybe_substitution) = opt(path_substitution).parse(input)?;
        let (input, first_array) = array(config)(input)?;
        let (input, remaining_arrays) = many0(array(config)).parse(input)?;

        let result = match (maybe_substitution, remaining_arrays.is_empty()) {
            (None, true) => first_array,
            (None, false) => match (first_array, helper::extract_result(remaining_arrays)) {
                (Ok(mut values), Ok(arrays)) => {
                    arrays
                        .into_iter()
                        .for_each(|mut arr| values.append(&mut arr));
                    Ok(values)
                }
                (Err(e), _) | (_, Err(e)) => Err(e),
            },
            (Some(subst), _) => {
                let mut values = vec![HoconInternal::from_value(
                    HoconValue::PathSubstitutionInParent(Box::new(subst)),
                )];
                match (first_array, helper::extract_result(remaining_arrays)) {
                    (Ok(mut fa), Ok(arrays)) => {
                        values.append(&mut fa);
                        arrays
                            .into_iter()
                            .for_each(|mut arr| values.append(&mut arr));
                        Ok(values)
                    }
                    (Err(e), _) | (_, Err(e)) => Err(e),
                }
            }
        };

        Ok((input, result))
    }
}

// ============================================================================
// Wrapper parser (handles all value types)
// ============================================================================

fn wrapper<'a>(
    config: &'a HoconLoaderConfig,
) -> impl FnMut(&'a str) -> IResult<&'a str, Result<HoconInternal>> {
    move |input| {
        let (input, _) = possible_comment(input)?;

        // Try hashes first
        if let Ok((remaining, h)) = hashes(config)(input) {
            return Ok((remaining, h.map(HoconInternal::from_object)));
        }

        // Try arrays
        if let Ok((remaining, a)) = arrays(config)(input) {
            return Ok((remaining, a.map(HoconInternal::from_array)));
        }

        // Try include
        if let Ok((remaining, included)) = include_parser(input) {
            return Ok((remaining, HoconInternal::from_include(included, config)));
        }

        // Try value
        let (remaining, val) = hocon_value(input)?;
        Ok((remaining, Ok(HoconInternal::from_value(val))))
    }
}

// ============================================================================
// Root parser (entry point)
// ============================================================================

fn root_include<'a>(
    config: &'a HoconLoaderConfig,
) -> impl FnMut(&'a str) -> IResult<&'a str, Result<HoconInternal>> {
    move |input| {
        let (input, included) = ws(include_parser).parse(input)?;
        let (input, doc) = root(config)(input)?;
        Ok((input, doc.and_then(|mut d| d.add_include(included, config))))
    }
}
