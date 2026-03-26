//! WIT record → Rust struct code generator for the `wit_events!` proc macro.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use wit_parser::{Resolve, Type, TypeDefKind};

/// Parse a WIT file or directory and generate Rust type definitions.
///
/// Supports both single `.wit` files and directories containing multiple `.wit`
/// files (uses `push_dir` for proper multi-file package handling).
///
/// Also emits hidden `include_str!` constants so Cargo rebuilds when WIT changes.
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

        // Multi-package directories need the WIT deps/ layout for cross-package
        // `use` resolution. Build a temp structure: for each package, create a
        // directory with the .wit file and a deps/ dir containing its dependencies.
        load_multi_package_dir(&mut resolve, wit_path, &wit_files, span)?;

        for path in &wit_files {
            // Emit include_str! so Cargo tracks the file.
            if let Some(p) = path.to_str() {
                output.extend(quote! { const _: &str = include_str!(#p); });
            }
        }
    } else {
        // Single file.
        let contents = std::fs::read_to_string(wit_path).map_err(|e| {
            syn::Error::new(
                span,
                format!("failed to read WIT file '{}': {e}", wit_path.display()),
            )
        })?;
        resolve
            .push_str(wit_path.display().to_string(), &contents)
            .map_err(|e| syn::Error::new(span, format!("failed to parse WIT: {e}")))?;

        if let Some(p) = wit_path.to_str() {
            output.extend(quote! { const _: &str = include_str!(#p); });
        }
    }

    for (_, type_def) in &resolve.types {
        let Some(ref name) = type_def.name else {
            continue;
        };

        let rust_name = format_ident!("{}", kebab_to_pascal(name));
        let doc = type_def
            .docs
            .contents
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty());
        let doc_attr = doc.map(|d| quote! { #[doc = #d] });

        match &type_def.kind {
            TypeDefKind::Record(record) => {
                let fields = record_fields(&resolve, record);
                output.extend(quote! {
                    #doc_attr
                    #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
                    #[serde(rename_all = "kebab-case")]
                    pub struct #rust_name {
                        #(#fields)*
                    }
                });
            }
            TypeDefKind::Enum(enum_def) => {
                let variants: Vec<TokenStream> = enum_def
                    .cases
                    .iter()
                    .map(|case| {
                        let variant_name = format_ident!("{}", kebab_to_pascal(&case.name));
                        let case_doc = case_doc_attr(&case.docs);
                        quote! { #case_doc #variant_name, }
                    })
                    .collect();
                output.extend(quote! {
                    #doc_attr
                    #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
                    #[serde(rename_all = "kebab-case")]
                    pub enum #rust_name {
                        #(#variants)*
                    }
                });
            }
            TypeDefKind::Flags(flags_def) => {
                // WIT flags are bitmasks — multiple can be set simultaneously.
                // Generate an enum for individual values + Vec alias for the set.
                let variants: Vec<TokenStream> = flags_def
                    .flags
                    .iter()
                    .map(|flag| {
                        let variant_name = format_ident!("{}", kebab_to_pascal(&flag.name));
                        let flag_doc = case_doc_attr(&flag.docs);
                        quote! { #flag_doc #variant_name, }
                    })
                    .collect();
                let flag_enum_name = format_ident!("{}Flag", kebab_to_pascal(name));
                output.extend(quote! {
                    #doc_attr
                    #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
                    #[serde(rename_all = "kebab-case")]
                    pub enum #flag_enum_name {
                        #(#variants)*
                    }

                    /// Set of [`#flag_enum_name`] values (serializes as a JSON array).
                    pub type #rust_name = Vec<#flag_enum_name>;
                });
            }
            TypeDefKind::Variant(variant) => {
                // WIT variants map to adjacently-tagged Rust enums.
                let cases: Vec<TokenStream> = variant
                    .cases
                    .iter()
                    .map(|case| {
                        let variant_name = format_ident!("{}", kebab_to_pascal(&case.name));
                        let case_doc = case_doc_attr(&case.docs);
                        if let Some(ref ty) = case.ty {
                            let (rust_ty, _) = wit_type_to_rust(&resolve, ty);
                            quote! { #case_doc #variant_name(#rust_ty), }
                        } else {
                            quote! { #case_doc #variant_name, }
                        }
                    })
                    .collect();
                output.extend(quote! {
                    #doc_attr
                    #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
                    #[serde(tag = "tag", content = "value", rename_all = "kebab-case")]
                    pub enum #rust_name {
                        #(#cases)*
                    }
                });
            }
            // Skip types we don't codegen (resources, handles, type aliases, etc.).
            _ => {}
        }
    }

    Ok(output)
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
        // ErrorContext is an async resource handle — not meaningful in IPC events.
        Type::String | Type::ErrorContext => (quote! { String }, false),
        Type::Id(id) => {
            let type_def = &resolve.types[*id];
            match &type_def.kind {
                TypeDefKind::List(inner) => {
                    let (inner_ty, _) = wit_type_to_rust(resolve, inner);
                    (quote! { Vec<#inner_ty> }, false)
                }
                TypeDefKind::Option(inner) => {
                    let (inner_ty, inner_optional) = wit_type_to_rust(resolve, inner);
                    if inner_optional {
                        // option<option<T>> — preserve both layers.
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
                TypeDefKind::Record(_) | TypeDefKind::Enum(_) | TypeDefKind::Variant(_) => {
                    let name = type_def.name.as_deref().unwrap_or("Unknown");
                    let ident = format_ident!("{}", kebab_to_pascal(name));
                    (quote! { #ident }, false)
                }
                TypeDefKind::Flags(_) => {
                    // Flags type alias is Vec<FlagEnum>, reference by the set name.
                    let name = type_def.name.as_deref().unwrap_or("Unknown");
                    let ident = format_ident!("{}", kebab_to_pascal(name));
                    (quote! { #ident }, false)
                }
                // Fallback for unsupported types (resource, handle, future, stream).
                _ => (quote! { ::serde_json::Value }, false),
            }
        }
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

/// Load a directory of WIT files that may contain multiple packages with
/// cross-package `use` references.
///
/// Creates a temp directory with the standard WIT deps layout so `push_dir`
/// can resolve foreign references:
/// ```text
/// temp/<pkg>/
///   <pkg>.wit
///   deps/
///     <dep-pkg>/
///       <dep-pkg>.wit
/// ```
fn load_multi_package_dir(
    resolve: &mut Resolve,
    source_dir: &std::path::Path,
    wit_files: &[std::path::PathBuf],
    span: proc_macro2::Span,
) -> syn::Result<()> {
    use std::collections::HashMap;

    // Read all files and extract package names for dependency resolution.
    let mut file_contents: Vec<(std::path::PathBuf, String)> = Vec::new();
    let mut pkg_by_file: HashMap<String, String> = HashMap::new(); // filename → "ns:name"

    for path in wit_files {
        let contents = std::fs::read_to_string(path).map_err(|e| {
            syn::Error::new(span, format!("failed to read '{}': {e}", path.display()))
        })?;
        // Extract package name (e.g., "astrid:types" from "package astrid:types@1.0.0;")
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

    // Detect which packages each file depends on via `use ns:name/...`
    let mut deps_by_file: HashMap<String, Vec<String>> = HashMap::new();
    for (path, contents) in &file_contents {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let mut deps = Vec::new();
        for line in contents.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("use ") {
                // "use astrid:types/types.{message};" → "astrid:types"
                if let Some(pkg) = rest.split('/').next() {
                    let dep_name = pkg.split_once('@').map_or(pkg, |(n, _)| n);
                    deps.push(dep_name.to_string());
                }
            }
        }
        deps_by_file.insert(filename, deps);
    }

    // Build temp dirs and load packages in dependency order.
    // Foundation packages (no deps) first, then packages with deps.
    let tmp_root = std::env::temp_dir().join(format!("astrid-wit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_root);

    let mut loaded_pkgs: Vec<String> = Vec::new();

    // Sort: files with no deps first, then files with deps.
    let mut ordered: Vec<_> = file_contents.iter().collect();
    ordered.sort_by_key(|(path, _)| {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let dep_count = deps_by_file.get(filename).map_or(0, |d| d.len());
        dep_count
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
        if let Some(deps) = deps_by_file.get(filename) {
            if !deps.is_empty() {
                let deps_dir = pkg_dir.join("deps");
                for dep_pkg_name in deps {
                    // Find the file that provides this package.
                    for (dep_filename, pkg_name) in &pkg_by_file {
                        if pkg_name == dep_pkg_name {
                            let dep_dir_name = dep_pkg_name.replace(':', "-");
                            let dep_target = deps_dir.join(&dep_dir_name);
                            std::fs::create_dir_all(&dep_target).map_err(|e| {
                                syn::Error::new(span, format!("failed to create dep dir: {e}"))
                            })?;
                            // Copy the dep file.
                            let dep_source = source_dir.join(dep_filename);
                            let dep_contents =
                                std::fs::read_to_string(&dep_source).map_err(|e| {
                                    syn::Error::new(
                                        span,
                                        format!(
                                            "failed to read dep '{}': {e}",
                                            dep_source.display()
                                        ),
                                    )
                                })?;
                            std::fs::write(dep_target.join(dep_filename), dep_contents).map_err(
                                |e| syn::Error::new(span, format!("failed to write dep: {e}")),
                            )?;
                            break;
                        }
                    }
                }
            }
        }

        resolve
            .push_dir(&pkg_dir)
            .map_err(|e| syn::Error::new(span, format!("failed to resolve '{}': {e}", filename)))?;

        if let Some(pkg_name) = pkg_by_file.get(filename) {
            loaded_pkgs.push(pkg_name.clone());
        }
    }

    // Cleanup temp dir (best-effort).
    let _ = std::fs::remove_dir_all(&tmp_root);

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
