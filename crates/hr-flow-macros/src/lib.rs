//! Proc-macros for hr-flow.
//!
//! Currently exposes `#[flow_action]` to register a Rust async function as a
//! custom flow action. The macro keeps the original function callable and
//! emits a sibling `register_<name>` fn that hr-flow can use to wire the
//! action into a `FlowEngine`'s registry. Input/output capture, duration
//! measurement and error wrapping are handled by the engine itself when it
//! invokes the registered closure — the macro only concerns itself with
//! signature normalisation.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, ItemFn, LitStr, Meta, MetaNameValue, Expr, ExprLit, Lit};

#[proc_macro_attribute]
pub fn flow_action(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let attr_args = parse_macro_input!(attr with syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated);

    let mut name: Option<String> = None;
    for meta in attr_args {
        if let Meta::NameValue(MetaNameValue { path, value, .. }) = meta {
            if path.is_ident("name") {
                if let Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) = value {
                    name = Some(s.value());
                }
            }
        }
    }

    let fn_ident = &input.sig.ident;
    let action_name = name.unwrap_or_else(|| fn_ident.to_string());
    let action_name_lit = LitStr::new(&action_name, fn_ident.span());
    let register_ident = format_ident!("register_{}", fn_ident);

    let expanded = quote! {
        #input

        pub fn #register_ident(builder: &mut ::hr_flow::engine::FlowEngineBuilder) {
            builder.register_action(#action_name_lit, #fn_ident);
        }
    };

    expanded.into()
}
