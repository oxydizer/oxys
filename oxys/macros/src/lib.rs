//! The `#[oxys::config]` attribute for Oxys config files.
//!
//! Applied to the config function, it does two things:
//!
//! 1. Rewrites every struct literal in the body that omits fields to carry a
//!    trailing `..Default::default()`, so configs state only what differs
//!    from the defaults.
//! 2. Emits the program entry point (`fn main`) that runs the config and
//!    writes `manifest.toml` — the job `oxys::main!(config);` used to do.
//!
//! Struct literals that already carry a `..rest` spread are left untouched,
//! so old-style bodies keep compiling under the attribute. Enum struct
//! variants (`LoginFrontend::OxysLogin { .. }`) are skipped because Rust
//! forbids functional-update syntax on them; they must stay fully spelled.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::{
    Expr, ExprStruct, ItemFn, Macro, Token, parse_quote, punctuated::Punctuated,
    visit_mut::VisitMut,
};

#[proc_macro_attribute]
pub fn config(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

fn expand(attr: TokenStream2, item: TokenStream2) -> Result<TokenStream2, syn::Error> {
    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(
            attr,
            "#[oxys::config] takes no arguments",
        ));
    }
    let mut function: ItemFn = syn::parse2(item).map_err(|err| {
        syn::Error::new(
            err.span(),
            "#[oxys::config] must be applied to the config function",
        )
    })?;

    AutoDefault.visit_item_fn_mut(&mut function);

    let name = &function.sig.ident;
    Ok(quote! {
        #function

        fn main() {
            ::oxys::run_config(#name);
        }
    })
}

struct AutoDefault;

impl VisitMut for AutoDefault {
    fn visit_expr_struct_mut(&mut self, expr: &mut ExprStruct) {
        // Rewrite nested literals in field values first.
        syn::visit_mut::visit_expr_struct_mut(self, expr);

        if expr.rest.is_some() || is_enum_variant_path(expr) {
            return;
        }
        expr.dot2_token = Some(Default::default());
        expr.rest = Some(Box::new(parse_quote!(::core::default::Default::default())));
    }

    fn visit_macro_mut(&mut self, mac: &mut Macro) {
        // syn does not descend into macro tokens; handle the one macro where
        // struct literals routinely appear (`vec![Subvolume { .. }, ...]`).
        if !mac.path.is_ident("vec") {
            return;
        }
        let Ok(mut elements) = mac.parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated)
        else {
            // e.g. `vec![expr; n]` or anything else we don't understand:
            // leave the tokens exactly as written.
            return;
        };
        for element in elements.iter_mut() {
            self.visit_expr_mut(element);
        }
        mac.tokens = elements.to_token_stream();
    }
}

/// Heuristic for enum struct variants like `LoginFrontend::OxysLogin { .. }`:
/// with two or more path segments, an uppercase-initial second-to-last segment
/// reads as a type name, making the last segment a variant. Plain structs
/// reached through modules (`manifest::Compiler { .. }`) keep lowercase module
/// segments and are still rewritten.
fn is_enum_variant_path(expr: &ExprStruct) -> bool {
    let segments = &expr.path.segments;
    if segments.len() < 2 {
        return false;
    }
    segments[segments.len() - 2]
        .ident
        .to_string()
        .chars()
        .next()
        .is_some_and(|first| first.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand_ok(item: TokenStream2) -> String {
        expand(TokenStream2::new(), item)
            .expect("expansion should succeed")
            .to_string()
    }

    #[test]
    fn sparse_literal_gains_default_spread() {
        let out = expand_ok(quote! {
            pub fn config() -> Oxys {
                Oxys { os: Os { hostname: "x".into() } }
            }
        });
        assert_eq!(
            out.matches(":: core :: default :: Default :: default ()")
                .count(),
            2
        );
    }

    #[test]
    fn existing_spread_is_untouched() {
        let out = expand_ok(quote! {
            pub fn config() -> Oxys {
                Oxys { os: base_os(), ..Default::default() }
            }
        });
        assert!(out.contains(".. Default :: default ()"));
        assert!(!out.contains(":: core :: default :: Default :: default ()"));
    }

    #[test]
    fn enum_variant_literal_is_skipped() {
        let out = expand_ok(quote! {
            fn config() -> Oxys {
                Oxys { login: LoginFrontend::OxysLogin { tty: 1, fallback_tty_login: true } }
            }
        });
        // Only the outer Oxys literal is rewritten, not the enum variant.
        assert_eq!(
            out.matches(":: core :: default :: Default :: default ()")
                .count(),
            1
        );
        assert!(out.contains("LoginFrontend :: OxysLogin { tty : 1 , fallback_tty_login : true }"));
    }

    #[test]
    fn vec_macro_elements_are_rewritten() {
        let out = expand_ok(quote! {
            fn config() -> Oxys {
                Oxys { subvolumes: vec![Subvolume { name: "@".into() }, Subvolume { name: "@home".into() }] }
            }
        });
        // Oxys + both Subvolume literals.
        assert_eq!(
            out.matches(":: core :: default :: Default :: default ()")
                .count(),
            3
        );
    }

    #[test]
    fn vec_repeat_form_is_left_alone() {
        let out = expand_ok(quote! {
            fn config() -> Oxys {
                Oxys { counts: vec![0u8; 4] }
            }
        });
        assert!(out.contains("vec ! [0u8 ; 4]"));
    }

    #[test]
    fn entry_point_calls_run_config() {
        let out = expand_ok(quote! {
            pub fn config() -> Oxys { Oxys {} }
        });
        assert!(out.contains("fn main () { :: oxys :: run_config (config) ; }"));
    }

    #[test]
    fn attribute_arguments_are_rejected() {
        let err = expand(
            quote!(some_arg),
            quote!(
                fn config() -> Oxys {
                    Oxys {}
                }
            ),
        )
        .expect_err("arguments should be rejected");
        assert!(err.to_string().contains("takes no arguments"));
    }

    #[test]
    fn non_function_items_are_rejected() {
        let err = expand(
            TokenStream2::new(),
            quote!(
                struct NotAFn;
            ),
        )
        .expect_err("non-fn should be rejected");
        assert!(err.to_string().contains("config function"));
    }
}
