//! Proc macros for Cadmus.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ItemFn, LitStr, Token, parse_macro_input};

enum AcquireMode {
    /// `acquire` returns the lease value directly.
    Direct,
    /// `acquire` returns `Result`; propagate with `?` (function must return `Result`).
    Try,
    /// `acquire` returns `Result`; log and `return` on `Err` (function must return `()`).
    OrReturn { message: Option<LitStr> },
}

struct LeaseArgs {
    target: Expr,
    name: LitStr,
    mode: AcquireMode,
}

impl Parse for LeaseArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let target: Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let name: LitStr = input.parse()?;
        let mode = if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let ident: syn::Ident = input.parse()?;
            if ident == "try" {
                AcquireMode::Try
            } else if ident == "or_return" {
                let message = if input.peek(syn::token::Paren) {
                    let content;
                    syn::parenthesized!(content in input);
                    Some(content.parse::<LitStr>()?)
                } else {
                    None
                };
                AcquireMode::OrReturn { message }
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    "expected `try` or `or_return` after lease name",
                ));
            }
        } else {
            AcquireMode::Direct
        };
        Ok(Self { target, name, mode })
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
///
/// #[lease(self.wifi_session, "time-sync", or_return)]
/// fn run(&mut self) {
///     // on `Err`: log and return early
/// }
///
/// #[lease(self.wifi_session, "time-sync", or_return("failed to acquire WiFi lease for time sync"))]
/// fn run(&mut self) {
///     // custom error message
/// }
/// ```
///
/// The first argument is any expression with an `acquire` method.
///
/// - No third argument — `acquire` returns the lease directly.
/// - `try` — `acquire` returns [`Result`] and the function returns [`Result`];
///   uses `?`.
/// - `or_return` — `acquire` returns [`Result`] and the function returns `()`;
///   logs with `tracing::error!` and returns on `Err`. Pass an optional string
///   for a custom message (default: `"failed to acquire lease"`).
#[proc_macro_attribute]
pub fn lease(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as LeaseArgs);
    let mut input_fn = parse_macro_input!(item as ItemFn);

    let target = &args.target;
    let name = &args.name;
    let acquire = match &args.mode {
        AcquireMode::Try => quote! {
            let _lease = (#target).acquire(#name)?;
        },
        AcquireMode::OrReturn { message } => {
            let message = message
                .as_ref()
                .map(|m| m.value())
                .unwrap_or_else(|| "failed to acquire lease".to_string());
            quote! {
                let _lease = match (#target).acquire(#name) {
                    ::core::result::Result::Ok(lease) => lease,
                    ::core::result::Result::Err(error) => {
                        ::tracing::error!(error = %error, name = #name, #message);
                        return;
                    }
                };
            }
        }
        AcquireMode::Direct => quote! {
            let _lease = (#target).acquire(#name);
        },
    };

    let original_block = *input_fn.block;
    input_fn.block = syn::parse_quote!({
        #acquire
        #original_block
    });

    TokenStream::from(quote! { #input_fn })
}
