//! The `.proto` file the WIT becomes: what a window manager in another
//! language gives its protobuf generator. It describes exactly what
//! `rust.rs` writes, in protobuf's own words.
//!
//! - A record is a message, its fields numbered from 1 in the WIT's order.
//! - A variant is a message with one `oneof kind`, its cases numbered from 1.
//!   A case that carries nothing is a `Unit`.
//! - An enum is an enum, its cases numbered from 0 and named after it, as
//!   protobuf's style has them (`EASING_OUT_CUBIC`).
//! - A list or an option that is a variant's case, and a list that may be
//!   absent, is a message of its own holding it as field 1 (`U64List`,
//!   `OptionalString`): a `oneof` takes neither `repeated` nor `optional`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use wit_parser::TypeDefKind;

use super::{Model, Scalar, Shape, camel, fail};

pub(super) fn generate(model: &Model, version: &str) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "// The protocol between Solium and a window manager, version {version}.\n\
         //\n\
         // Generated from the WIT in crates/wm-protocol/wit, which is the\n\
         // protocol's one definition: change that, never this file.\n\
         //\n\
         // A window manager is started with a Unix stream socket as its standard\n\
         // input. Each message on it, either way, is one `Message`, with its\n\
         // length before it as a varint: protobuf's delimited framing.\n\
         // docs/window-managers.md says who sends what, and when.\n\
         //\n\
         // What is here is only ever added to: a field, a case of a `oneof` or\n\
         // of an enum, at the end. A peer that does not know it passes over it.\n\n\
         syntax = \"proto3\";\n\n\
         package solium.wm;\n\n\
         option csharp_namespace = \"Solium.Wm\";\n\
         option go_package = \"solium/wm\";\n\
         option java_package = \"org.solium.wm\";\n\
         option java_multiple_files = true;\n\n\
         // Nothing: a case that carries no value.\n\
         message Unit {{}}\n\n"
    );
    // The messages that hold one list or one option, by their names.
    let mut holders = BTreeMap::new();
    for id in &model.named {
        let def = model.def(*id);
        let name = camel(model.name(*id));
        match &def.kind {
            TypeDefKind::Record(record) => {
                comment(def.docs.contents.as_ref(), "", &mut out);
                let _ = writeln!(out, "message {name} {{");
                for (index, field) in record.fields.iter().enumerate() {
                    comment(field.docs.contents.as_ref(), "  ", &mut out);
                    let shape = model.shape(&field.ty);
                    let ty = match &shape {
                        Shape::List(item) => format!("repeated {}", word(model, item)),
                        Shape::Option(item) => match &**item {
                            Shape::List(_) => {
                                format!("optional {}", holder(model, item, &mut holders))
                            }
                            other => format!("optional {}", word(model, other)),
                        },
                        other => word(model, other),
                    };
                    let _ = writeln!(out, "  {ty} {} = {};", field_name(&field.name), index + 1);
                }
                out.push_str("}\n\n");
            }
            TypeDefKind::Variant(variant) => {
                comment(def.docs.contents.as_ref(), "", &mut out);
                let _ = writeln!(out, "message {name} {{\n  oneof kind {{");
                for (index, case) in variant.cases.iter().enumerate() {
                    comment(case.docs.contents.as_ref(), "    ", &mut out);
                    let ty = match case.ty.as_ref().map(|payload| model.shape(payload)) {
                        None => "Unit".to_owned(),
                        Some(shape @ (Shape::List(_) | Shape::Option(_))) => {
                            holder(model, &shape, &mut holders)
                        }
                        Some(shape) => word(model, &shape),
                    };
                    let _ = writeln!(out, "    {ty} {} = {};", field_name(&case.name), index + 1);
                }
                out.push_str("  }\n}\n\n");
            }
            TypeDefKind::Enum(enumeration) => {
                comment(def.docs.contents.as_ref(), "", &mut out);
                let _ = writeln!(out, "enum {name} {{");
                let prefix = shout(model.name(*id));
                for (index, case) in enumeration.cases.iter().enumerate() {
                    comment(case.docs.contents.as_ref(), "  ", &mut out);
                    let _ = writeln!(out, "  {prefix}_{} = {index};", shout(&case.name));
                }
                out.push_str("}\n\n");
            }
            // A name for a list or for another type is written out where
            // it is used: protobuf has no aliases.
            _ => {}
        }
    }
    for (name, body) in &holders {
        let what = if body.starts_with("repeated") {
            "A list, where a list cannot stand as a field of its own."
        } else {
            "A value that may be absent, where `optional` cannot be written."
        };
        let _ = writeln!(out, "// {what}\nmessage {name} {{\n  {body} = 1;\n}}\n");
    }
    // One newline at the end, as a file has.
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// A WIT comment as a `.proto` one.
fn comment(contents: Option<&String>, indent: &str, out: &mut String) {
    if let Some(contents) = contents {
        for line in contents.lines() {
            let line = format!("{indent}// {line}");
            let _ = writeln!(out, "{}", line.trim_end());
        }
    }
}

/// `kebab-case` as `snake_case`, which a `.proto` field is.
fn field_name(name: &str) -> String {
    name.replace('-', "_")
}

/// `kebab-case` as `SHOUTING_CASE`, which an enum's value is.
fn shout(name: &str) -> String {
    name.replace('-', "_").to_uppercase()
}

/// The `.proto` word for a shape that is one value.
fn word(model: &Model, shape: &Shape) -> String {
    match shape {
        Shape::Scalar(scalar) => match scalar {
            Scalar::Bool => "bool",
            Scalar::U8 | Scalar::U16 | Scalar::U32 | Scalar::Char => "uint32",
            Scalar::U64 => "uint64",
            Scalar::S8 | Scalar::S16 | Scalar::S32 => "sint32",
            Scalar::S64 => "sint64",
            Scalar::F32 => "float",
            Scalar::F64 => "double",
            Scalar::String => "string",
        }
        .to_owned(),
        Shape::Named(id) => camel(model.name(*id)),
        Shape::Bytes => "bytes".to_owned(),
        Shape::List(_) | Shape::Option(_) => fail("a list or an option is not one value"),
    }
}

/// The name of the message that holds one list or one option, which is
/// noted in `holders` with its one field.
fn holder(model: &Model, shape: &Shape, holders: &mut BTreeMap<String, String>) -> String {
    // What the item is called in a message's name: its WIT word, so that a
    // list of `u64` and one of `u32` are two messages.
    let called = |item: &Shape| match item {
        Shape::Scalar(scalar) => match scalar {
            Scalar::Bool => "Bool",
            Scalar::U8 => "U8",
            Scalar::U16 => "U16",
            Scalar::U32 => "U32",
            Scalar::U64 => "U64",
            Scalar::S8 => "S8",
            Scalar::S16 => "S16",
            Scalar::S32 => "S32",
            Scalar::S64 => "S64",
            Scalar::F32 => "F32",
            Scalar::F64 => "F64",
            Scalar::Char => "Char",
            Scalar::String => "String",
        }
        .to_owned(),
        Shape::Named(id) => camel(model.name(*id)),
        Shape::Bytes => "Bytes".to_owned(),
        Shape::List(_) | Shape::Option(_) => fail("a list or an option of lists or options"),
    };
    let (name, body) = match shape {
        Shape::List(item) => (
            format!("{}List", called(item)),
            format!("repeated {} items", word(model, item)),
        ),
        Shape::Option(item) => (
            format!("Optional{}", called(item)),
            format!("optional {} value", word(model, item)),
        ),
        _ => fail("only a list or an option needs a message to hold it"),
    };
    holders.insert(name.clone(), body);
    name
}
