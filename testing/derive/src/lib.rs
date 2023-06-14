extern crate proc_macro;

use args::Args;
use proc_macro::TokenStream;
use syn::parse_macro_input;

mod args;
mod declaration;

#[proc_macro_attribute]
pub fn kernel_test(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attribute as Args);

    let expanded = declaration::expand(args, parse_macro_input!(item));

    TokenStream::from(expanded)
}
