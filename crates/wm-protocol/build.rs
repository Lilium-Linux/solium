//! Generate the protocol's Rust types and their encoding from `wit/`.
//!
//! The WIT is the protocol's one definition: a window manager in another
//! language reads it, and this crate is built from it, so the two cannot
//! disagree. Every named type of every interface becomes a Rust type of the
//! same name in `UpperCamelCase`, its fields and cases in `snake_case` and
//! `UpperCamelCase`, with a [`Wire`] implementation in the encoding
//! `src/lib.rs` documents.

use std::fmt::Write as _;
use std::path::Path;

use wit_parser::{Resolve, Type, TypeDefKind, TypeId, TypeOwner};

fn main() {
    let wit = Path::new("wit");
    println!("cargo:rerun-if-changed=wit");
    let mut resolve = Resolve::default();
    let (package, _) = match resolve.push_dir(wit) {
        Ok(read) => read,
        Err(err) => fail(&format!("reading wit/: {err:?}")),
    };
    let version = resolve.packages[package]
        .name
        .version
        .as_ref()
        .map_or_else(|| "0.0.0".to_owned(), ToString::to_string);
    println!("cargo:rustc-env=SOLIUM_WM_PROTOCOL_VERSION={version}");

    let mut out = String::new();
    out.push_str("use crate::wire::{DecodeError, Input, Wire, write_leb};\n\n");
    // Every type in the package, in the order the resolver sorted them,
    // which puts each after the types it uses.
    for (id, def) in &resolve.types {
        let TypeOwner::Interface(owner) = def.owner else {
            continue;
        };
        if resolve.interfaces[owner].package != Some(package) {
            continue;
        }
        let Some(name) = &def.name else {
            continue;
        };
        // A `use` brings a type into another interface as an alias of the
        // same id: the original is generated once, under its own name.
        if let TypeDefKind::Type(Type::Id(other)) = &def.kind
            && resolve.types[*other].name.as_deref() == Some(name.as_str())
        {
            continue;
        }
        generate(&resolve, id, name, &mut out);
    }

    let target = Path::new(&std::env::var("OUT_DIR").unwrap_or_else(|_| fail("no OUT_DIR")))
        .join("generated.rs");
    if let Err(err) = std::fs::write(&target, out) {
        fail(&format!("writing {}: {err}", target.display()));
    }
}

fn fail(message: &str) -> ! {
    eprintln!("solium-wm-protocol: {message}");
    std::process::exit(1)
}

/// `kebab-case` as `UpperCamelCase`.
fn camel(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect()
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

/// The Rust type a WIT type is written as.
fn rust(resolve: &Resolve, ty: &Type) -> String {
    match ty {
        Type::Bool => "bool".into(),
        Type::U8 => "u8".into(),
        Type::U16 => "u16".into(),
        Type::U32 => "u32".into(),
        Type::U64 => "u64".into(),
        Type::S8 => "i8".into(),
        Type::S16 => "i16".into(),
        Type::S32 => "i32".into(),
        Type::S64 => "i64".into(),
        Type::F32 => "f32".into(),
        Type::F64 => "f64".into(),
        Type::Char => "char".into(),
        Type::String => "String".into(),
        Type::Id(id) => {
            let def = &resolve.types[*id];
            if let Some(name) = &def.name {
                // An alias brought in by `use` is the original's name.
                let mut target = *id;
                while let TypeDefKind::Type(Type::Id(next)) = &resolve.types[target].kind {
                    if resolve.types[*next].name.as_deref() != Some(name.as_str()) {
                        break;
                    }
                    target = *next;
                }
                return camel(resolve.types[target].name.as_deref().unwrap_or(name));
            }
            match &def.kind {
                TypeDefKind::List(item) => format!("Vec<{}>", rust(resolve, item)),
                TypeDefKind::Option(item) => format!("Option<{}>", rust(resolve, item)),
                TypeDefKind::Tuple(tuple) => {
                    let items: Vec<String> =
                        tuple.types.iter().map(|item| rust(resolve, item)).collect();
                    format!("({},)", items.join(", "))
                }
                TypeDefKind::Type(inner) => rust(resolve, inner),
                other => fail(&format!(
                    "an anonymous {} is not in the protocol",
                    other.as_str()
                )),
            }
        }
        other => fail(&format!("{other:?} is not in the protocol")),
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

fn generate(resolve: &Resolve, id: TypeId, name: &str, out: &mut String) {
    let def = &resolve.types[id];
    let ty = camel(name);
    docs(def.docs.contents.as_ref(), "", out);
    match &def.kind {
        TypeDefKind::Record(record) => {
            let _ = writeln!(
                out,
                "#[derive(Clone, Debug, PartialEq)]\npub struct {ty} {{"
            );
            for field in &record.fields {
                docs(field.docs.contents.as_ref(), "    ", out);
                let _ = writeln!(
                    out,
                    "    pub {}: {},",
                    snake(&field.name),
                    rust(resolve, &field.ty)
                );
            }
            out.push_str("}\n\n");
            let _ = writeln!(
                out,
                "impl Wire for {ty} {{\n    fn encode(&self, out: &mut Vec<u8>) {{"
            );
            for field in &record.fields {
                let _ = writeln!(out, "        self.{}.encode(out);", snake(&field.name));
            }
            out.push_str("    }\n    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {\n        Ok(Self {\n");
            for field in &record.fields {
                let _ = writeln!(
                    out,
                    "            {}: Wire::decode(input)?,",
                    snake(&field.name)
                );
            }
            out.push_str("        })\n    }\n}\n\n");
        }
        TypeDefKind::Variant(variant) => {
            let _ = writeln!(out, "#[derive(Clone, Debug, PartialEq)]\npub enum {ty} {{");
            for case in &variant.cases {
                docs(case.docs.contents.as_ref(), "    ", out);
                match &case.ty {
                    Some(payload) => {
                        let _ = writeln!(
                            out,
                            "    {}({}),",
                            camel(&case.name),
                            rust(resolve, payload)
                        );
                    }
                    None => {
                        let _ = writeln!(out, "    {},", camel(&case.name));
                    }
                }
            }
            out.push_str("}\n\n");
            let _ = writeln!(
                out,
                "impl Wire for {ty} {{\n    fn encode(&self, out: &mut Vec<u8>) {{\n        match self {{"
            );
            for (index, case) in variant.cases.iter().enumerate() {
                let case_name = camel(&case.name);
                if case.ty.is_some() {
                    let _ = writeln!(
                        out,
                        "            Self::{case_name}(payload) => {{ write_leb(out, {index}); payload.encode(out); }}"
                    );
                } else {
                    let _ = writeln!(
                        out,
                        "            Self::{case_name} => write_leb(out, {index}),"
                    );
                }
            }
            out.push_str("        }\n    }\n    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {\n        match input.leb(64)? {\n");
            for (index, case) in variant.cases.iter().enumerate() {
                let case_name = camel(&case.name);
                if case.ty.is_some() {
                    let _ = writeln!(
                        out,
                        "            {index} => Ok(Self::{case_name}(Wire::decode(input)?)),"
                    );
                } else {
                    let _ = writeln!(out, "            {index} => Ok(Self::{case_name}),");
                }
            }
            let _ = writeln!(
                out,
                "            case => Err(DecodeError::BadCase({name:?}, case)),\n        }}\n    }}\n}}\n"
            );
        }
        TypeDefKind::Enum(enumeration) => {
            let _ = writeln!(
                out,
                "#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\npub enum {ty} {{"
            );
            for case in &enumeration.cases {
                docs(case.docs.contents.as_ref(), "    ", out);
                let _ = writeln!(out, "    {},", camel(&case.name));
            }
            out.push_str("}\n\n");
            let _ = writeln!(
                out,
                "impl Wire for {ty} {{\n    fn encode(&self, out: &mut Vec<u8>) {{\n        write_leb(out, *self as u64);\n    }}\n    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {{\n        match input.leb(64)? {{"
            );
            for (index, case) in enumeration.cases.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "            {index} => Ok(Self::{}),",
                    camel(&case.name)
                );
            }
            let _ = writeln!(
                out,
                "            case => Err(DecodeError::BadCase({name:?}, case)),\n        }}\n    }}\n}}\n"
            );
        }
        TypeDefKind::Type(inner) => {
            let _ = writeln!(out, "pub type {ty} = {};\n", rust(resolve, inner));
        }
        TypeDefKind::List(item) => {
            let _ = writeln!(out, "pub type {ty} = Vec<{}>;\n", rust(resolve, item));
        }
        TypeDefKind::Option(item) => {
            let _ = writeln!(out, "pub type {ty} = Option<{}>;\n", rust(resolve, item));
        }
        TypeDefKind::Tuple(_) => {
            let _ = writeln!(out, "pub type {ty} = {};\n", rust(resolve, &Type::Id(id)));
        }
        other => fail(&format!(
            "`{name}` is a {}, which the protocol does not use",
            other.as_str()
        )),
    }
}
