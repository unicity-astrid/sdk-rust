//! WIT record → Rust struct code generator for the `wit_events!` proc macro.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::BTreeMap;
use wit_parser::{Resolve, Type, TypeDefKind, TypeOwner};

/// Parse a WIT file or directory and generate Rust type definitions.
///
/// Supports both single `.wit` files and directories containing multiple `.wit`
/// files. Also emits hidden `include_str!` constants so Cargo rebuilds on changes.
pub(crate) fn generate(
    wit_path: &std::path::Path,
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    let mut resolve = Resolve::default();
    let mut output = TokenStream::new();

    if wit_path.is_dir() {
        let wit_files = collect_wit_files(wit_path, span)?;
        if wit_files.is_empty() {
            return Ok(output);
        }
        load_multi_package_dir(&mut resolve, wit_path, &wit_files, span)?;
        emit_include_strs(&wit_files, &mut output);
    } else {
        load_single_file(&mut resolve, wit_path, span)?;
        emit_include_str(wit_path, &mut output);
    }

    emit_type_definitions(&resolve, &mut output);
    Ok(output)
}

/// Load a single `.wit` file into the resolver.
fn load_single_file(
    resolve: &mut Resolve,
    wit_path: &std::path::Path,
    span: proc_macro2::Span,
) -> syn::Result<()> {
    let contents = std::fs::read_to_string(wit_path).map_err(|e| {
        syn::Error::new(
            span,
            format!("failed to read WIT file '{}': {e}", wit_path.display()),
        )
    })?;
    resolve
        .push_str(wit_path.display().to_string(), &contents)
        .map_err(|e| syn::Error::new(span, format!("failed to parse WIT: {e}")))?;
    Ok(())
}

/// Emit `include_str!` for a single file so Cargo tracks it.
fn emit_include_str(path: &std::path::Path, output: &mut TokenStream) {
    if let Some(p) = path.to_str() {
        output.extend(quote! { const _: &str = include_str!(#p); });
    }
}

/// Emit `include_str!` for each `.wit` file so Cargo tracks them.
fn emit_include_strs(wit_files: &[std::path::PathBuf], output: &mut TokenStream) {
    for path in wit_files {
        emit_include_str(path, output);
    }
}

/// Generate Rust struct/enum definitions, one `pub mod <interface> { … }`
/// per WIT interface. Per-interface namespacing prevents collisions
/// across the canonical interface set — e.g. both `agent.wit` and
/// `approval.wit` define `record response`, and both must produce a
/// reachable `Response` struct.
fn emit_type_definitions(resolve: &Resolve, output: &mut TokenStream) {
    // Group types by their owning interface. `BTreeMap` so the macro
    // output is deterministic across runs (HashMap iteration order
    // would change every rebuild and turn the generated docs into a
    // diff churn vector).
    let mut by_interface: BTreeMap<String, Vec<TokenStream>> = BTreeMap::new();
    let mut orphans: Vec<TokenStream> = Vec::new();

    for (_, type_def) in &resolve.types {
        let Some(ref name) = type_def.name else {
            continue;
        };
        let Some(tokens) = type_tokens(resolve, name, type_def) else {
            continue;
        };

        match type_def.owner {
            TypeOwner::Interface(iface_id) => {
                let iface_name = resolve.interfaces[iface_id]
                    .name
                    .clone()
                    .unwrap_or_else(|| "anon".to_string());
                by_interface.entry(iface_name).or_default().push(tokens);
            }
            // Top-level (package-level) types fall through here. Rare —
            // emitted at the parent module level to mirror the WIT shape.
            _ => orphans.push(tokens),
        }
    }

    for tokens in orphans {
        output.extend(tokens);
    }
    for (iface_name, items) in by_interface {
        let mod_ident = format_ident!("{}", kebab_to_snake(&iface_name));
        let doc = format!("Types generated from the `{iface_name}` WIT interface.");
        output.extend(quote! {
            #[doc = #doc]
            pub mod #mod_ident {
                #(#items)*
            }
        });
    }
}

/// Build the token stream for a single named type definition, or
/// `None` for kinds we don't emit (`type` aliases, resources, etc.).
fn type_tokens(
    resolve: &Resolve,
    name: &str,
    type_def: &wit_parser::TypeDef,
) -> Option<TokenStream> {
    let rust_name = format_ident!("{}", kebab_to_pascal(name));
    let doc_attr = case_doc_attr(&type_def.docs);

    match &type_def.kind {
        TypeDefKind::Record(record) => {
            let fields = record_fields(resolve, record);
            Some(quote! {
                #doc_attr
                #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
                #[serde(rename_all = "snake_case")]
                pub struct #rust_name {
                    #(#fields)*
                }
            })
        }
        TypeDefKind::Enum(enum_def) => {
            let variants = enum_variants(enum_def);
            Some(quote! {
                #doc_attr
                #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
                #[serde(rename_all = "snake_case")]
                pub enum #rust_name { #(#variants)* }
            })
        }
        TypeDefKind::Flags(flags_def) => {
            let variants = flags_variants(flags_def);
            let flag_enum_name = format_ident!("{}Flag", kebab_to_pascal(name));
            Some(quote! {
                #doc_attr
                #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
                #[serde(rename_all = "snake_case")]
                pub enum #flag_enum_name { #(#variants)* }
                /// Set of flag values (serializes as a JSON array).
                pub type #rust_name = Vec<#flag_enum_name>;
            })
        }
        TypeDefKind::Variant(variant) => {
            let cases = variant_cases(resolve, variant);
            Some(quote! {
                #doc_attr
                #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
                #[serde(tag = "tag", content = "value", rename_all = "snake_case")]
                pub enum #rust_name { #(#cases)* }
            })
        }
        _ => None,
    }
}

/// Generate enum variant tokens from a WIT enum definition.
fn enum_variants(enum_def: &wit_parser::Enum) -> Vec<TokenStream> {
    enum_def
        .cases
        .iter()
        .map(|case| {
            let variant_name = format_ident!("{}", kebab_to_pascal(&case.name));
            let doc = case_doc_attr(&case.docs);
            quote! { #doc #variant_name, }
        })
        .collect()
}

/// Generate enum variant tokens from a WIT flags definition.
fn flags_variants(flags_def: &wit_parser::Flags) -> Vec<TokenStream> {
    flags_def
        .flags
        .iter()
        .map(|flag| {
            let variant_name = format_ident!("{}", kebab_to_pascal(&flag.name));
            let doc = case_doc_attr(&flag.docs);
            quote! { #doc #variant_name, }
        })
        .collect()
}

/// Generate variant case tokens from a WIT variant definition.
fn variant_cases(resolve: &Resolve, variant: &wit_parser::Variant) -> Vec<TokenStream> {
    variant
        .cases
        .iter()
        .map(|case| {
            let variant_name = format_ident!("{}", kebab_to_pascal(&case.name));
            let doc = case_doc_attr(&case.docs);
            if let Some(ref ty) = case.ty {
                let (rust_ty, _) = wit_type_to_rust(resolve, ty);
                quote! { #doc #variant_name(#rust_ty), }
            } else {
                quote! { #doc #variant_name, }
            }
        })
        .collect()
}

/// Extract doc comment from a WIT Docs object as a `#[doc = "..."]` attribute.
fn case_doc_attr(docs: &wit_parser::Docs) -> Option<TokenStream> {
    docs.contents
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| quote! { #[doc = #d] })
}

/// Generate field definitions for a WIT record.
fn record_fields(resolve: &Resolve, record: &wit_parser::Record) -> Vec<TokenStream> {
    record
        .fields
        .iter()
        .map(|field| {
            let field_name = format_ident!("{}", kebab_to_snake(&field.name));
            let (ty, is_optional) = wit_type_to_rust(resolve, &field.ty);
            let doc_attr = case_doc_attr(&field.docs);

            if is_optional {
                quote! {
                    #doc_attr
                    #[serde(default, skip_serializing_if = "Option::is_none")]
                    pub #field_name: Option<#ty>,
                }
            } else {
                quote! {
                    #doc_attr
                    pub #field_name: #ty,
                }
            }
        })
        .collect()
}

/// Map a WIT type to a Rust type token stream.
///
/// Returns `(type_tokens, is_optional)` where `is_optional` means the
/// field should be wrapped in `Option<T>`.
fn wit_type_to_rust(resolve: &Resolve, ty: &Type) -> (TokenStream, bool) {
    match ty {
        Type::Bool => (quote! { bool }, false),
        Type::U8 => (quote! { u8 }, false),
        Type::U16 => (quote! { u16 }, false),
        Type::U32 => (quote! { u32 }, false),
        Type::U64 => (quote! { u64 }, false),
        Type::S8 => (quote! { i8 }, false),
        Type::S16 => (quote! { i16 }, false),
        Type::S32 => (quote! { i32 }, false),
        Type::S64 => (quote! { i64 }, false),
        Type::F32 => (quote! { f32 }, false),
        Type::F64 => (quote! { f64 }, false),
        Type::Char => (quote! { char }, false),
        Type::String | Type::ErrorContext => (quote! { String }, false),
        Type::Id(id) => wit_typedef_to_rust(resolve, &resolve.types[*id]),
    }
}

/// Map a named WIT type definition to a Rust type.
fn wit_typedef_to_rust(resolve: &Resolve, type_def: &wit_parser::TypeDef) -> (TokenStream, bool) {
    match &type_def.kind {
        TypeDefKind::List(inner) => {
            let (inner_ty, _) = wit_type_to_rust(resolve, inner);
            (quote! { Vec<#inner_ty> }, false)
        }
        TypeDefKind::Option(inner) => {
            let (inner_ty, inner_optional) = wit_type_to_rust(resolve, inner);
            if inner_optional {
                (quote! { Option<#inner_ty> }, true)
            } else {
                (quote! { #inner_ty }, true)
            }
        }
        TypeDefKind::Tuple(tuple) => {
            let types: Vec<TokenStream> = tuple
                .types
                .iter()
                .map(|t| wit_type_to_rust(resolve, t).0)
                .collect();
            (quote! { (#(#types),*) }, false)
        }
        TypeDefKind::Type(inner) => wit_type_to_rust(resolve, inner),
        TypeDefKind::Record(_)
        | TypeDefKind::Enum(_)
        | TypeDefKind::Variant(_)
        | TypeDefKind::Flags(_) => {
            let name = type_def.name.as_deref().unwrap_or("Unknown");
            let ident = format_ident!("{}", kebab_to_pascal(name));
            // Per-interface modules mean a struct in `llm` referencing
            // `Message` (defined in `types`) needs a qualified path.
            // The resolver tells us which interface owns the type;
            // emit `super::<iface>::<Name>` so the reference resolves
            // from inside the consuming module.
            if let TypeOwner::Interface(iface_id) = type_def.owner
                && let Some(ref iface_name) = resolve.interfaces[iface_id].name
            {
                let mod_ident = format_ident!("{}", kebab_to_snake(iface_name));
                (quote! { super::#mod_ident::#ident }, false)
            } else {
                (quote! { #ident }, false)
            }
        }
        _ => (quote! { ::serde_json::Value }, false),
    }
}

/// Convert `kebab-case` to `PascalCase`.
fn kebab_to_pascal(s: &str) -> String {
    s.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Convert `kebab-case` to `snake_case`.
fn kebab_to_snake(s: &str) -> String {
    s.replace('-', "_")
}

// ── Multi-package directory loading ──────────────────────────────────

/// Load a directory of WIT files that may contain multiple packages with
/// cross-package `use` references. Creates a temp directory with the standard
/// WIT deps layout so `push_dir` can resolve foreign references.
fn load_multi_package_dir(
    resolve: &mut Resolve,
    source_dir: &std::path::Path,
    wit_files: &[std::path::PathBuf],
    span: proc_macro2::Span,
) -> syn::Result<()> {
    let (file_contents, pkg_by_file) = read_and_index_packages(wit_files, span)?;
    let deps_by_file = detect_dependencies(&file_contents);

    let tmp_root = std::env::temp_dir().join(format!("astrid-wit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_root);

    // Sort: files with no deps first, then files with deps.
    let mut ordered: Vec<_> = file_contents.iter().collect();
    ordered.sort_by_key(|(path, _)| {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        deps_by_file.get(filename).map_or(0, Vec::len)
    });

    for (path, contents) in &ordered {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown.wit");
        let stem = filename.trim_end_matches(".wit");

        let pkg_dir = tmp_root.join(stem);
        std::fs::create_dir_all(&pkg_dir)
            .map_err(|e| syn::Error::new(span, format!("failed to create temp dir: {e}")))?;
        std::fs::write(pkg_dir.join(filename), contents)
            .map_err(|e| syn::Error::new(span, format!("failed to write temp WIT: {e}")))?;

        // Create deps/ with the files this package depends on.
        if let Some(deps) = deps_by_file.get(filename).filter(|d| !d.is_empty()) {
            write_dep_files(&pkg_dir, deps, &pkg_by_file, source_dir, span)?;
        }

        resolve
            .push_dir(&pkg_dir)
            .map_err(|e| syn::Error::new(span, format!("failed to resolve '{filename}': {e}")))?;
    }

    let _ = std::fs::remove_dir_all(&tmp_root);
    Ok(())
}

/// File contents paired with their paths.
type FileContents = Vec<(std::path::PathBuf, String)>;
/// Maps filename → package name (e.g., "types.wit" → "astrid:types").
type PkgIndex = std::collections::HashMap<String, String>;

/// Read all WIT files and extract package names from their `package` declarations.
fn read_and_index_packages(
    wit_files: &[std::path::PathBuf],
    span: proc_macro2::Span,
) -> syn::Result<(FileContents, PkgIndex)> {
    let mut file_contents = Vec::new();
    let mut pkg_by_file = std::collections::HashMap::new();

    for path in wit_files {
        let contents = std::fs::read_to_string(path).map_err(|e| {
            syn::Error::new(span, format!("failed to read '{}': {e}", path.display()))
        })?;
        for line in contents.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("package ") {
                let pkg_full = rest.trim_end_matches(';').trim();
                let pkg_name = pkg_full.split_once('@').map_or(pkg_full, |(n, _)| n);
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    pkg_by_file.insert(filename.to_string(), pkg_name.to_string());
                }
                break;
            }
        }
        file_contents.push((path.clone(), contents));
    }

    Ok((file_contents, pkg_by_file))
}

/// Detect which packages each file depends on via `use ns:name/...` statements.
fn detect_dependencies(
    file_contents: &[(std::path::PathBuf, String)],
) -> std::collections::HashMap<String, Vec<String>> {
    let mut deps_by_file = std::collections::HashMap::new();
    for (path, contents) in file_contents {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let mut deps = Vec::new();
        for line in contents.lines() {
            let trimmed = line.trim();
            if let Some(pkg) = trimmed
                .strip_prefix("use ")
                .and_then(|rest| rest.split('/').next())
            {
                let dep_name = pkg.split_once('@').map_or(pkg, |(n, _)| n);
                deps.push(dep_name.to_string());
            }
        }
        deps_by_file.insert(filename, deps);
    }
    deps_by_file
}

/// Write dependency WIT files into the `deps/` subdirectory for `push_dir` resolution.
fn write_dep_files(
    pkg_dir: &std::path::Path,
    deps: &[String],
    pkg_by_file: &std::collections::HashMap<String, String>,
    source_dir: &std::path::Path,
    span: proc_macro2::Span,
) -> syn::Result<()> {
    let deps_dir = pkg_dir.join("deps");
    for dep_pkg_name in deps {
        for (dep_filename, pkg_name) in pkg_by_file {
            if pkg_name == dep_pkg_name {
                let dep_dir_name = dep_pkg_name.replace(':', "-");
                let dep_target = deps_dir.join(&dep_dir_name);
                std::fs::create_dir_all(&dep_target)
                    .map_err(|e| syn::Error::new(span, format!("failed to create dep dir: {e}")))?;
                let dep_contents = std::fs::read_to_string(source_dir.join(dep_filename))
                    .map_err(|e| syn::Error::new(span, format!("failed to read dep: {e}")))?;
                std::fs::write(dep_target.join(dep_filename), dep_contents)
                    .map_err(|e| syn::Error::new(span, format!("failed to write dep: {e}")))?;
                break;
            }
        }
    }
    Ok(())
}

/// Collect all `.wit` file paths from a directory.
fn collect_wit_files(
    dir: &std::path::Path,
    span: proc_macro2::Span,
) -> syn::Result<Vec<std::path::PathBuf>> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        syn::Error::new(
            span,
            format!("failed to read directory '{}': {e}", dir.display()),
        )
    })?;
    Ok(entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|ext| ext.to_str()) == Some("wit"))
        .collect())
}
