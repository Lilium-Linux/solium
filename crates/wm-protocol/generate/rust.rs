//! The Rust the WIT becomes: each named type under the same name in
//! `UpperCamelCase`, its fields and cases in `snake_case` and
//! `UpperCamelCase`, and a [`Wire`] implementation that writes it as the
//! protobuf message `proto.rs` describes.

use std::fmt::Write as _;

use wit_parser::{Type, TypeDefKind, TypeId};

use super::{Model, Scalar, Shape, camel, fail};

/// The variants whose every case is a word of the protocol a window manager
/// reads or writes, and so is documented.
const VOCABULARY: &[&str] = &["message", "change", "event", "command", "query", "reply"];

pub(super) fn generate(model: &Model) -> String {
    let mut out = String::new();
    out.push_str("use crate::wire::{self, DecodeError, Input, Kind, Wire};\n\n");
    // What the WIT says nothing about: the documentation is generated from
    // its comments, so a type or a word of the protocol's vocabulary with
    // none is one nobody is told the meaning of.
    let mut undocumented = Vec::new();
    for id in &model.named {
        let def = model.def(*id);
        let name = model.name(*id);
        if def.docs.contents.is_none() {
            undocumented.push(name.to_owned());
        }
        if let TypeDefKind::Variant(variant) = &def.kind
            && VOCABULARY.contains(&name)
        {
            for case in &variant.cases {
                if case.docs.contents.is_none() {
                    undocumented.push(format!("{name}.{}", case.name));
                }
            }
        }
        one(model, *id, &mut out);
    }
    out.push_str(
        "/// The types of the WIT, and the cases of its messages, changes, events,\n\
         /// commands, queries and replies, that have no documentation: none, which\n\
         /// `tests::every_word_of_the_protocol_says_what_it_means` holds it to.\n\
         pub const UNDOCUMENTED: &[&str] = &[\n",
    );
    for name in &undocumented {
        let _ = writeln!(out, "    {name:?},");
    }
    out.push_str("];\n");
    out
}

/// `kebab-case` as `snake_case`, a Rust keyword escaped.
fn snake(name: &str) -> String {
    let name = name.replace('-', "_");
    match name.as_str() {
        "type" | "move" | "match" | "ref" | "self" | "in" | "loop" | "box" | "use" | "mod"
        | "fn" | "impl" | "for" | "where" | "async" | "await" | "dyn" | "static" | "struct"
        | "enum" | "trait" | "crate" | "super" | "const" | "let" | "mut" | "pub" | "return"
        | "true" | "false" | "while" | "if" | "else" | "break" | "continue" | "unsafe"
        | "extern" | "abstract" | "final" | "override" | "virtual" | "yield" | "try" | "macro"
        | "priv" | "typeof" | "unsized" | "become" | "do" | "gen" => {
            format!("r#{name}")
        }
        _ => name,
    }
}

/// The Rust type a shape is written as.
fn rust(model: &Model, shape: &Shape) -> String {
    match shape {
        Shape::Scalar(scalar) => match scalar {
            Scalar::Bool => "bool",
            Scalar::U8 => "u8",
            Scalar::U16 => "u16",
            Scalar::U32 => "u32",
            Scalar::U64 => "u64",
            Scalar::S8 => "i8",
            Scalar::S16 => "i16",
            Scalar::S32 => "i32",
            Scalar::S64 => "i64",
            Scalar::F32 => "f32",
            Scalar::F64 => "f64",
            Scalar::Char => "char",
            Scalar::String => "String",
        }
        .to_owned(),
        Shape::Named(id) => camel(model.name(*id)),
        Shape::Bytes => "Vec<u8>".to_owned(),
        Shape::List(item) => format!("Vec<{}>", rust(model, item)),
        Shape::Option(item) => format!("Option<{}>", rust(model, item)),
    }
}

/// The documentation a WIT item carries, as Rust doc comments.
fn docs(contents: Option<&String>, indent: &str, out: &mut String) {
    if let Some(contents) = contents {
        for line in contents.lines() {
            let _ = writeln!(out, "{indent}/// {line}");
        }
    }
}

/// A scalar's number as a varint carries it, from an expression of its
/// own type: unused for the two floats and for text.
fn as_varint(scalar: Scalar, value: &str) -> String {
    match scalar {
        Scalar::Bool | Scalar::U8 | Scalar::U16 | Scalar::U32 => format!("u64::from({value})"),
        Scalar::U64 => value.to_owned(),
        Scalar::S8 | Scalar::S16 | Scalar::S32 => format!("wire::zigzag(i64::from({value}))"),
        Scalar::S64 => format!("wire::zigzag({value})"),
        Scalar::Char => format!("u64::from(u32::from({value}))"),
        Scalar::F32 | Scalar::F64 | Scalar::String => String::new(),
    }
}

/// The statement that writes a value as field `number`, whatever the value
/// is. `place` names it: a reference to it when `by_ref`, the value's own
/// place otherwise.
fn put(model: &Model, shape: &Shape, number: usize, place: &str, by_ref: bool) -> String {
    let value = if by_ref {
        format!("*{place}")
    } else {
        place.to_owned()
    };
    let reference = if by_ref {
        place.to_owned()
    } else {
        format!("&{place}")
    };
    match shape {
        Shape::Scalar(Scalar::F32) => format!("wire::f32_field(out, {number}, {value});"),
        Shape::Scalar(Scalar::F64) => format!("wire::f64_field(out, {number}, {value});"),
        Shape::Scalar(Scalar::String) => {
            format!("wire::bytes_field(out, {number}, {place}.as_bytes());")
        }
        Shape::Scalar(scalar) => format!(
            "wire::varint_field(out, {number}, {});",
            as_varint(*scalar, &value)
        ),
        Shape::Named(id) if model.is_enum(*id) => {
            format!("wire::varint_field(out, {number}, {value} as u64);")
        }
        Shape::Named(_) => format!("wire::message_field(out, {number}, {reference});"),
        Shape::Bytes => format!("wire::bytes_field(out, {number}, {reference});"),
        Shape::List(item) => match &**item {
            Shape::Scalar(Scalar::String) => format!(
                "for item in {reference} {{ wire::bytes_field(out, {number}, item.as_bytes()); }}"
            ),
            Shape::Named(id) if !model.is_enum(*id) => {
                format!("for item in {reference} {{ wire::message_field(out, {number}, item); }}")
            }
            // Numbers are packed: one field, the items back to back.
            packed => {
                let raw = match packed {
                    Shape::Scalar(Scalar::F32 | Scalar::F64) => {
                        "out.extend_from_slice(&item.to_le_bytes());".to_owned()
                    }
                    Shape::Scalar(scalar) => {
                        format!("wire::varint(out, {});", as_varint(*scalar, "*item"))
                    }
                    _ => "wire::varint(out, *item as u64);".to_owned(),
                };
                format!(
                    "if !{place}.is_empty() {{ wire::nested(out, {number}, |out| {{ \
                     for item in {reference} {{ {raw} }} }}); }}"
                )
            }
        },
        Shape::Option(item) => {
            let inner = match &**item {
                // A list that may be absent is a message of its own, so
                // that none and empty are told apart.
                Shape::List(_) => format!(
                    "wire::nested(out, {number}, |out| {{ {} }});",
                    put(model, item, 1, "item", true)
                ),
                other => put(model, other, number, "item", true),
            };
            format!("if let Some(item) = {reference} {{ {inner} }}")
        }
    }
}

/// The statement that writes a record's field: as [`put`] does, but a
/// value that is its type's default is left out, as proto3 leaves it.
fn put_field(model: &Model, shape: &Shape, number: usize, place: &str) -> String {
    let written = put(model, shape, number, place, false);
    let unless = match shape {
        Shape::Scalar(Scalar::Bool) => place.to_owned(),
        Shape::Scalar(Scalar::F32 | Scalar::F64) => format!("{place}.to_bits() != 0"),
        Shape::Scalar(Scalar::Char) => format!("{place} != '\\0'"),
        Shape::Scalar(Scalar::String) | Shape::Bytes => format!("!{place}.is_empty()"),
        Shape::Scalar(_) => format!("{place} != 0"),
        Shape::Named(id) if model.is_enum(*id) => format!("{place} as u64 != 0"),
        Shape::Named(_) | Shape::List(_) | Shape::Option(_) => return written,
    };
    format!("if {unless} {{ {written} }}")
}

/// How one value of a shape that is not a list or an option is read: the
/// kind its field has, and the expression that reads it.
fn read(model: &Model, shape: &Shape) -> (&'static str, String) {
    match shape {
        Shape::Scalar(scalar) => match scalar {
            Scalar::Bool => ("Varint", "input.varint()? != 0".to_owned()),
            Scalar::U8 => ("Varint", "input.unsigned(8)? as u8".to_owned()),
            Scalar::U16 => ("Varint", "input.unsigned(16)? as u16".to_owned()),
            Scalar::U32 => ("Varint", "input.unsigned(32)? as u32".to_owned()),
            Scalar::U64 => ("Varint", "input.unsigned(64)?".to_owned()),
            Scalar::S8 => ("Varint", "input.signed(8)? as i8".to_owned()),
            Scalar::S16 => ("Varint", "input.signed(16)? as i16".to_owned()),
            Scalar::S32 => ("Varint", "input.signed(32)? as i32".to_owned()),
            Scalar::S64 => ("Varint", "input.signed(64)?".to_owned()),
            Scalar::F32 => ("I32", "input.f32()?".to_owned()),
            Scalar::F64 => ("I64", "input.f64()?".to_owned()),
            Scalar::Char => ("Varint", "input.char()?".to_owned()),
            Scalar::String => ("Len", "input.string()?".to_owned()),
        },
        Shape::Named(id) if model.is_enum(*id) => (
            "Varint",
            format!(
                "{}::from_number(input.varint()?).ok_or(DecodeError::UnknownCase({:?}))?",
                camel(model.name(*id)),
                model.name(*id)
            ),
        ),
        Shape::Named(_) => ("Len", "input.message()?".to_owned()),
        Shape::Bytes => ("Len", "input.bytes()?".to_owned()),
        Shape::List(_) | Shape::Option(_) => fail("a list or an option is read by its items"),
    }
}

/// The match arms that read a list's items from field `number` into the
/// vector `into`.
fn take_list(model: &Model, item: &Shape, number: usize, into: &str) -> String {
    match item {
        Shape::Scalar(Scalar::String) => {
            format!("({number}, Kind::Len) => {into}.push(input.string()?),\n")
        }
        // An item in a case this version does not have is left out.
        Shape::Named(id) if !model.is_enum(*id) => format!(
            "({number}, Kind::Len) => {{ if let Some(item) = input.item()? {{ {into}.push(item); }} }}\n"
        ),
        // Packed, or one at a time: a reader takes both.
        packed => {
            let (kind, one) = read(model, packed);
            format!(
                "({number}, Kind::Len) => input.packed(|input| {{ {into}.push({one}); Ok(()) }})?,\n\
                 ({number}, Kind::{kind}) => {into}.push({one}),\n"
            )
        }
    }
}

/// The expression that reads a message holding nothing but a list in its
/// field 1.
fn read_list_message(model: &Model, item: &Shape) -> String {
    format!(
        "input.nested(|input| {{ let mut items = Vec::new(); \
         while let Some(tag) = input.tag()? {{ match tag {{ {} (_, kind) => input.skip(kind)?, }} }} \
         Ok(items) }})?",
        take_list(model, item, 1, "items")
    )
}

/// The match arms that read field `number` of `shape`, each value read
/// handed to `sink` as an expression, which makes a statement of it.
fn take(model: &Model, shape: &Shape, number: usize, sink: &dyn Fn(&str) -> String) -> String {
    let (kind, value) = read(model, shape);
    format!("({number}, Kind::{kind}) => {{ {} }}\n", sink(&value))
}

fn record(model: &Model, id: TypeId, out: &mut String) {
    let TypeDefKind::Record(record) = &model.def(id).kind else {
        return;
    };
    let ty = camel(model.name(id));
    let _ = writeln!(
        out,
        "#[derive(Clone, Debug, Default, PartialEq)]\npub struct {ty} {{"
    );
    for field in &record.fields {
        docs(field.docs.contents.as_ref(), "    ", out);
        let _ = writeln!(
            out,
            "    pub {}: {},",
            snake(&field.name),
            rust(model, &model.shape(&field.ty))
        );
    }
    out.push_str("}\n\n");
    let _ = writeln!(
        out,
        "impl Wire for {ty} {{\n    fn encode(&self, out: &mut Vec<u8>) {{"
    );
    for (index, field) in record.fields.iter().enumerate() {
        let place = format!("self.{}", snake(&field.name));
        let _ = writeln!(
            out,
            "        {}",
            put_field(model, &model.shape(&field.ty), index + 1, &place)
        );
    }
    out.push_str(
        "    }\n    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {\n        \
         let mut value = Self::default();\n        while let Some(tag) = input.tag()? {\n            \
         match tag {\n",
    );
    for (index, field) in record.fields.iter().enumerate() {
        let number = index + 1;
        let place = format!("value.{}", snake(&field.name));
        let arms = match model.shape(&field.ty) {
            Shape::List(item) => take_list(model, &item, number, &place),
            Shape::Option(item) => match &*item {
                Shape::List(inner) => format!(
                    "({number}, Kind::Len) => {{ {place} = Some({}); }}\n",
                    read_list_message(model, inner)
                ),
                other => take(model, other, number, &|value| {
                    format!("{place} = Some({value});")
                }),
            },
            shape => take(model, &shape, number, &|value| {
                format!("{place} = {value};")
            }),
        };
        for arm in arms.lines() {
            let _ = writeln!(out, "                {arm}");
        }
    }
    out.push_str(
        "                (_, kind) => input.skip(kind)?,\n            }\n        }\n        \
         Ok(value)\n    }\n}\n\n",
    );
}

fn variant(model: &Model, id: TypeId, out: &mut String) {
    let TypeDefKind::Variant(variant) = &model.def(id).kind else {
        return;
    };
    let name = model.name(id);
    let ty = camel(name);
    let _ = writeln!(out, "#[derive(Clone, Debug, PartialEq)]\npub enum {ty} {{");
    for case in &variant.cases {
        docs(case.docs.contents.as_ref(), "    ", out);
        match &case.ty {
            Some(payload) => {
                let _ = writeln!(
                    out,
                    "    {}({}),",
                    camel(&case.name),
                    rust(model, &model.shape(payload))
                );
            }
            None => {
                let _ = writeln!(out, "    {},", camel(&case.name));
            }
        }
    }
    out.push_str("}\n\n");
    // What a field that was not sent reads as: the first case.
    if let Some(first) = variant.cases.first() {
        let payload = if first.ty.is_some() {
            "(Default::default())"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "impl Default for {ty} {{\n    fn default() -> Self {{\n        Self::{}{payload}\n    }}\n}}\n",
            camel(&first.name)
        );
    }
    // Each case's name as the WIT spells it, and all of them: an event is
    // listened to by its name.
    let _ = writeln!(
        out,
        "impl {ty} {{\n    /// Every case's name, as the WIT spells it, in its order.\n    \
         pub const CASES: &'static [&'static str] = &["
    );
    for case in &variant.cases {
        let _ = writeln!(out, "        {:?},", case.name);
    }
    let _ = writeln!(
        out,
        "    ];\n\n    /// This case's name, as the WIT spells it.\n    \
         pub const fn case(&self) -> &'static str {{\n        match self {{"
    );
    for case in &variant.cases {
        let pattern = if case.ty.is_some() { "(_)" } else { "" };
        let _ = writeln!(
            out,
            "            Self::{}{pattern} => {:?},",
            camel(&case.name),
            case.name
        );
    }
    out.push_str("        }\n    }\n}\n\n");
    let _ = writeln!(
        out,
        "impl Wire for {ty} {{\n    fn encode(&self, out: &mut Vec<u8>) {{\n        match self {{"
    );
    for (index, case) in variant.cases.iter().enumerate() {
        let number = index + 1;
        let case_name = camel(&case.name);
        let Some(payload) = &case.ty else {
            let _ = writeln!(
                out,
                "            Self::{case_name} => wire::nested(out, {number}, |_| {{}}),"
            );
            continue;
        };
        let shape = model.shape(payload);
        let written = match &shape {
            // A list or an option cannot be one of several: it goes in a
            // message of its own, as its field 1.
            Shape::List(_) | Shape::Option(_) => format!(
                "wire::nested(out, {number}, |out| {{ {} }});",
                put(model, &shape, 1, "payload", true)
            ),
            other => put(model, other, number, "payload", true),
        };
        let _ = writeln!(
            out,
            "            Self::{case_name}(payload) => {{ {written} }}"
        );
    }
    out.push_str(
        "        }\n    }\n    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {\n        \
         let mut read = None;\n        let mut unknown = false;\n        \
         while let Some(tag) = input.tag()? {\n            match tag {\n",
    );
    for (index, case) in variant.cases.iter().enumerate() {
        let number = index + 1;
        let case_name = camel(&case.name);
        let arms = match case.ty.as_ref().map(|payload| model.shape(payload)) {
            None => format!(
                "({number}, Kind::Len) => {{ input.unit()?; read = Some(Self::{case_name}); }}\n"
            ),
            Some(Shape::List(item)) => format!(
                "({number}, Kind::Len) => {{ read = Some(Self::{case_name}({})); }}\n",
                read_list_message(model, &item)
            ),
            Some(Shape::Option(item)) => {
                if matches!(*item, Shape::List(_)) {
                    fail(&format!(
                        "`{name}.{}` is an option of a list: make it a record",
                        case.name
                    ));
                }
                format!(
                    "({number}, Kind::Len) => {{ read = Some(Self::{case_name}(input.nested(|input| {{ \
                     let mut item = None; while let Some(tag) = input.tag()? {{ match tag {{ {} \
                     (_, kind) => input.skip(kind)?, }} }} Ok(item) }})?)); }}\n",
                    take(model, &item, 1, &|value| format!("item = Some({value});")).trim_end()
                )
            }
            Some(shape) => take(model, &shape, number, &|value| {
                format!("read = Some(Self::{case_name}({value}));")
            }),
        };
        for arm in arms.lines() {
            let _ = writeln!(out, "                {arm}");
        }
    }
    // A case this version does not have is not the first case: whatever
    // holds this value is told it could not be read. A message with no
    // case at all is one nobody set, which is the default.
    let _ = writeln!(
        out,
        "                (_, kind) => {{ input.skip(kind)?; unknown = true; }}\n            }}\n        }}\n        \
         match read {{\n            Some(read) => Ok(read),\n            \
         None if unknown => Err(DecodeError::UnknownCase({name:?})),\n            \
         None => Ok(Self::default()),\n        }}\n    }}\n}}\n"
    );
}

fn enumeration(model: &Model, id: TypeId, out: &mut String) {
    let TypeDefKind::Enum(enumeration) = &model.def(id).kind else {
        return;
    };
    let ty = camel(model.name(id));
    let _ = writeln!(
        out,
        "#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]\npub enum {ty} {{"
    );
    for (index, case) in enumeration.cases.iter().enumerate() {
        docs(case.docs.contents.as_ref(), "    ", out);
        if index == 0 {
            out.push_str("    #[default]\n");
        }
        let _ = writeln!(out, "    {},", camel(&case.name));
    }
    out.push_str("}\n\n");
    // Each case's name as the WIT spells it, for a peer that speaks in
    // words: a script, a log.
    let _ = writeln!(
        out,
        "impl {ty} {{\n    /// This case's name, as the WIT spells it.\n    \
         pub const fn name(self) -> &'static str {{\n        match self {{"
    );
    for case in &enumeration.cases {
        let _ = writeln!(
            out,
            "            Self::{} => {:?},",
            camel(&case.name),
            case.name
        );
    }
    let _ = writeln!(
        out,
        "        }}\n    }}\n\n    /// The case of this number on the wire, if this version has it.\n    \
         pub const fn from_number(number: u64) -> Option<Self> {{\n        match number {{"
    );
    for (index, case) in enumeration.cases.iter().enumerate() {
        let _ = writeln!(
            out,
            "            {index} => Some(Self::{}),",
            camel(&case.name)
        );
    }
    out.push_str("            _ => None,\n        }\n    }\n}\n\n");
}

fn one(model: &Model, id: TypeId, out: &mut String) {
    let def = model.def(id);
    let name = model.name(id);
    docs(def.docs.contents.as_ref(), "", out);
    match &def.kind {
        TypeDefKind::Record(_) => record(model, id, out),
        TypeDefKind::Variant(_) => variant(model, id, out),
        TypeDefKind::Enum(_) => enumeration(model, id, out),
        // A name for a list or for another type: the same name in Rust.
        TypeDefKind::Type(_) | TypeDefKind::List(_) | TypeDefKind::Option(_) => {
            let _ = writeln!(
                out,
                "pub type {} = {};\n",
                camel(name),
                rust(model, &model.shape(&Type::Id(id)))
            );
        }
        other => fail(&format!(
            "`{name}` is a {}, which the protocol does not use",
            other.as_str()
        )),
    }
}
