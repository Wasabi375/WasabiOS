use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{Error, ItemFn, Result, Signature};

use crate::kernel_test::args::TestFunctionVariance;

use super::args::{CrateList, TestArgs};

fn crate_name() -> String {
    std::env::var("CARGO_CRATE_NAME").unwrap()
}

fn expand_test_function(signature: &Signature) -> Result<TokenStream> {
    let fn_name_ident = &signature.ident;

    let fn_variance = TestFunctionVariance::try_from(signature)?.name();
    let fn_variance = Ident::new(fn_variance, signature.span());

    Ok(quote! {
        testing::description::KernelTestFunction::#fn_variance (#fn_name_ident)
    })
}

pub fn expand_test(args: TestArgs, test_fn: ItemFn) -> Result<TokenStream> {
    let fn_to_test = expand_test_function(&test_fn.sig)?;
    let fn_name = test_fn.sig.ident.to_string();

    let description_name = format_ident!("__KERNEL_TEST_{}", fn_name.to_uppercase());

    let name = args
        .name
        .map_or_else(|| quote! { #fn_name }, |n| quote! { #n });

    let expected_exit = args.expected_exit.map_or_else(
        || quote! { testing::description::TestExitState::Succeed },
        |exit| quote! { #exit },
    );

    let crate_name = crate_name();
    let test_location = quote! {
        testing::description::SourceLocation {
            crate_name: #crate_name,
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
    let allow_mapping_leak = args.allow_mapping_leak;

    let dist_slice_ref = {
        let name = test_slice_name(&crate_name);
        quote! {
            crate::__testing::#name
        }
    };

    Ok(quote! {
        #test_fn

        #[linkme::distributed_slice(#dist_slice_ref)]
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
            allow_mapping_leak: #allow_mapping_leak,
        };
    })
}

fn test_slice_name(crate_name: &str) -> Ident {
    assert!(!crate_name.is_empty());

    let crate_name = crate_name.to_uppercase();

    format_ident!("KERNEL_TEST_{}", crate_name)
}

pub fn expand_slice() -> TokenStream {
    let slice_ident = test_slice_name(&crate_name());

    quote! {

       #[doc(hidden)]
       pub mod __testing {
           #[doc(hidden)]
           #[linkme::distributed_slice]
           pub static #slice_ident: [testing::description::KernelTestDescription] = [..];
       }
    }
}

pub fn expand_get_tests(crates: CrateList) -> TokenStream {
    let slices = crates.0.iter().map(|c| {
        let name = c.to_string();
        let slice_name = test_slice_name(&name);
        let crate_name = if name == crate_name() { "crate" } else { &name };
        let crate_name = format_ident!("{}", crate_name);
        quote! {
            #crate_name::__testing::#slice_name
        }
    });

    slices.fold(quote!(core::iter::empty()), |acc, slice| {
        quote! {
            core::iter::Iterator::chain(#acc, #slice)
        }
    })
}
