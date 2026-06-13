use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, LitStr, parse_macro_input};

/// Register an async function as a shell command.
///
/// Usage:
/// ```ignore
/// #[command("ls")]
/// async fn cmd_ls(os: &dyn Kernel, args: &[String]) -> i32 {
///     // ...
/// }
/// ```
#[proc_macro_attribute]
pub fn command(attr: TokenStream, item: TokenStream) -> TokenStream {
    let name = parse_macro_input!(attr as LitStr);
    let mut func = parse_macro_input!(item as ItemFn);
    let func_ident = &func.sig.ident;

    let registration_ident = syn::Ident::new(
        &format!("__STRANDS_SHELL_CMD_{}", name.value().to_uppercase()),
        func_ident.span(),
    );

    // Make the function pub(crate) so the WASM static lookup table in
    // commands/mod.rs can reference it directly.
    func.vis = syn::Visibility::Restricted(syn::VisRestricted {
        pub_token: syn::token::Pub::default(),
        paren_token: syn::token::Paren::default(),
        in_token: None,
        path: Box::new(syn::parse_quote!(crate)),
    });

    let expanded = quote! {
        #func

        #[cfg(not(target_arch = "wasm32"))]
        ::inventory::submit! {
            crate::commands::CommandEntry {
                name: #name,
                func: |os, args| Box::pin(#func_ident(os, args)),
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        #[used]
        static #registration_ident: () = ();
    };

    expanded.into()
}
