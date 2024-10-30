use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{Error, ItemFn, Signature};

use crate::args::Args;

pub fn expand_test_function(signature: &Signature) -> TokenStream {
    let fn_name_ident = &signature.ident;

    let fn_variance = match signature.inputs.len() {
        0 => Ident::new("Normal", signature.span()),
        1 => Ident::new("MPBarrier", signature.span()),
        _ => {
            return TokenStream::from(
                Error::new_spanned(signature, "Invalid function arguments").to_compile_error(),
            )
        }
    };

    quote! {
        testing::description::KernelTestFunction::#fn_variance (#fn_name_ident)
    }
}

pub fn expand(args: Args, test_fn: ItemFn) -> TokenStream {
    let fn_to_test = expand_test_function(&test_fn.sig);
    let fn_name = test_fn.sig.ident.to_string();

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
    let multiprocessor = args.multiprocessor;
    let allow_heap_leak = args.allow_heap_leak;
    let allow_page_leak = args.allow_page_leak;
    let allow_frame_leak = args.allow_frame_leak;

    quote! {
        #test_fn

        #[linkme::distributed_slice(testing::description::KERNEL_TESTS)]
        static #description_name: testing::description::KernelTestDescription = testing::description::KernelTestDescription {
            name: #name,
            fn_name: #fn_name,
            expected_exit: #expected_exit,
            test_fn: #fn_to_test,
            test_location: #test_location,
            focus: #focus,
            ignore: #ignore,
            multiprocessor: #multiprocessor,
            allow_heap_leak: #allow_heap_leak,
            allow_frame_leak: #allow_frame_leak,
            allow_page_leak: #allow_page_leak,
        };
    }
}
