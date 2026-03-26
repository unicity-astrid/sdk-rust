//! WIT record → Rust struct code generator for the `wit_events!` proc macro.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use wit_parser::{Resolve, Type, TypeDefKind};

/// Parse a WIT file and generate Rust struct/enum definitions for all types.
pub(crate) fn generate(
    wit_path: &std::path::Path,
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    let contents = std::fs::read_to_string(wit_path).map_err(|e| {
        syn::Error::new(
            span,
            format!("failed to read WIT file '{}': {e}", wit_path.display()),
        )
    })?;

    let mut resolve = Resolve::default();
    resolve
        .push_str(wit_path.display().to_string(), &contents)
        .map_err(|e| syn::Error::new(span, format!("failed to parse WIT: {e}")))?;

    let mut output = TokenStream::new();

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
                        let case_doc = case
                            .docs
                            .contents
                            .as_deref()
                            .map(str::trim)
                            .filter(|d| !d.is_empty());
                        let case_doc_attr = case_doc.map(|d| quote! { #[doc = #d] });
                        quote! { #case_doc_attr #variant_name, }
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
                let variants: Vec<TokenStream> = flags_def
                    .flags
                    .iter()
                    .map(|flag| {
                        let variant_name = format_ident!("{}", kebab_to_pascal(&flag.name));
                        let flag_doc = flag
                            .docs
                            .contents
                            .as_deref()
                            .map(str::trim)
                            .filter(|d| !d.is_empty());
                        let flag_doc_attr = flag_doc.map(|d| quote! { #[doc = #d] });
                        quote! { #flag_doc_attr #variant_name, }
                    })
                    .collect();
                // Flags are serialized as arrays of strings.
                output.extend(quote! {
                    #doc_attr
                    #[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
                    #[serde(rename_all = "kebab-case")]
                    pub enum #rust_name {
                        #(#variants)*
                    }
                });
            }
            // Skip types we don't codegen (resources, handles, aliases, etc.).
            _ => {}
        }
    }

    Ok(output)
}

/// Generate field definitions for a WIT record.
fn record_fields(resolve: &Resolve, record: &wit_parser::Record) -> Vec<TokenStream> {
    record
        .fields
        .iter()
        .map(|field| {
            let field_name = format_ident!("{}", kebab_to_snake(&field.name));
            let (ty, is_optional) = wit_type_to_rust(resolve, &field.ty);

            let doc = field
                .docs
                .contents
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty());
            let doc_attr = doc.map(|d| quote! { #[doc = #d] });

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
        Type::Id(id) => {
            let type_def = &resolve.types[*id];
            match &type_def.kind {
                TypeDefKind::List(inner) => {
                    let (inner_ty, _) = wit_type_to_rust(resolve, inner);
                    (quote! { Vec<#inner_ty> }, false)
                }
                TypeDefKind::Option(inner) => {
                    let (inner_ty, _) = wit_type_to_rust(resolve, inner);
                    (quote! { #inner_ty }, true)
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
                TypeDefKind::Record(_) | TypeDefKind::Enum(_) | TypeDefKind::Flags(_) => {
                    // Reference to another named type — use its PascalCase name.
                    let name = type_def.name.as_deref().unwrap_or("Unknown");
                    let ident = format_ident!("{}", kebab_to_pascal(name));
                    (quote! { #ident }, false)
                }
                // Fallback for unsupported types.
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
