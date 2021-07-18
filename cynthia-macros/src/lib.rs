#![forbid(unsafe_code, future_incompatible, rust_2018_idioms)]
#![deny(missing_debug_implementations, nonstandard_style)]
#![recursion_limit = "512"]

use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;

#[cfg(not(test))]
#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::ItemFn);
    let ret = &input.sig.output;
    let inputs = &input.sig.inputs;
    let name = &input.sig.ident;
    let body = &input.block;
    let attrs = &input.attrs;
    let vis = &input.vis;

    if name != "main" {
        return TokenStream::from(quote_spanned! { name.span() =>
            compile_error!("only the main function can be tagged with #[cynthia::main]"),
        });
    }

    if input.sig.asyncness.is_none() {
        return TokenStream::from(quote_spanned! { input.span() =>
            compile_error!("the async keyword is missing from the function declaration"),
        });
    }

    let result = quote! {
        #vis fn main() #ret {
            #(#attrs)*
            async fn main(#inputs) #ret {
                #body
            }

            cynthia::runtime::block_on(async {
                main().await
            })
        }

    };

    result.into()
}

#[proc_macro_attribute]
pub fn test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::ItemFn);
    let ret = &input.sig.output;
    let name = &input.sig.ident;
    let body = &input.block;
    let attrs = &input.attrs;
    let vis = &input.vis;

    if input.sig.asyncness.is_none() {
        return TokenStream::from(quote_spanned! { input.span() =>
            compile_error!("the async keyword is missing from the function declaration"),
        });
    }

    let result = quote! {
        #[::core::prelude::v1::test]
        #(#attrs)*
        #vis fn #name() #ret {
            cynthia::runtime::block_on(async { #body })
        }
    };

    result.into()
}

#[proc_macro_attribute]
pub fn bench(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::ItemFn);
    let ret = &input.sig.output;
    let args = &input.sig.inputs;
    let name = &input.sig.ident;
    let body = &input.block;
    let attrs = &input.attrs;
    let vis = &input.vis;

    if input.sig.asyncness.is_none() {
        return TokenStream::from(quote_spanned! { input.span() =>
            compile_error!("the async keyword is missing from the function declaration"),
        });
    }

    if !args.is_empty() {
        return TokenStream::from(quote_spanned! { args.span() =>
            compile_error!("async benchmarks don't take any arguments"),
        });
    }

    let result = quote! {
        #[::core::prelude::v1::bench]
        #(#attrs)*
        #vis fn #name(b: &mut test::Bencher) #ret {
            cynthia::runtime::block_on(async {
                #body
            })
        }
    };

    result.into()
}
