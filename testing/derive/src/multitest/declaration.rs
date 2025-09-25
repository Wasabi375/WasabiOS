use proc_macro::{Diagnostic, Level};
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Attribute, Error, Expr, Ident, Item, ItemFn, ItemMod, ItemUse, Result, token::Brace};

use crate::kernel_test::args::{TestArgs, TestFunctionVariance};

use super::args::MultiTestArgs;

pub fn expand_multitest(args: MultiTestArgs, module: ItemMod) -> Result<TokenStream> {
    let rust_tests = expand_rust_tests(&args, &module)?;
    let kernel_tests = expand_kernel_tests(&args, &module)?;

    Ok(quote! {
        #rust_tests
        #kernel_tests
    })
}

fn expand_rust_tests(_args: &MultiTestArgs, module: &ItemMod) -> Result<TokenStream> {
    let cfg_attr = quote! { #[cfg(test)] };

    let attrs = &module.attrs;
    let vis = &module.vis;
    let unsafety = &module.unsafety;
    let content = expand_content_rust_test(module.content.as_ref())?;
    let semi = &module.semi;

    let ident = Ident::new(&format!("__{}__rust_test", module.ident), Span::call_site());

    Ok(quote! {
        #cfg_attr
        #(#attrs)*
        #vis #unsafety mod #ident {
            #(#content)*
        }#semi
    })
}

fn expand_kernel_tests(args: &MultiTestArgs, module: &ItemMod) -> Result<TokenStream> {
    let cfg_attr = expand_cfg_attribute_rust_test(args.cfg.as_ref());
    let attrs = &module.attrs;
    let vis = &module.vis;
    let unsafety = &module.unsafety;
    let ident = &module.ident;
    let content = module
        .content
        .as_ref()
        .map(|(_, items)| items.as_slice())
        .unwrap_or(&[]);
    let semi = &module.semi;

    Ok(quote! {
       #cfg_attr
       #(#attrs)*
       #vis #unsafety mod #ident {
           #(#content)*
       }#semi
    })
}

fn expand_cfg_attribute_rust_test(attr: Option<&Expr>) -> TokenStream {
    if let Some(attr) = attr {
        quote! {
            #[cfg(#attr)]
        }
    } else {
        quote! {}
    }
}

fn expand_content_rust_test(content: Option<&(Brace, Vec<Item>)>) -> Result<Vec<TokenStream>> {
    let res = if let Some((_, content)) = content {
        content
            .iter()
            .map(|content| match content {
                Item::Fn(fun) => expand_function_rust_test(fun),
                Item::Use(use_) => expand_use_rust_test(use_),
                _ => Ok(quote! {
                    #content
                }),
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        vec![]
    };
    Ok(res)
}

fn expand_use_rust_test(use_: &ItemUse) -> Result<TokenStream> {
    Ok(quote! {
        #[allow(unused_imports)]
        #use_
    })
}

fn expand_function_rust_test(fun: &ItemFn) -> Result<TokenStream> {
    if fun.attrs.is_empty() {
        return Ok(expand_normal_function(fun));
    }

    let mut test_args = None;
    let mut attrs = Vec::with_capacity(fun.attrs.len());

    for attr in fun.attrs.iter() {
        if let Some(args) = parse_kernel_test_attribute(attr)? {
            if test_args.is_some() {
                return Err(Error::new_spanned(
                    attr,
                    "kernel_test can only be specified once",
                ));
            }
            test_args = Some(args);
        } else {
            attrs.push(attr);
        }
    }

    let Some(test_args) = test_args else {
        return Ok(expand_normal_function(fun));
    };

    let attrs: &[&Attribute] = &attrs;
    let test_args: &TestArgs = &test_args;
    let to_test_fn_def = {
        let inputs = &fun.sig.inputs;
        let output = &fun.sig.output;
        let block = &fun.block;

        quote! {
            #(#attrs)*
            fn test_fn (#inputs) #output
            #block
        }
    };

    let fn_name = &fun.sig.ident;

    if test_args.multiprocessor {
        let fn_name = Ident::new(&format!("{fn_name}__multiprocessor"), Span::call_site());
        return Ok(quote! {
            #[test]
            #[ignore]
            fn #fn_name() {
                // TODO
                // multiprocessor test not supported by rust-test
            }
        });
    }

    let test_fn_variance = TestFunctionVariance::try_from(&fun.sig)?;
    if !matches!(test_fn_variance, TestFunctionVariance::Normal) {
        return Err(Error::new_spanned(
            &fun.sig,
            "Single-thread test must use normal KernelTestFunction",
        ));
    }

    let should_panic_attr = if test_args.is_panicing() {
        quote! { #[should_panic] }
    } else {
        quote! {}
    };

    let ignore_attr = if test_args.ignore {
        quote! { #[ignore] }
    } else {
        quote! {}
    };

    let expected_result = test_args
        .expected_exit
        .as_ref()
        .map(|exp| quote! { #exp })
        .unwrap_or(quote! { testing::description::TestExitState::Succeed});

    Ok(quote! {
        #[test]
        #should_panic_attr
        #ignore_attr
        #(#attrs)*
        fn #fn_name() {
            #to_test_fn_def

            unsafe {
                use host_shared::sync::StdInterruptState as _;
                use shared::sync::CoreInfo as _;
                // Safety: host_shared::sync will alwys provide the same values
                testing::multiprocessor::init_interrupt_state(
                    &host_shared::sync::STD_INTERRUPT_STATE,
                    host_shared::sync::StdInterruptState::max_core_count()
                );
            }

            host_shared::test_utils::init_test_logger();

            let result = test_fn();

            let expected_result = #expected_result;

            match expected_result {
                testing::description::TestExitState::Succeed => result.expect("Test failed with t_assert!"),
                testing::description::TestExitState::Error(expected) => {
                    assert!(result.is_err(), "Expected test to fail with {:?}", expected);

                    if let Some(expected) = expected {
                        assert_eq!(result.unwrap_err(), expected, "Expected test to fail with {:?}", expected);
                    }
                },
                testing::description::TestExitState::Panic => { /* handled by [should_panic] */ }
            }
        }
    })
}

fn parse_kernel_test_attribute(attr: &Attribute) -> Result<Option<TestArgs>> {
    let Some(last_path_seg) = attr.path().segments.last().map(|seg| &seg.ident) else {
        return Ok(None);
    };

    // TODO can I make this more robust? Maybe I can get the required path in the multitest args
    if *last_path_seg != "kernel_test" {
        return Ok(None);
    }

    match attr.meta {
        syn::Meta::Path(_) => Ok(Some(TestArgs::default())),
        syn::Meta::List(_) | syn::Meta::NameValue(_) => attr.parse_args().map(Some),
    }
}

fn expand_normal_function(fun: &ItemFn) -> TokenStream {
    quote! {
        #fun
    }
}
