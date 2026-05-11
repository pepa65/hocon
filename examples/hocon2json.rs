use std::env;

use serde_json::{Number, Value};

use hocon_::{Error, Hocon, HoconLoader};

fn hocon_to_json(hocon: Hocon) -> Option<Value> {
    match hocon {
        Hocon::Boolean(b) => Some(Value::Bool(b)),
        Hocon::Integer(i) => Some(Value::Number(Number::from(i))),
        Hocon::Real(f) => {
            // If float is a whole number, output as integer for JSON compatibility
            if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                Some(Value::Number(Number::from(f as i64)))
            } else {
                Some(Value::Number(
                    Number::from_f64(f).unwrap_or(Number::from(0)),
                ))
            }
        }
        Hocon::String(s) => Some(Value::String(s)),
        Hocon::Array(vec) => Some(Value::Array(
            vec.into_iter().filter_map(hocon_to_json).collect(),
        )),
        Hocon::Hash(map) => Some(Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, hocon_to_json(v)))
                .filter_map(|(k, v)| v.map(|v| (k, v)))
                .collect(),
        )),
        Hocon::Null => Some(Value::Null),
        Hocon::BadValue(_) => None,
    }
}

fn parse_to_json(path: &str) -> Result<String, Error> {
    let hocon = HoconLoader::new().no_system().load_file(path)?.hocon()?;
    let json: Option<_> = hocon_to_json(hocon);
    serde_json::to_string_pretty(&json).map_err(|e| Error::Deserialization {
        message: e.to_string(),
    })
}

fn main() {
    match env::args().nth(1) {
        None => println!("please provide a HOCON file"),
        Some(file) => println!(
            "{}",
            parse_to_json(&file).unwrap_or_else(|_| String::from(""))
        ),
    }
}
