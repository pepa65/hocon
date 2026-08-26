use std::cell::RefCell;
use std::rc::Rc;

use crate::Hocon;
use crate::HoconLoaderConfig;

use super::intermediate::Child;
use super::intermediate::HoconIntermediate;
use super::intermediate::KeyType;
use super::intermediate::Node;

#[derive(Clone, Debug)]
pub(crate) enum HoconValue {
    Real(f64),
    Integer(i64),
    String(Rc<str>),
    UnquotedString(Rc<str>),
    Boolean(bool),
    Concat(Vec<HoconValue>),
    PathSubstitution {
        target: Box<HoconValue>,
        optional: bool,
        original: Option<Box<HoconValue>>,
    },
    PathSubstitutionInParent(Box<HoconValue>),
    ToConcatToArray {
        value: Box<HoconValue>,
        original_path: Rc<[HoconValue]>,
        // an internal id, to keep track of the current parent object in case of an object to concat to an array
        item_id: Rc<str>,
    },
    Null(String),
    // Placeholder for a value that will be replaced before returning final document
    Temp,
    // Placeholder to mark an error when not processing document strictly
    BadValue(crate::Error),
    // Placeholder for an empty object
    EmptyObject,
    // Placeholder for an empty array
    EmptyArray,
    Included {
        value: Box<HoconValue>,
        include_root: Option<Vec<HoconValue>>,
        original_path: Rc<[HoconValue]>,
    },
}

impl HoconValue {
    pub(crate) fn maybe_concat(values: Vec<HoconValue>) -> HoconValue {
        let nb_values = values.len();
        let trimmed_values: Vec<HoconValue> = values
            .into_iter()
            .enumerate()
            .filter_map(|item| match item {
                (0, HoconValue::UnquotedString(ref s)) if s.trim() == "" => None,
                (i, HoconValue::UnquotedString(ref s)) if s.trim() == "" && i == nb_values - 1 => {
                    None
                }
                (_, v) => Some(v),
            })
            .collect();
        match trimmed_values {
            ref values if values.len() == 1 => {
                values.first().expect("unexpected empty values").clone()
            }
            values => HoconValue::Concat(values),
        }
    }

    pub(crate) fn to_path(&self) -> Vec<HoconValue> {
        match self {
            HoconValue::UnquotedString(s) if s.as_ref() == "." => vec![],
            HoconValue::UnquotedString(s) => s
                .trim()
                .split('.')
                .map(|s| HoconValue::String(Rc::from(s)))
                .collect(),
            HoconValue::String(s) => vec![HoconValue::String(Rc::clone(s))],
            HoconValue::Concat(values) => values.iter().flat_map(HoconValue::to_path).collect(),
            _ => vec![self.clone()],
        }
    }

    pub(crate) fn finalize(
        self,
        root: &HoconIntermediate,
        config: &HoconLoaderConfig,
        in_concat: bool,
        included_path: Option<Vec<HoconValue>>,
        substituting_path: Option<Vec<HoconValue>>,
    ) -> Result<Hocon, crate::Error> {
        match self {
            HoconValue::Null(_) => Ok(Hocon::Null),
            HoconValue::BadValue(err) => Ok(public_bad_value_or_err!(config, err)),
            HoconValue::Boolean(b) => Ok(Hocon::Boolean(b)),
            HoconValue::Integer(i) => Ok(Hocon::Integer(i)),
            HoconValue::Real(f) => Ok(Hocon::Real(f)),
            HoconValue::String(s) => Ok(Hocon::String(s.to_string())),
            HoconValue::UnquotedString(ref s) if s.as_ref() == "null" => Ok(Hocon::Null),
            HoconValue::UnquotedString(s) => {
                if in_concat {
                    Ok(Hocon::String(s.to_string()))
                } else {
                    Ok(Hocon::String(s.trim().to_string()))
                }
            }
            HoconValue::Concat(values) => Ok(Hocon::String({
                let nb_items = values.len();
                values
                    .into_iter()
                    .enumerate()
                    .map(|item| match item {
                        (0, HoconValue::UnquotedString(s)) => {
                            HoconValue::UnquotedString(Rc::from(s.trim_start()))
                        }
                        (i, HoconValue::UnquotedString(ref s)) if i == nb_items - 1 => {
                            HoconValue::UnquotedString(Rc::from(s.trim_end()))
                        }
                        (_, v) => v,
                    })
                    .map(|v| {
                        v.finalize(
                            root,
                            config,
                            true,
                            included_path.clone(),
                            substituting_path.clone(),
                        )
                    })
                    .filter_map(|v| v.ok().and_then(|v| v.as_internal_string()))
                    .collect::<Vec<String>>()
                    .join("")
            })),
            HoconValue::PathSubstitution {
                target: v,
                optional,
                original,
            } => {
                // second pass for substitution
                let fixed_up_path = if let Some(included_path) = included_path.clone() {
                    let mut fixed_up_path = included_path
                        .iter()
                        .cloned()
                        .flat_map(|path_item| path_item.to_path())
                        .collect::<Vec<_>>();
                    fixed_up_path.append(&mut v.to_path());
                    fixed_up_path
                } else {
                    v.to_path()
                };
                if Some(fixed_up_path.clone()) == substituting_path {
                    Ok(Hocon::Null)
                } else {
                    let fallback_included_path = included_path.clone();
                    let fallback_substituting_path = substituting_path;
                    match (
                        config.strict,
                        config.system,
                        root.tree
                            .find_key(config, fixed_up_path.clone())
                            .and_then(|v| {
                                v.finalize(root, config, included_path, Some(fixed_up_path))
                            }),
                    ) {
                        (_, true, Err(err)) | (_, true, Ok(Hocon::BadValue(err))) => {
                            match (
                                std::env::var(
                                    v.to_path()
                                        .into_iter()
                                        .map(HoconValue::string_value)
                                        .collect::<Vec<_>>()
                                        .join("."),
                                ),
                                optional,
                                original,
                            ) {
                                (Ok(val), _, _) => Ok(Hocon::String(val)),
                                // The previous value of the key can be any
                                // value, a concatenation or another
                                // substitution included, so resolve it the
                                // same way as a value found in the tree.
                                (_, true, Some(val)) => val.finalize(
                                    root,
                                    config,
                                    in_concat,
                                    fallback_included_path,
                                    fallback_substituting_path,
                                ),
                                _ => Ok(public_bad_value_or_err!(config, err)),
                            }
                        }
                        (true, _, Err(err)) | (true, _, Ok(Hocon::BadValue(err))) => Err(err),
                        (_, _, v) => v,
                    }
                }
            }
            HoconValue::Included {
                value,
                include_root,
                ..
            } => value.finalize(root, config, in_concat, include_root, None),
            // These cases should have been replaced during substitution
            // and not exist anymore at this point
            HoconValue::Temp => unreachable!(),
            HoconValue::EmptyObject => unreachable!(),
            HoconValue::EmptyArray => unreachable!(),
            HoconValue::PathSubstitutionInParent(_) => unreachable!(),
            HoconValue::ToConcatToArray { .. } => unreachable!(),
        }
    }

    pub(crate) fn string_value(self) -> String {
        match self {
            HoconValue::String(s) => s.to_string(),
            HoconValue::UnquotedString(s) => s.to_string(),
            HoconValue::Null(_) => String::from("null"),
            HoconValue::Integer(i) => i.to_string(),
            _ => unreachable!(),
        }
    }

    pub(crate) fn substitute(
        self,
        config: &HoconLoaderConfig,
        current_tree: &Rc<Child>,
        at_path: &[HoconValue],
    ) -> Result<Node, crate::Error> {
        match self {
            HoconValue::PathSubstitution {
                target: path,
                optional,
                original,
            } => {
                match current_tree.find_key(config, path.to_path()) {
                    Err(_) | Ok(Node::Leaf(HoconValue::BadValue(_))) => {
                        // If node is not found, keep substitution to try again on second pass
                        Ok(Node::Leaf(HoconValue::PathSubstitution {
                            target: path,
                            optional,
                            original,
                        }))
                    }
                    Ok(v) => Ok(v.deep_clone()),
                }
            }
            HoconValue::Concat(values) => {
                let substituted = crate::helper::extract_result(
                    values
                        .into_iter()
                        .map(|v| v.substitute(config, current_tree, at_path))
                        .map(|v| match v {
                            Ok(node) => Ok(node),
                            Err(err) => Ok(Node::Leaf(bad_value_or_err!(config, err))),
                        })
                        .collect::<Vec<_>>(),
                )?;

                if substituted
                    .iter()
                    .any(|node| matches!(node, Node::Node { .. }))
                {
                    let children = substituted
                        .into_iter()
                        .flat_map(|node| match node {
                            Node::Leaf(_) => vec![Rc::new(Child {
                                key: HoconValue::Integer(0),
                                value: RefCell::new(node),
                            })],
                            Node::Node { children, .. } => children,
                        })
                        .enumerate()
                        .filter_map(|(i, child)| match *child.value.borrow() {
                            Node::Leaf(HoconValue::UnquotedString(ref us))
                                if us.trim().is_empty() =>
                            {
                                None
                            }
                            _ => Some(Rc::new(Child {
                                key: HoconValue::Integer(i as i64),
                                value: child.value.clone(),
                            })),
                        })
                        .collect::<Vec<_>>();

                    Ok(Node::Node {
                        children,
                        key_hint: None,
                    })
                } else {
                    Ok(Node::Leaf(HoconValue::Concat(
                        substituted
                            .into_iter()
                            .filter_map(|node| {
                                if let Node::Leaf(value) = node {
                                    Some(value)
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>(),
                    )))
                }
            }
            HoconValue::EmptyObject => Ok(Node::Node {
                children: vec![],
                key_hint: Some(KeyType::String),
            }),
            HoconValue::EmptyArray => Ok(Node::Node {
                children: vec![],
                key_hint: Some(KeyType::Int),
            }),
            HoconValue::Included {
                value,
                original_path,
                include_root,
            } => {
                match *value.clone() {
                    HoconValue::PathSubstitution { target: path, .. }
                    | HoconValue::PathSubstitutionInParent(path) => {
                        let root_path = at_path
                            .iter()
                            .take(at_path.len() - original_path.len())
                            .cloned()
                            .flat_map(|path_item| path_item.to_path())
                            .collect::<Vec<_>>();
                        let mut fixed_up_path = root_path;
                        fixed_up_path.append(&mut path.to_path());
                        match current_tree.find_key(config, fixed_up_path) {
                            Ok(Node::Leaf(HoconValue::BadValue(_))) | Err(_) => (),
                            Ok(new_value) => {
                                return Ok(new_value.deep_clone());
                            }
                        }
                    }
                    HoconValue::Concat(values) => {
                        return HoconValue::Concat(
                            values
                                .into_iter()
                                .map(|value| HoconValue::Included {
                                    value: Box::new(value),
                                    original_path: original_path.clone(),
                                    include_root: include_root.clone(),
                                })
                                .collect(),
                        )
                        .substitute(config, current_tree, at_path);
                    }
                    _ => (),
                }

                match value.substitute(config, current_tree, at_path) {
                    Ok(Node::Leaf(value_found)) => {
                        // remember leaf was found inside an include
                        Ok(Node::Leaf(HoconValue::Included {
                            value: Box::new(value_found),
                            original_path,
                            include_root,
                        }))
                    }
                    v => v,
                }
            }
            v => Ok(Node::Leaf(v)),
        }
    }
}

impl PartialEq for HoconValue {
    fn eq(&self, rhs: &Self) -> bool {
        match (self, rhs) {
            (HoconValue::Integer(left), HoconValue::Integer(right)) => left == right,
            (HoconValue::String(left), HoconValue::String(right)) => left == right,
            (HoconValue::BadValue(left), HoconValue::BadValue(right)) => left == right,
            (HoconValue::Null(left), HoconValue::Null(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for HoconValue {}

impl std::hash::Hash for HoconValue {
    fn hash<H>(&self, state: &mut H)
    where
        H: std::hash::Hasher,
    {
        match self {
            HoconValue::Integer(i) => i.hash(state),
            HoconValue::String(s) => s.hash(state),
            HoconValue::UnquotedString(s) => s.hash(state),
            HoconValue::Null(s) => s.hash(state),
            _ => unreachable!(),
        };
    }
}
