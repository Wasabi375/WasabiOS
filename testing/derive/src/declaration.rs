use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ItemFn;

use crate::args::Args;

pub fn expand(args: Args, test_fn: ItemFn) -> TokenStream {
    let fn_name_ident = &test_fn.sig.ident;
    let fn_name = fn_name_ident.to_string();

    let description_name = format_ident!("__KERNEL_TEST_{}", fn_name.to_uppercase());

    let name = args
        .name
        .map_or_else(|| quote! { #fn_name }, |n| quote! { #n });

    let expected_exit = args.expected_exit.map_or_else(
        || quote! { testing::description::TestExitState::Succeed },
        |exit| quote! { #exit },
    );

    let test_location = quote! {
        testing::description::SourceLocation {
            module: module_path!(),
            file: file!(),
            line: line!(),
            column: column!(),
        }
    };

    let focus = args.focus;
    let ignore = args.ignore;

    quote! {
        #test_fn

        #[linkme::distributed_slice(testing::description::KERNEL_TESTS)]
        static #description_name: testing::description::KernelTestDescription = testing::description::KernelTestDescription {
            name: #name,
            fn_name: #fn_name,
            expected_exit: #expected_exit,
            test_fn: #fn_name_ident,
            test_location: #test_location,
            focus: #focus,
            ignore: #ignore
        };
    }
}
