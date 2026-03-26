//! Procedural macros for building Astrid OS User-Space Capsules.
//!
//! This crate provides the `#[astrid::capsule]` macro to automatically
//! generate the Component Model `impl Guest` trait implementation and
//! `export!()` wiring for the Astrid WASM capsule world.

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{ImplItem, ItemImpl};

/// Marks an `impl` block as the entry point for an Astrid Capsule.
///
/// This macro automatically generates the Component Model `impl Guest`
/// trait and `export!()` call required by the Astrid Kernel, routing
/// incoming IPC/Tool/Hook requests to annotated methods within the block.
#[proc_macro_attribute]
pub fn capsule(attr: TokenStream, item: TokenStream) -> TokenStream {
    capsule_impl(attr.into(), item.into()).into()
}

/// Extract doc comments from a list of attributes, joining all lines.
///
/// `/// Foo` becomes `#[doc = " Foo"]` — we strip the leading space and
/// join with newlines so the full documentation is preserved.
fn extract_doc_comments(attrs: &[syn::Attribute]) -> Option<String> {
    let mut lines = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc")
            && let syn::Meta::NameValue(nv) = &attr.meta
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
        {
            let line = s.value();
            lines.push(line.strip_prefix(' ').unwrap_or(&line).to_string());
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n").trim().to_string())
    }
}

#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
fn capsule_impl(
    attr: proc_macro2::TokenStream,
    item: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let mut input: ItemImpl = match syn::parse2(item) {
        Ok(i) => i,
        Err(e) => return e.into_compile_error(),
    };
    let struct_name = &input.self_ty.clone();

    // `#[capsule(state)]` explicitly opts into stateful mode.
    // Stateful mode is also implied automatically when any method takes `&mut self`.
    let attr_is_stateful = syn::parse2::<syn::Ident>(attr)
        .map(|ident| ident == "state")
        .unwrap_or(false);

    // Detect stateful capsules by checking if any method takes `&mut self`.
    // Stateful capsules have their struct loaded from KV before each handler
    // and saved back after. No extra attribute needed — `&mut self` implies state.
    let is_stateful = attr_is_stateful
        || input.items.iter().any(|item| {
            if let ImplItem::Fn(method) = item {
                method
                    .sig
                    .inputs
                    .iter()
                    .any(|arg| matches!(arg, syn::FnArg::Receiver(r) if r.mutability.is_some()))
            } else {
                false
            }
        });

    // Extract doc comments from the impl block as the capsule-level description.
    let capsule_description = extract_doc_comments(&input.attrs);

    let mut command_arms = Vec::new();
    let mut hook_arms = Vec::new();
    let mut schema_arms = Vec::new();
    let mut install_method: Option<syn::Ident> = None;
    let mut upgrade_method: Option<syn::Ident> = None;
    let mut run_method: Option<syn::Ident> = None;

    for item in &mut input.items {
        if let ImplItem::Fn(method) = item {
            let method_name = &method.sig.ident;

            // Extract the argument type (the first Typed argument) for schema generation
            let mut arg_type = None;
            for arg in &method.sig.inputs {
                if let syn::FnArg::Typed(pat_type) = arg {
                    arg_type = Some(pat_type.ty.clone());
                    break;
                }
            }

            // Extract and process astrid attributes, then remove them
            let mut extracted_attrs = Vec::new();
            method.attrs.retain(|attr| {
                if attr.path().segments.len() == 2 && attr.path().segments[0].ident == "astrid" {
                    extracted_attrs.push(attr.clone());
                    false // Remove from the AST
                } else {
                    true // Keep other attributes
                }
            });

            // Determine if this method is marked as mutable.
            // Supported forms:
            //   #[astrid::mutable]                    (standalone, legacy)
            //   #[astrid::tool("name", mutable)]      (inline, preferred)
            //   #[astrid::tool(mutable)]              (inline, name inferred)
            let has_standalone_mutable = extracted_attrs
                .iter()
                .any(|a| a.path().segments[1].ident == "mutable");
            // Inline mutable is checked per-attr below when we parse tool args.
            // This flag accumulates both sources.
            let mut is_mutable = has_standalone_mutable;

            // Extract doc comments from the method for tool/command descriptions.
            let doc_description = extract_doc_comments(&method.attrs);

            for attr in &extracted_attrs {
                // All attrs here have exactly 2 segments (enforced by the retain
                // filter above), but guard defensively in case that changes.
                if attr.path().segments.len() < 2 {
                    continue;
                }
                let attr_name = &attr.path().segments[1].ident;

                // ---------------------------------------------------------------
                // Lifecycle hooks: install / upgrade / run
                // ---------------------------------------------------------------
                if (attr_name == "install" || attr_name == "upgrade" || attr_name == "run")
                    && is_mutable
                {
                    return syn::Error::new_spanned(
                        attr,
                        "#[astrid::mutable] cannot be used on lifecycle hooks or #[astrid::run]",
                    )
                    .into_compile_error();
                }

                if attr_name == "install" {
                    if install_method.is_some() {
                        return syn::Error::new_spanned(
                            attr,
                            "only one #[astrid::install] method is allowed per capsule",
                        )
                        .into_compile_error();
                    }
                    // Validate: no extra typed args (only &self)
                    if arg_type.is_some() {
                        return syn::Error::new_spanned(
                            &method.sig,
                            "#[astrid::install] must have signature: fn(&self) -> Result<(), SysError>",
                        )
                        .into_compile_error();
                    }
                    install_method = Some(method_name.clone());
                    continue;
                }

                if attr_name == "upgrade" {
                    if upgrade_method.is_some() {
                        return syn::Error::new_spanned(
                            attr,
                            "only one #[astrid::upgrade] method is allowed per capsule",
                        )
                        .into_compile_error();
                    }
                    // Validate: exactly one typed arg that must be &str
                    let is_ref_str = arg_type.as_ref().is_some_and(|ty| {
                        if let syn::Type::Reference(r) = ty.as_ref()
                            && let syn::Type::Path(p) = r.elem.as_ref()
                        {
                            return p.path.is_ident("str");
                        }
                        false
                    });
                    if !is_ref_str {
                        return syn::Error::new_spanned(
                            &method.sig,
                            "#[astrid::upgrade] must have signature: fn(&self, prev_version: &str) -> Result<(), SysError>",
                        )
                        .into_compile_error();
                    }
                    upgrade_method = Some(method_name.clone());
                    continue;
                }

                if attr_name == "run" {
                    if run_method.is_some() {
                        return syn::Error::new_spanned(
                            attr,
                            "only one #[astrid::run] method is allowed per capsule",
                        )
                        .into_compile_error();
                    }
                    // Validate: no extra typed args (only &self)
                    if arg_type.is_some() {
                        return syn::Error::new_spanned(
                            &method.sig,
                            "#[astrid::run] must have signature: fn(&self) -> Result<(), SysError>",
                        )
                        .into_compile_error();
                    }
                    run_method = Some(method_name.clone());
                    continue;
                }

                // ---------------------------------------------------------------
                // Existing dispatch attrs: tool / command / interceptor / cron
                // ---------------------------------------------------------------

                // Parse tool/command/interceptor/cron arguments.
                // Supports: ("name"), ("name", mutable), (mutable), or empty.
                let name_val;
                {
                    let mut parsed_name = None;
                    let mut parsed_mutable = false;
                    if let Ok(args) = attr.parse_args_with(
                        syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
                    ) {
                        for arg in &args {
                            match arg {
                                syn::Expr::Lit(syn::ExprLit {
                                    lit: syn::Lit::Str(s),
                                    ..
                                }) => {
                                    parsed_name = Some(s.value());
                                }
                                syn::Expr::Path(p) if p.path.is_ident("mutable") => {
                                    parsed_mutable = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    name_val = parsed_name.unwrap_or_else(|| method_name.to_string());
                    if parsed_mutable {
                        is_mutable = true;
                    }
                }

                // ---------------------------------------------------------
                // Build call expressions for interceptors and commands
                // ---------------------------------------------------------

                let call_expr = if arg_type.is_some() {
                    quote! {
                        {
                            let args = ::serde_json::from_slice(&payload)
                                .map_err(|e| format!("failed to parse arguments: {}", e))?;
                            instance.#method_name(args).map_err(|e| e.to_string())?
                        }
                    }
                } else {
                    quote! {
                        instance.#method_name().map_err(|e| e.to_string())?
                    }
                };

                let call_expr_stateless = if arg_type.is_some() {
                    quote! {
                        {
                            let args = ::serde_json::from_slice(&payload)
                                .map_err(|e| format!("failed to parse arguments: {}", e))?;
                            get_instance().#method_name(args).map_err(|e| e.to_string())?
                        }
                    }
                } else {
                    quote! {
                        get_instance().#method_name().map_err(|e| e.to_string())?
                    }
                };

                let execute_block = if is_stateful {
                    quote! {
                        let mut instance: #struct_name = match ::astrid_sdk::prelude::kv::get_json("__state") {
                            Ok(state) => state,
                            Err(::astrid_sdk::SysError::JsonError(_)) => Default::default(),
                            Err(e) => return ::astrid_sdk::astrid_sys::CapsuleResult {
                                action: "deny".into(),
                                data: Some(format!("failed to load state: {}", e)),
                            },
                        };
                        let result = match (|| -> Result<_, String> {
                            let val = #call_expr;
                            Ok(val)
                        })() {
                            Ok(val) => val,
                            Err(e) => return ::astrid_sdk::astrid_sys::CapsuleResult {
                                action: "deny".into(),
                                data: Some(e),
                            },
                        };
                        if let Err(e) = ::astrid_sdk::prelude::kv::set_json("__state", &instance) {
                            return ::astrid_sdk::astrid_sys::CapsuleResult {
                                action: "deny".into(),
                                data: Some(format!("failed to save state: {}", e)),
                            };
                        }
                        let res_json = match ::serde_json::to_string(&result) {
                            Ok(s) => s,
                            Err(e) => return ::astrid_sdk::astrid_sys::CapsuleResult {
                                action: "deny".into(),
                                data: Some(format!("failed to serialize result: {}", e)),
                            },
                        };
                        // If the result is JSON null (from () or None), return empty
                        // data so the interceptor chain keeps the original payload.
                        if res_json == "null" {
                            return ::astrid_sdk::astrid_sys::CapsuleResult {
                                action: "continue".into(),
                                data: None,
                            };
                        }
                        return ::astrid_sdk::astrid_sys::CapsuleResult {
                            action: "continue".into(),
                            data: Some(res_json),
                        };
                    }
                } else {
                    quote! {
                        let result = match (|| -> Result<_, String> {
                            let val = #call_expr_stateless;
                            Ok(val)
                        })() {
                            Ok(val) => val,
                            Err(e) => return ::astrid_sdk::astrid_sys::CapsuleResult {
                                action: "deny".into(),
                                data: Some(e),
                            },
                        };
                        let res_json = match ::serde_json::to_string(&result) {
                            Ok(s) => s,
                            Err(e) => return ::astrid_sdk::astrid_sys::CapsuleResult {
                                action: "deny".into(),
                                data: Some(format!("failed to serialize result: {}", e)),
                            },
                        };
                        // If the result is JSON null (from () or None), return empty
                        // data so the interceptor chain keeps the original payload.
                        if res_json == "null" {
                            return ::astrid_sdk::astrid_sys::CapsuleResult {
                                action: "continue".into(),
                                data: None,
                            };
                        }
                        return ::astrid_sdk::astrid_sys::CapsuleResult {
                            action: "continue".into(),
                            data: Some(res_json),
                        };
                    }
                };

                if attr_name == "tool" {
                    // Tools are routed through `astrid_hook_trigger` as
                    // interceptor actions named `"tool_execute_<tool_name>"`.
                    // The interceptor payload carries the IPC ToolExecuteRequest
                    // fields (call_id, tool_name, arguments). Results are
                    // published back via IPC rather than returned directly.
                    let action_name = format!("tool_execute_{name_val}");

                    // Build the call expression that invokes the user's method.
                    // For tools with args, we deserialize from the JSON `arguments` Value.
                    let tool_call_expr = if arg_type.is_some() {
                        quote! {
                            let args = ::serde_json::from_value(tool_req.arguments.clone())
                                .map_err(|e| format!("failed to parse tool arguments: {}", e))?;
                            instance.#method_name(args).map_err(|e| e.to_string())?
                        }
                    } else {
                        quote! {
                            instance.#method_name().map_err(|e| e.to_string())?
                        }
                    };

                    let tool_call_expr_stateless = if arg_type.is_some() {
                        quote! {
                            let args = ::serde_json::from_value(tool_req.arguments.clone())
                                .map_err(|e| format!("failed to parse tool arguments: {}", e))?;
                            get_instance().#method_name(args).map_err(|e| e.to_string())?
                        }
                    } else {
                        quote! {
                            get_instance().#method_name().map_err(|e| e.to_string())?
                        }
                    };

                    let (call_expr, state_setup, state_teardown) = if is_stateful {
                        (
                            tool_call_expr,
                            quote! {
                                let mut instance: #struct_name = match ::astrid_sdk::prelude::kv::get_json("__state") {
                                    Ok(state) => state,
                                    Err(::astrid_sdk::SysError::JsonError(_)) => Default::default(),
                                    Err(e) => {
                                        let err_call_id = tool_req.call_id.clone();
                                        let _ = ::astrid_sdk::prelude::ipc::publish_json(
                                            &format!("tool.v1.execute.{}.result", #name_val),
                                            &::serde_json::json!({
                                                "type": "tool_execute_result",
                                                "call_id": err_call_id.clone(),
                                                "result": {
                                                    "call_id": err_call_id,
                                                    "content": format!("failed to load state: {}", e),
                                                    "is_error": true,
                                                }
                                            }),
                                        );
                                        return ::astrid_sdk::astrid_sys::CapsuleResult {
                                            action: "continue".into(),
                                            data: None,
                                        };
                                    }
                                };
                            },
                            quote! {
                                if let Err(e) = ::astrid_sdk::prelude::kv::set_json("__state", &instance) {
                                    let save_call_id = call_id.clone();
                                    let _ = ::astrid_sdk::prelude::ipc::publish_json(
                                        &format!("tool.v1.execute.{}.result", #name_val),
                                        &::serde_json::json!({
                                            "type": "tool_execute_result",
                                            "call_id": save_call_id.clone(),
                                            "result": {
                                                "call_id": save_call_id,
                                                "content": format!("failed to save state: {}", e),
                                                "is_error": true,
                                            }
                                        }),
                                    );
                                    return ::astrid_sdk::astrid_sys::CapsuleResult {
                                        action: "continue".into(),
                                        data: None,
                                    };
                                }
                            },
                        )
                    } else {
                        (tool_call_expr_stateless, quote! {}, quote! {})
                    };

                    let tool_execute_block = quote! {
                        let tool_req: __AstridToolExecPayload = match ::serde_json::from_slice(&payload) {
                            Ok(r) => r,
                            Err(e) => return ::astrid_sdk::astrid_sys::CapsuleResult {
                                action: "deny".into(),
                                data: Some(format!("failed to parse tool execute payload: {}", e)),
                            },
                        };
                        let call_id = tool_req.call_id.clone();
                        #state_setup
                        let result_str = match (|| -> Result<String, String> {
                            let result = { #call_expr };
                            let serialized = ::serde_json::to_string(&result)
                                .map_err(|e| format!("failed to serialize tool result: {}", e))?;
                            Ok(serialized)
                        })() {
                            Ok(s) => (s, false),
                            Err(e) => (format!("{}", e), true),
                        };
                        // Only persist state on success — partial mutations from
                        // a failed tool call should not be saved.
                        if !result_str.1 {
                            #state_teardown
                        }
                        let ipc_result = ::serde_json::json!({
                            "type": "tool_execute_result",
                            "call_id": call_id.clone(),
                            "result": {
                                "call_id": call_id,
                                "content": result_str.0,
                                "is_error": result_str.1,
                            }
                        });
                        let topic = format!("tool.v1.execute.{}.result", #name_val);
                        let _ = ::astrid_sdk::prelude::ipc::publish_json(&topic, &ipc_result);
                        return ::astrid_sdk::astrid_sys::CapsuleResult {
                            action: "continue".into(),
                            data: None,
                        };
                    };

                    hook_arms.push(quote! {
                        #action_name => { #tool_execute_block }
                    });

                    // Automatically generate schemars extraction for this tool.
                    // Doc comments on the method become the tool description.
                    let desc_insertion = if let Some(desc) = &doc_description {
                        quote! {
                            let metadata = schema.schema.metadata.get_or_insert_with(Default::default);
                            metadata.description = Some(#desc.to_string());
                        }
                    } else {
                        quote! {}
                    };

                    if let Some(ty) = &arg_type {
                        schema_arms.push(quote! {
                            {
                                let mut schema = ::astrid_sdk::schemars::schema_for!(#ty);
                                schema.schema.extensions.insert(
                                    "mutable".to_string(),
                                    ::serde_json::json!(#is_mutable),
                                );
                                #desc_insertion
                                let desc_str: &str = schema.schema.metadata
                                    .as_ref()
                                    .and_then(|m| m.description.as_deref())
                                    .unwrap_or("");
                                let mut input_schema = ::serde_json::to_value(&schema)
                                    .unwrap_or(::serde_json::json!({"type": "object"}));
                                // Ensure `properties` exists — OpenAI function calling API requires it
                                // even for empty structs where schemars omits it.
                                if let Some(obj) = input_schema.as_object_mut() {
                                    obj.entry("properties").or_insert(::serde_json::json!({}));
                                }
                                tools.push(::serde_json::json!({
                                    "name": #name_val,
                                    "description": desc_str,
                                    "input_schema": input_schema,
                                }));
                            }
                        });
                    } else {
                        schema_arms.push(quote! {
                            {
                                tools.push(::serde_json::json!({
                                    "name": #name_val,
                                    "description": "",
                                    "input_schema": { "type": "object", "properties": {} },
                                }));
                            }
                        });
                    }
                } else if attr_name == "command" {
                    command_arms.push(quote! {
                        #name_val => { #execute_block }
                    });
                } else if attr_name == "interceptor" {
                    hook_arms.push(quote! {
                        #name_val => { #execute_block }
                    });
                }
            }
        }
    }

    // If there are any tools, generate a shared `tool_describe` interceptor arm
    // that returns schemas for all tools when requested.
    if !schema_arms.is_empty() {
        let capsule_description_for_describe = if let Some(desc) = &capsule_description {
            quote! { Some(#desc) }
        } else {
            quote! { None }
        };

        hook_arms.push(quote! {
            "tool_describe" => {
                // Build tool list as an array of {name, description, input_schema} objects.
                // Format matches the MCP bridge and what the prompt-builder expects.
                let mut tools: Vec<::serde_json::Value> = Vec::new();
                #( #schema_arms )*

                let capsule_desc: Option<&str> = #capsule_description_for_describe;
                let response = ::serde_json::json!({
                    "tools": tools,
                    "description": capsule_desc.unwrap_or(""),
                });

                let data = match ::serde_json::to_string(&response) {
                    Ok(s) => s,
                    Err(e) => {
                        return ::astrid_sdk::astrid_sys::CapsuleResult {
                            action: "deny".into(),
                            data: Some(format!("failed to serialize tool_describe: {e}")),
                        };
                    }
                };
                return ::astrid_sdk::astrid_sys::CapsuleResult {
                    action: "continue".into(),
                    data: Some(data),
                };
            }
        });
    }

    let instance_block = if is_stateful {
        quote! {}
    } else {
        quote! {
            static INSTANCE: ::std::sync::OnceLock<#struct_name> = ::std::sync::OnceLock::new();

            fn get_instance() -> &'static #struct_name {
                INSTANCE.get_or_init(|| #struct_name::default())
            }
        }
    };

    // Commands are now dispatched inside astrid_hook_trigger alongside interceptors.
    // Merge command arms into hook_arms so they're part of the same match.
    hook_arms.extend(command_arms);

    // --- Generate the `astrid_hook_trigger` body ---
    let hook_trigger_body = if hook_arms.is_empty() {
        quote! {
            ::astrid_sdk::astrid_sys::CapsuleResult {
                action: "deny".into(),
                data: Some(format!("unknown hook action: {}", action)),
            }
        }
    } else {
        quote! {
            match action.as_str() {
                #( #hook_arms )*
                _ => ::astrid_sdk::astrid_sys::CapsuleResult {
                    action: "deny".into(),
                    data: Some(format!("unknown hook action: {}", action)),
                },
            }
        }
    };

    // --- Generate the install body ---
    let install_body = if let Some(method_name) = &install_method {
        if is_stateful {
            quote! {
                // Install always starts from Default - there is no prior state.
                let mut instance = #struct_name::default();
                if let Err(e) = instance.#method_name() {
                    // Do NOT persist state on install failure — the capsule
                    // should go through install again on next activation.
                    let _ = ::astrid_sdk::prelude::log::error(
                        &format!("install hook failed: {e:?}")
                    );
                    return;
                }
                if let Err(e) = ::astrid_sdk::prelude::kv::set_json("__state", &instance) {
                    let _ = ::astrid_sdk::prelude::log::error(
                        &format!("install: failed to persist state: {e}")
                    );
                    return;
                }
            }
        } else {
            quote! {
                let instance = #struct_name::default();
                if let Err(e) = instance.#method_name() {
                    ::astrid_sdk::prelude::log::error(
                        &format!("install hook failed: {e:?}")
                    );
                }
            }
        }
    } else {
        quote! {}
    };

    // --- Generate the upgrade body ---
    let upgrade_body = if let Some(method_name) = &upgrade_method {
        if is_stateful {
            quote! {
                // Upgrade loads existing state; falls back to Default on deserialization failure.
                let mut instance: #struct_name = match ::astrid_sdk::prelude::kv::get_json("__state") {
                    Ok(state) => state,
                    Err(e @ ::astrid_sdk::SysError::JsonError(_)) => {
                        let _ = ::astrid_sdk::log::warn(
                            &format!("failed to deserialize state, falling back to default: {}", e),
                        );
                        Default::default()
                    }
                    Err(e) => {
                        let _ = ::astrid_sdk::prelude::log::error(
                            &format!("upgrade: failed to load state: {e}")
                        );
                        return;
                    }
                };
                // Read prev_version from capsule config (set by kernel before upgrade).
                let prev_version = ::astrid_sdk::prelude::env::var("prev_version")
                    .unwrap_or_default();
                if let Err(e) = instance.#method_name(&prev_version) {
                    let _ = ::astrid_sdk::prelude::log::error(
                        &format!("upgrade hook failed: {e:?}")
                    );
                    return;
                }
                if let Err(e) = ::astrid_sdk::prelude::kv::set_json("__state", &instance) {
                    let _ = ::astrid_sdk::prelude::log::error(
                        &format!("upgrade: failed to persist state: {e}")
                    );
                    return;
                }
            }
        } else {
            quote! {
                let instance = #struct_name::default();
                // Read prev_version from capsule config (set by kernel before upgrade).
                let prev_version = ::astrid_sdk::prelude::env::var("prev_version")
                    .unwrap_or_default();
                if let Err(e) = instance.#method_name(&prev_version) {
                    let _ = ::astrid_sdk::prelude::log::error(
                        &format!("upgrade hook failed: {e:?}")
                    );
                }
            }
        }
    } else {
        quote! {}
    };

    // --- Generate the run body ---
    let run_body = if let Some(method_name) = &run_method {
        if is_stateful {
            quote! {
                let mut instance: #struct_name = match ::astrid_sdk::prelude::kv::get_json("__state") {
                    Ok(state) => state,
                    Err(e @ ::astrid_sdk::SysError::JsonError(_)) => {
                        let _ = ::astrid_sdk::log::warn(
                            &format!("failed to deserialize state, falling back to default: {}", e),
                        );
                        Default::default()
                    }
                    Err(e) => {
                        let _ = ::astrid_sdk::prelude::log::error(
                            &format!("run: failed to load state: {e}")
                        );
                        return;
                    }
                };
                if let Err(e) = instance.#method_name() {
                    let _ = ::astrid_sdk::prelude::log::error(
                        &format!("run loop exited with error: {e:?}")
                    );
                }
            }
        } else {
            quote! {
                if let Err(e) = get_instance().#method_name() {
                    let _ = ::astrid_sdk::prelude::log::error(
                        &format!("run loop exited with error: {e:?}")
                    );
                }
            }
        }
    } else {
        quote! {}
    };

    let expanded = quote! {
        #input

        // Enforce Default implementation with a clearer compiler error
        const _: () = {
            fn assert_default<T: ::std::default::Default>() {}
            let _ = assert_default::<#struct_name>;
        };

        // -------------------------------------------------------------------
        // The Astrid OS Component Model ABI
        // -------------------------------------------------------------------

        /// Deserialization helper for tool execution IPC payloads.
        /// Mirrors the fields from `IpcPayload::ToolExecuteRequest`.
        #[derive(::serde::Deserialize)]
        struct __AstridToolExecPayload {
            call_id: String,
            tool_name: String,
            arguments: ::serde_json::Value,
        }

        #instance_block

        struct __AstridExport;

        impl ::astrid_sdk::astrid_sys::Guest for __AstridExport {
            fn astrid_hook_trigger(action: String, payload: Vec<u8>) -> ::astrid_sdk::astrid_sys::CapsuleResult {
                #hook_trigger_body
            }

            fn run() {
                #run_body
            }

            fn astrid_install() {
                #install_body
            }

            fn astrid_upgrade() {
                #upgrade_body
            }
        }

        ::astrid_sdk::astrid_sys::export!(__AstridExport with_types_in ::astrid_sdk::astrid_sys);
    };

    expanded
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: proc_macro2::TokenStream::to_string() serialises `json!(true)` as
    // "json ! (true)" with spaces around the bang and parens. These assertions
    // rely on that stable (but undocumented) formatting.

    #[test]
    fn mutable_attr_sets_true_in_schema() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("write_file")]
                #[astrid::mutable]
                fn write_file(&self, args: WriteArgs) -> Result<WriteResult, Error> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();

        assert!(
            output.contains("json ! (true)"),
            "Expected json!(true) in generated schema, but got:\n{output}"
        );
    }

    #[test]
    fn non_mutable_tool_sets_false_in_schema() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("read_file")]
                fn read_file(&self, args: ReadArgs) -> Result<ReadResult, Error> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();

        assert!(
            output.contains("json ! (false)"),
            "Expected json!(false) in generated schema, but got:\n{output}"
        );
        assert!(
            !output.contains("json ! (true)"),
            "Non-mutable tool should not have json!(true)"
        );
    }

    #[test]
    fn inline_mutable_in_tool_attr() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("write_file", mutable)]
                fn write_file(&self, args: WriteArgs) -> Result<WriteResult, Error> {
                    todo!()
                }
            }
        };
        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("json ! (true)"),
            "Inline mutable should produce json!(true), got:\n{output}"
        );
    }

    #[test]
    fn inline_mutable_name_inferred() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool(mutable)]
                fn write_file(&self, args: WriteArgs) -> Result<WriteResult, Error> {
                    todo!()
                }
            }
        };
        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("json ! (true)"),
            "Inline mutable with inferred name should produce json!(true), got:\n{output}"
        );
        assert!(
            output.contains("\"write_file\""),
            "Name should be inferred from method, got:\n{output}"
        );
    }

    /// `#[astrid::mutable]` listed before `#[astrid::tool]` must still work (legacy).
    #[test]
    fn mutable_before_tool_attr_order() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::mutable]
                #[astrid::tool("delete_file")]
                fn delete_file(&self, args: DeleteArgs) -> Result<DeleteResult, Error> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();

        assert!(
            output.contains("json ! (true)"),
            "Mutable-before-tool should still produce json!(true), got:\n{output}"
        );
    }

    #[test]
    fn install_generates_export() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::install]
                fn install(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("astrid_install"),
            "Expected astrid_install export, got:\n{output}"
        );
        // Should NOT generate upgrade logic (method body should be empty)
        assert!(
            output.contains("fn astrid_upgrade"),
            "Should always generate astrid_upgrade stub in Guest impl"
        );
        // Non-stateful install should NOT persist state
        assert!(
            !output.contains("set_json"),
            "Non-stateful install should not call set_json"
        );
    }

    #[test]
    fn upgrade_generates_export() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::upgrade]
                fn upgrade(&self, prev_version: &str) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("astrid_upgrade"),
            "Expected astrid_upgrade export, got:\n{output}"
        );
        assert!(
            output.contains("prev_version"),
            "Upgrade export should reference prev_version"
        );
        // Component Model: prev_version comes from env::var, not input bytes
        assert!(
            output.contains("env :: var"),
            "Upgrade export should read prev_version from env::var, got:\n{output}"
        );
        // Non-stateful upgrade should NOT load/persist state
        assert!(
            !output.contains("set_json"),
            "Non-stateful upgrade should not call set_json"
        );
        assert!(
            !output.contains("get_json"),
            "Non-stateful upgrade should not call get_json"
        );
    }

    #[test]
    fn no_lifecycle_no_exports() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("do_thing")]
                fn do_thing(&self, args: DoArgs) -> Result<String, Error> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        // Tool dispatch should be routed through astrid_hook_trigger
        assert!(
            output.contains("astrid_hook_trigger"),
            "Should generate astrid_hook_trigger for tool dispatch"
        );
        assert!(
            output.contains("tool_execute_do_thing"),
            "Should generate tool_execute_do_thing interceptor arm"
        );
        // Component Model always generates all four exports via Guest trait
        assert!(
            output.contains("fn astrid_install"),
            "Guest impl always has astrid_install"
        );
        assert!(
            output.contains("fn astrid_upgrade"),
            "Guest impl always has astrid_upgrade"
        );
        assert!(output.contains("fn run"), "Guest impl always has run");
    }

    #[test]
    fn duplicate_install_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::install]
                fn install(&self) -> Result<(), SysError> {
                    todo!()
                }
                #[astrid::install]
                fn install2(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Duplicate #[astrid::install] should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn duplicate_upgrade_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::upgrade]
                fn upgrade1(&self, v: &str) -> Result<(), SysError> {
                    todo!()
                }
                #[astrid::upgrade]
                fn upgrade2(&self, v: &str) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Duplicate #[astrid::upgrade] should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn install_with_args_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::install]
                fn install(&self, args: InstallArgs) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Install with args should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn upgrade_without_args_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::upgrade]
                fn upgrade(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Upgrade without prev_version arg should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn upgrade_with_wrong_arg_type_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::upgrade]
                fn upgrade(&self, prev_version: u32) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Upgrade with u32 arg should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn upgrade_with_string_arg_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::upgrade]
                fn upgrade(&self, prev_version: String) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Upgrade with String (not &str) arg should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn both_install_and_upgrade() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::install]
                fn install(&self) -> Result<(), SysError> {
                    todo!()
                }
                #[astrid::upgrade]
                fn upgrade(&self, prev_version: &str) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("astrid_install"),
            "Should generate astrid_install"
        );
        assert!(
            output.contains("astrid_upgrade"),
            "Should generate astrid_upgrade"
        );
    }

    #[test]
    fn mutable_on_install_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::mutable]
                #[astrid::install]
                fn install(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Mutable on install should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn mutable_on_upgrade_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::mutable]
                #[astrid::upgrade]
                fn upgrade(&self, v: &str) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Mutable on upgrade should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn stateful_install_persists_state() {
        let attr = quote::quote! { state };
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::install]
                fn install(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("astrid_install"),
            "Should generate astrid_install"
        );
        // Stateful install must persist state to KV
        assert!(
            output.contains("set_json"),
            "Stateful install should persist state via set_json, got:\n{output}"
        );
    }

    #[test]
    fn stateful_upgrade_loads_and_persists_state() {
        let attr = quote::quote! { state };
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::upgrade]
                fn upgrade(&self, prev_version: &str) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("astrid_upgrade"),
            "Should generate astrid_upgrade"
        );
        // Stateful upgrade must load existing state from KV
        assert!(
            output.contains("get_json"),
            "Stateful upgrade should load state via get_json, got:\n{output}"
        );
        // And persist it back
        assert!(
            output.contains("set_json"),
            "Stateful upgrade should persist state via set_json, got:\n{output}"
        );
    }

    #[test]
    fn stateful_both_install_and_upgrade() {
        let attr = quote::quote! { state };
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::install]
                fn install(&self) -> Result<(), SysError> {
                    todo!()
                }
                #[astrid::upgrade]
                fn upgrade(&self, prev_version: &str) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("astrid_install"),
            "Should generate astrid_install"
        );
        assert!(
            output.contains("astrid_upgrade"),
            "Should generate astrid_upgrade"
        );
        // Both must persist state
        let install_pos = output
            .find("fn astrid_install")
            .expect("astrid_install missing");
        let upgrade_pos = output
            .find("fn astrid_upgrade")
            .expect("astrid_upgrade missing");
        // set_json must appear after both export names (in their respective bodies)
        let after_install = &output[install_pos..];
        assert!(
            after_install.contains("set_json"),
            "Stateful install must call set_json"
        );
        let after_upgrade = &output[upgrade_pos..];
        assert!(
            after_upgrade.contains("set_json"),
            "Stateful upgrade must call set_json"
        );
    }

    #[test]
    fn install_then_mutable_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::install]
                #[astrid::mutable]
                fn install(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Install-then-mutable order should also produce compile_error, got:\n{output}"
        );
    }

    /// Multiple tools in one impl block — only the mutable one gets `true`.
    #[test]
    fn multi_tool_mixed_mutability() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("read_file")]
                fn read_file(&self, args: ReadArgs) -> Result<ReadResult, Error> {
                    todo!()
                }

                #[astrid::tool("write_file")]
                #[astrid::mutable]
                fn write_file(&self, args: WriteArgs) -> Result<WriteResult, Error> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();

        // Both json!(false) and json!(true) must appear — one per tool
        assert!(
            output.contains("json ! (false)"),
            "read_file should have json!(false), got:\n{output}"
        );
        assert!(
            output.contains("json ! (true)"),
            "write_file should have json!(true), got:\n{output}"
        );
    }

    // ---------------------------------------------------------------
    // #[astrid::run] tests
    // ---------------------------------------------------------------

    #[test]
    fn run_generates_export() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::run]
                fn run(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("fn run"),
            "Expected run export in Guest impl, got:\n{output}"
        );
    }

    #[test]
    fn run_stateless_uses_get_instance() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::run]
                fn run(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("get_instance"),
            "Stateless run should use get_instance(), got:\n{output}"
        );
    }

    #[test]
    fn run_stateful_loads_state() {
        let attr = quote::quote! { state };
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::run]
                fn run(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("get_json"),
            "Stateful run should load state via get_json, got:\n{output}"
        );
        // Run loops are infinite - should NOT auto-save state
        // Find the generated run method in the Guest impl.
        let run_pos = output.find("fn run ()").expect("run export missing");
        let after_run = &output[run_pos..];
        assert!(
            !after_run.contains("set_json"),
            "Stateful run should NOT auto-save state (run loops are infinite), got:\n{output}"
        );
    }

    #[test]
    fn duplicate_run_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::run]
                fn run(&self) -> Result<(), SysError> {
                    todo!()
                }
                #[astrid::run]
                fn run2(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Duplicate #[astrid::run] should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn run_with_args_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::run]
                fn run(&self, args: RunArgs) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Run with args should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn mutable_on_run_is_compile_error() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::mutable]
                #[astrid::run]
                fn run(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("compile_error"),
            "Mutable on run should produce compile_error, got:\n{output}"
        );
    }

    #[test]
    fn run_with_tools_and_install() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("search")]
                fn search(&self, args: SearchArgs) -> Result<SearchResult, Error> {
                    todo!()
                }

                #[astrid::install]
                fn install(&self) -> Result<(), SysError> {
                    todo!()
                }

                #[astrid::run]
                fn run(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("tool_execute_search"),
            "Should generate tool_execute_search interceptor arm"
        );
        assert!(
            output.contains("astrid_install"),
            "Should generate install export"
        );
        assert!(output.contains("fn run"), "Should generate run export");
    }

    /// Stateful capsule with both tools and run - verify tool dispatch calls
    /// set_json (stateful persist) but the run export does NOT.
    #[test]
    fn stateful_run_with_tools_separates_state_persistence() {
        let attr = quote::quote! { state };
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("search")]
                fn search(&self, args: SearchArgs) -> Result<SearchResult, Error> {
                    todo!()
                }

                #[astrid::run]
                fn run(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        // Tool dispatch must persist state (stateful capsule) — now via hook trigger
        let tool_pos = output
            .find("tool_execute_search")
            .expect("tool interceptor arm missing");
        let tool_section = &output[tool_pos..];
        assert!(
            tool_section.contains("set_json"),
            "Stateful tool dispatch should call set_json"
        );
        // Run export must NOT persist state (run loops are infinite)
        let run_pos = output.find("fn run ()").expect("run export missing");
        let run_section = &output[run_pos..];
        assert!(
            !run_section.contains("set_json"),
            "Stateful run should NOT call set_json even when tools exist"
        );
    }

    /// Method named something other than "run" still generates fn run() in Guest impl.
    #[test]
    fn run_with_different_method_name() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::run]
                fn event_loop(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        // The Guest export must always be named "run" regardless of method name
        assert!(
            output.contains("fn run"),
            "Should generate fn run even when method is event_loop"
        );
        // The generated body should call the user's method by its original name
        assert!(
            output.contains("event_loop"),
            "Should call user's event_loop method"
        );
    }

    #[test]
    fn doc_comment_becomes_tool_description() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                /// Read the contents of a file.
                ///
                /// Supports optional line range selection for partial reads.
                #[astrid::tool("read_file")]
                fn read_file(&self, args: ReadArgs) -> Result<String, Error> {
                    todo!()
                }
            }
        };
        let output = capsule_impl(attr, input).to_string();
        // The description should appear in the schema generation code
        assert!(
            output.contains("Read the contents of a file."),
            "Schema should contain the first line of the doc comment, got:\n{output}"
        );
        assert!(
            output.contains("Supports optional line range selection"),
            "Schema should contain the full doc comment, got:\n{output}"
        );
        assert!(
            output.contains("metadata . description"),
            "Schema should set metadata.description, got:\n{output}"
        );
    }

    #[test]
    fn tool_without_doc_comment_has_no_description() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("bare_tool")]
                fn bare_tool(&self, args: Args) -> Result<String, Error> {
                    todo!()
                }
            }
        };
        let output = capsule_impl(attr, input).to_string();
        // Should NOT contain description insertion
        assert!(
            !output.contains("metadata . description"),
            "Tool without doc comments should not set description, got:\n{output}"
        );
    }

    #[test]
    fn capsule_doc_comment_becomes_export_description() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            /// Core filesystem tools for the Astrid OS.
            ///
            /// Provides sandboxed file operations through the VFS.
            impl FsTools {
                /// Read a file.
                #[astrid::tool("read_file")]
                fn read_file(&self, args: ReadArgs) -> Result<String, Error> {
                    todo!()
                }
            }
        };
        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("Core filesystem tools"),
            "Schema export should contain capsule doc comment, got:\n{output}"
        );
        assert!(
            output.contains("sandboxed file operations"),
            "Schema export should contain full capsule description, got:\n{output}"
        );
        assert!(
            output.contains(r#""description""#),
            "Schema export should insert description key, got:\n{output}"
        );
    }

    // ---------------------------------------------------------------
    // Stateful dispatch: tool / interceptor / command
    // ---------------------------------------------------------------

    /// `&mut self` tool: generated dispatch must load state, call method, persist state.
    #[test]
    fn stateful_tool_dispatch_loads_and_saves_state() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("update")]
                fn update(&mut self, args: UpdateArgs) -> Result<UpdateResult, SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        let tool_pos = output
            .find("tool_execute_update")
            .expect("tool interceptor arm missing");
        let section = &output[tool_pos..];
        assert!(
            section.contains("get_json"),
            "Stateful tool dispatch must load state via get_json"
        );
        assert!(
            section.contains("set_json"),
            "Stateful tool dispatch must persist state via set_json"
        );
    }

    /// `&self` tool (stateless): uses `get_instance()`, no KV at all.
    #[test]
    fn stateless_tool_dispatch_uses_singleton() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("read")]
                fn read(&self, args: ReadArgs) -> Result<ReadResult, SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("get_instance"),
            "Stateless tool dispatch must use singleton via get_instance"
        );
        let tool_pos = output
            .find("tool_execute_read")
            .expect("tool interceptor arm missing");
        let section = &output[tool_pos..];
        assert!(
            !section.contains("get_json"),
            "Stateless tool dispatch must not call get_json"
        );
        assert!(
            !section.contains("set_json"),
            "Stateless tool dispatch must not call set_json"
        );
    }

    /// `&mut self` interceptor: generated dispatch must load state, call method, persist state.
    #[test]
    fn stateful_interceptor_dispatch_loads_and_saves_state() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::interceptor("handle_event")]
                fn handle_event(&mut self, payload: EventPayload) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        let pos = output
            .find("astrid_hook_trigger")
            .expect("hook export missing");
        let section = &output[pos..];
        assert!(
            section.contains("get_json"),
            "Stateful interceptor dispatch must load state via get_json"
        );
        assert!(
            section.contains("set_json"),
            "Stateful interceptor dispatch must persist state via set_json"
        );
    }

    /// `&mut self` command: generated dispatch must load state, call method, persist state.
    #[test]
    fn stateful_command_dispatch_loads_and_saves_state() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::command("reset")]
                fn reset(&mut self, payload: ResetPayload) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        // Commands are now dispatched via hook trigger as well
        let pos = output
            .find("astrid_hook_trigger")
            .expect("hook trigger export missing");
        let section = &output[pos..];
        assert!(
            section.contains("get_json"),
            "Stateful command dispatch must load state via get_json"
        );
        assert!(
            section.contains("set_json"),
            "Stateful command dispatch must persist state via set_json"
        );
    }

    /// Explicit `#[capsule(state)]` makes even `&self` tools use KV dispatch.
    #[test]
    fn explicit_state_attr_forces_stateful_dispatch() {
        let attr = quote::quote! { state };
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("query")]
                fn query(&self, args: QueryArgs) -> Result<QueryResult, SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        let pos = output
            .find("tool_execute_query")
            .expect("tool interceptor arm missing");
        let section = &output[pos..];
        assert!(
            section.contains("get_json"),
            "Explicit #[capsule(state)] must use KV load even for &self tools"
        );
        assert!(
            section.contains("set_json"),
            "Explicit #[capsule(state)] must use KV save even for &self tools"
        );
        assert!(
            !output.contains("get_instance"),
            "Explicit #[capsule(state)] must not generate get_instance singleton"
        );
    }

    #[test]
    fn capsule_without_doc_has_no_description() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl BareCapsule {
                #[astrid::tool("do_thing")]
                fn do_thing(&self, args: Args) -> Result<String, Error> {
                    todo!()
                }
            }
        };
        let output = capsule_impl(attr, input).to_string();
        // The capsule_desc should be None, so no "description" key inserted
        assert!(
            output.contains("let capsule_desc : Option < & str > = None"),
            "Capsule without doc should have None description, got:\n{output}"
        );
    }

    // ---------------------------------------------------------------
    // Component Model specific tests
    // ---------------------------------------------------------------

    #[test]
    fn generates_guest_impl_and_export_macro() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("do_thing")]
                fn do_thing(&self, args: DoArgs) -> Result<String, Error> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("impl :: astrid_sdk :: astrid_sys :: Guest for __AstridExport"),
            "Should generate impl Guest for __AstridExport, got:\n{output}"
        );
        assert!(
            output.contains(":: astrid_sdk :: astrid_sys :: export !"),
            "Should generate export!() call, got:\n{output}"
        );
        assert!(
            output.contains("__AstridExport"),
            "Should reference __AstridExport struct"
        );
    }

    #[test]
    fn no_extism_references() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("do_thing")]
                fn do_thing(&self, args: DoArgs) -> Result<String, Error> {
                    todo!()
                }
                #[astrid::install]
                fn install(&self) -> Result<(), SysError> {
                    todo!()
                }
                #[astrid::upgrade]
                fn upgrade(&self, prev_version: &str) -> Result<(), SysError> {
                    todo!()
                }
                #[astrid::run]
                fn run(&self) -> Result<(), SysError> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            !output.contains("extism_pdk"),
            "Should not contain any extism_pdk references, got:\n{output}"
        );
        assert!(
            !output.contains("no_mangle"),
            "Should not contain #[no_mangle] (Component Model uses trait impl), got:\n{output}"
        );
        assert!(
            !output.contains("extern \"C\""),
            "Should not contain extern \"C\" (Component Model uses trait impl), got:\n{output}"
        );
    }

    #[test]
    fn hook_trigger_returns_capsule_result() {
        let attr = quote::quote! {};
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::interceptor("my_hook")]
                fn handle(&self, data: HookData) -> Result<String, Error> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("CapsuleResult"),
            "astrid_hook_trigger should return CapsuleResult, got:\n{output}"
        );
        assert!(
            output.contains("\"continue\""),
            "Successful dispatch should return action: \"continue\", got:\n{output}"
        );
        assert!(
            output.contains("\"deny\""),
            "Error paths should return action: \"deny\" to halt the interceptor chain, got:\n{output}"
        );
    }

    #[test]
    fn all_four_guest_methods_always_present() {
        let attr = quote::quote! {};
        // Minimal capsule with only one tool - should still have all 4 exports
        let input = quote::quote! {
            impl MyCapsule {
                #[astrid::tool("ping")]
                fn ping(&self) -> Result<String, Error> {
                    todo!()
                }
            }
        };

        let output = capsule_impl(attr, input).to_string();
        assert!(
            output.contains("fn astrid_hook_trigger"),
            "Guest impl must have astrid_hook_trigger"
        );
        assert!(output.contains("fn run"), "Guest impl must have run");
        assert!(
            output.contains("fn astrid_install"),
            "Guest impl must have astrid_install"
        );
        assert!(
            output.contains("fn astrid_upgrade"),
            "Guest impl must have astrid_upgrade"
        );
    }
}
