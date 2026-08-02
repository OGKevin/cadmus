//! Proc macros for Cadmus.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ItemFn, LitStr, Token, parse_macro_input};

struct LeaseArgs {
    target: Expr,
    name: LitStr,
    try_acquire: bool,
}

impl Parse for LeaseArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let target: Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let name: LitStr = input.parse()?;
        let try_acquire = if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let ident: syn::Ident = input.parse()?;
            if ident != "try" {
                return Err(syn::Error::new(
                    ident.span(),
                    "expected `try` after lease name",
                ));
            }
            true
        } else {
            false
        };
        Ok(Self {
            target,
            name,
            try_acquire,
        })
    }
}

/// Holds a named lease for the duration of the annotated function.
///
/// ```ignore
/// #[lease(tracker, "time-sync")]
/// fn work(tracker: &LeaseTracker) {
///     // `tracker.acquire("time-sync")` held until return
/// }
///
/// #[lease(self.wifi_session, "ota-download", try)]
/// fn download(&self) -> Result<(), Error> {
///     // `self.wifi_session.acquire("ota-download")?` held until return
/// }
/// ```
///
/// The first argument is any expression with an `acquire` method. Pass `try` as
/// a third argument when `acquire` returns [`Result`] and the function returns
/// `Result` so the expansion can use `?`.
#[proc_macro_attribute]
pub fn lease(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as LeaseArgs);
    let mut input_fn = parse_macro_input!(item as ItemFn);

    let target = &args.target;
    let name = &args.name;
    let acquire = if args.try_acquire {
        quote! {
            let _lease = (#target).acquire(#name)?;
        }
    } else {
        quote! {
            let _lease = (#target).acquire(#name);
        }
    };

    let original_block = *input_fn.block;
    input_fn.block = syn::parse_quote!({
        #acquire
        #original_block
    });

    TokenStream::from(quote! { #input_fn })
}
