//! Proc-macros for hr-flow.
//!
//! `#[flow_action]` keeps the original async fn callable and emits **two**
//! sibling functions:
//!
//! * `register_<name>(&mut FlowEngineBuilder)` — wires the action into the
//!   in-process engine. Used by apps still in `EmbeddedEngine` mode (Phase ≤ 4
//!   for Wallet) and by the daemon-side helper `hr-flow-daemon`.
//!
//! * `mount_<name>(CallbackRouter<S>) -> CallbackRouter<S>` — exposes the same
//!   action over HTTP as `POST /_flow/action/<name>`. Used by apps in
//!   callback mode (Phase 4 onward, all stacks).
//!
//! Both call into the *same* Rust function — a single annotated `async fn`
//! is enough to be runnable embedded AND remote in parallel during the
//! Wallet pilot transition.
//!
//! Note: emitting `mount_<name>` references `::hr_flow_callback::CallbackRouter`
//! by absolute path. Apps that don't yet depend on `hr-flow-callback` simply
//! never call `mount_<name>` and the unused fn never triggers a resolution
//! error (Rust resolves item paths lazily for unused fns).

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Expr, ExprLit, ItemFn, Lit, LitStr, Meta, MetaNameValue};

#[proc_macro_attribute]
pub fn flow_action(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let attr_args = parse_macro_input!(attr with syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated);

    let mut name: Option<String> = None;
    for meta in attr_args {
        if let Meta::NameValue(MetaNameValue { path, value, .. }) = meta {
            if path.is_ident("name") {
                if let Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) = value
                {
                    name = Some(s.value());
                }
            }
        }
    }

    let fn_ident = &input.sig.ident;
    let action_name = name.unwrap_or_else(|| fn_ident.to_string());
    let action_name_lit = LitStr::new(&action_name, fn_ident.span());
    let register_ident = format_ident!("register_{}", fn_ident);
    let mount_ident = format_ident!("mount_{}", fn_ident);

    let expanded = quote! {
        #input

        pub fn #register_ident(builder: &mut ::hr_flow::engine::FlowEngineBuilder) {
            builder.register_action(#action_name_lit, #fn_ident);
        }

        /// Mount this action on a `hr-flow-callback` router so the daemon can
        /// invoke it over HTTP (Phase 4+ callback mode).
        pub fn #mount_ident(
            router: ::hr_flow_callback::CallbackRouter,
        ) -> ::hr_flow_callback::CallbackRouter {
            router.with_action_fn(#action_name_lit, #fn_ident)
        }
    };

    expanded.into()
}
