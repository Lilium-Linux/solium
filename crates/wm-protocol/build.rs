//! Generate, from `wit/`, the protocol's Rust types with their encoding and
//! the `.proto` file that says the same in protobuf's own language.
//!
//! The WIT is the protocol's one definition. This crate is built from it,
//! and a window manager in another language is built from the `.proto`
//! with protobuf's generator for that language, so the two cannot
//! disagree: what is on the wire is protobuf, and both files describe it.
//!
//! A field's number on the wire is its place in its record, counted from
//! 1; a variant's case is a field numbered by its place, from 1; an enum's
//! case is its place, from 0. So the WIT is only ever added to at the end
//! of a record, a variant or an enum, and what is there keeps its place.

#[path = "generate/proto.rs"]
mod proto;
#[path = "generate/rust.rs"]
mod rust;

use std::path::Path;

use wit_parser::{Resolve, Type, TypeDef, TypeDefKind, TypeId, TypeOwner};

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

    // Every type of the package that has a name of its own, in the order
    // the resolver sorted them, which puts each after the types it uses.
    let mut named = Vec::new();
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
        // same name: the original is generated once.
        if let TypeDefKind::Type(Type::Id(other)) = &def.kind
            && resolve.types[*other].name.as_deref() == Some(name.as_str())
        {
            continue;
        }
        named.push(id);
    }
    let model = Model {
        resolve: &resolve,
        named,
    };

    let out =
        Path::new(&std::env::var("OUT_DIR").unwrap_or_else(|_| fail("no OUT_DIR"))).to_path_buf();
    for (file, text) in [
        ("generated.rs", rust::generate(&model)),
        ("solium_wm.proto", proto::generate(&model, &version)),
    ] {
        let target = out.join(file);
        if let Err(err) = std::fs::write(&target, text) {
            fail(&format!("writing {}: {err}", target.display()));
        }
    }
}

fn fail(message: &str) -> ! {
    eprintln!("solium-wm-protocol: {message}");
    std::process::exit(1)
}

/// The WIT, as both generators read it.
struct Model<'a> {
    resolve: &'a Resolve,
    /// The types generated, in order.
    named: Vec<TypeId>,
}

/// A value with no parts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scalar {
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Char,
    String,
}

/// What a field or a case's payload is, with every alias followed.
enum Shape {
    Scalar(Scalar),
    /// A record, a variant or an enum, by the id of its definition.
    Named(TypeId),
    /// `list<u8>`.
    Bytes,
    /// A list of scalars or of named types.
    List(Box<Shape>),
    Option(Box<Shape>),
}

impl Model<'_> {
    fn def(&self, id: TypeId) -> &TypeDef {
        &self.resolve.types[id]
    }

    /// A named type's name, as the WIT spells it.
    fn name(&self, id: TypeId) -> &str {
        self.def(id).name.as_deref().unwrap_or("")
    }

    fn is_enum(&self, id: TypeId) -> bool {
        matches!(self.def(id).kind, TypeDefKind::Enum(_))
    }

    /// The shape of a type, or the build's end for one protobuf has no
    /// plain word for: a tuple, whose parts have no names, and a list or
    /// an option nested in another.
    fn shape(&self, ty: &Type) -> Shape {
        let scalar = match ty {
            Type::Bool => Scalar::Bool,
            Type::U8 => Scalar::U8,
            Type::U16 => Scalar::U16,
            Type::U32 => Scalar::U32,
            Type::U64 => Scalar::U64,
            Type::S8 => Scalar::S8,
            Type::S16 => Scalar::S16,
            Type::S32 => Scalar::S32,
            Type::S64 => Scalar::S64,
            Type::F32 => Scalar::F32,
            Type::F64 => Scalar::F64,
            Type::Char => Scalar::Char,
            Type::String => Scalar::String,
            Type::Id(id) => {
                return match &self.def(*id).kind {
                    TypeDefKind::Record(_) | TypeDefKind::Variant(_) | TypeDefKind::Enum(_) => {
                        Shape::Named(*id)
                    }
                    TypeDefKind::Type(inner) => self.shape(inner),
                    TypeDefKind::List(Type::U8) => Shape::Bytes,
                    TypeDefKind::List(item) => {
                        let item = self.shape(item);
                        if !matches!(item, Shape::Scalar(_) | Shape::Named(_)) {
                            fail(
                                "a list of lists, of options or of bytes has no name for \
                                 its items: make the item a record",
                            );
                        }
                        Shape::List(Box::new(item))
                    }
                    TypeDefKind::Option(item) => {
                        let item = self.shape(item);
                        if matches!(item, Shape::Option(_)) {
                            fail("an option of an option is one option");
                        }
                        Shape::Option(Box::new(item))
                    }
                    TypeDefKind::Tuple(_) => fail(
                        "a tuple's parts have no names, and a window manager in another \
                         language reads them by name: make it a record",
                    ),
                    other => fail(&format!("a {} is not in the protocol", other.as_str())),
                };
            }
            other => fail(&format!("{other:?} is not in the protocol")),
        };
        Shape::Scalar(scalar)
    }
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
