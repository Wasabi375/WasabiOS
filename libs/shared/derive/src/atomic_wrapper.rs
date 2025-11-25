use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::{DataStruct, DeriveInput, Field, Ident, parse_macro_input, spanned::Spanned};

const PRIMITIVE_TYPES: &[&str] = &[
    "bool", "u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "usize", "isize",
];
const ATOMIC_TYPES: &[&str] = &[
    "AtomicBool",
    "AtomicU8",
    "AtomicI8",
    "AtomicU16",
    "AtomicI16",
    "AtomicU32",
    "AtomicI32",
    "AtomicU64",
    "AtomicI64",
    "AtomicUsize",
    "AtomicIsize",
];

pub fn derive_atomic_wrapper(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input: DeriveInput = parse_macro_input!(input);

    let (_primitive, atomic_prim, from_prim, to_prim) = extract_wrapper_info(&input);

    let vis = &input.vis;

    let atomic_wrapper = format_ident!("Atomic{}", input.ident);
    let wrapper_ident = &input.ident;

    let v_to_prim = to_prim(&Ident::new("v", Span::call_site()));
    let val_to_prim = to_prim(&Ident::new("val", Span::call_site()));
    let current_to_prim = to_prim(&Ident::new("current", Span::call_site()));
    let new_to_prim = to_prim(&Ident::new("new", Span::call_site()));
    let from_prim_v = from_prim(&Ident::new("v", Span::call_site()));
    let from_prim_prim = from_prim(&Ident::new("prim", Span::call_site()));

    let wrapper_doc = format!(
        "Wrapper around [{}] that converts from/to [{}]\n\nSee [{}] for function documentation",
        atomic_prim, wrapper_ident, atomic_prim
    );

    let ordering = quote! { core::sync::atomic::Ordering };

    quote_spanned! { Span::call_site() =>
        #[doc = #wrapper_doc]
        #[repr(transparent)]
        #vis struct #atomic_wrapper(#atomic_prim);

        #[allow(missing_docs)]
        impl #atomic_wrapper {
            #vis const fn into_inner(self) -> #atomic_prim {
                self.0
            }

            #vis const fn from_primitive(inner: #atomic_prim) -> #atomic_wrapper {
                #atomic_wrapper(inner)
            }

            #vis const fn new(v: #wrapper_ident) -> #atomic_wrapper {
                #atomic_wrapper(#atomic_prim::new(#v_to_prim))
            }

            #vis fn load(&self, order: #ordering) -> #wrapper_ident {
                let v = self.0.load(order);
                #from_prim_v
            }

            #vis fn store(&self, val: #wrapper_ident, order: #ordering) {
                self.0.store(#val_to_prim, order)
            }

            #vis fn swap(&self, val: #wrapper_ident, order: #ordering) -> #wrapper_ident {
                let v = self.0.swap(#val_to_prim, order);
                #from_prim_v
            }

            #vis fn compare_exchange(
                &self,
                current: #wrapper_ident,
                new: #wrapper_ident,
                success: #ordering,
                failure: #ordering
            ) -> Result<#wrapper_ident, #wrapper_ident> {
                let current = #current_to_prim;
                let new = #new_to_prim;

                match self.0.compare_exchange(current, new, success, failure) {
                    Ok(v) => Ok(#from_prim_v),
                    Err(v) => Err(#from_prim_v),
                }
            }

            #vis fn compare_exchange_weak(
                &self,
                current: #wrapper_ident,
                new: #wrapper_ident,
                success: #ordering,
                failure: #ordering
            ) -> Result<#wrapper_ident, #wrapper_ident> {
                let current = #current_to_prim;
                let new = #new_to_prim;

                match self.0.compare_exchange_weak(current, new, success, failure) {
                    Ok(v) => Ok(#from_prim_v),
                    Err(v) => Err(#from_prim_v),
                }
            }

            #vis fn fetch_upate<F>(
                &self,
                set_order: #ordering,
                fetch_order: #ordering,
                mut f: F,
            ) -> Result<#wrapper_ident, #wrapper_ident>
            where
                F: FnMut(#wrapper_ident) -> Option<#wrapper_ident>,
            {
                match self.0.fetch_update(
                    set_order, fetch_order,
                    |prim| {
                        f(#from_prim_prim)
                            .map(|v| #v_to_prim)
                    }
                ) {
                    Ok(v) => Ok(#from_prim_v),
                    Err(v) => Err(#from_prim_v),
                }
            }
        }

        #[allow(missing_docs)]
        impl #atomic_wrapper
        where
            for<'z> #wrapper_ident: core::ops::Add<#wrapper_ident, Output = #wrapper_ident>
        {
            #vis fn fetch_add(&self, val: #wrapper_ident, order: #ordering) -> #wrapper_ident {
                let v = self.0.fetch_add(#val_to_prim, order);
                #from_prim_v
            }
        }

        #[allow(missing_docs)]
        impl #atomic_wrapper
        where
            for<'z> #wrapper_ident: core::ops::Sub<#wrapper_ident, Output = #wrapper_ident>
        {
            #vis fn fetch_sub(&self, val: #wrapper_ident, order: #ordering) -> #wrapper_ident {
                let v = self.0.fetch_sub(#val_to_prim, order);
                #from_prim_v
            }
        }

        #[allow(missing_docs)]
        impl #atomic_wrapper
        where
            for<'z> #wrapper_ident: core::ops::BitAnd<#wrapper_ident, Output = #wrapper_ident>
        {
            #vis fn fetch_and(&self, val: #wrapper_ident, order: #ordering) -> #wrapper_ident {
                let v = self.0.fetch_and(#val_to_prim, order);
                #from_prim_v
            }
        }

        #[allow(missing_docs)]
        impl #atomic_wrapper
        where
            for<'z> #wrapper_ident: core::ops::BitAnd<#wrapper_ident, Output = #wrapper_ident>,
            for<'z> #wrapper_ident: core::ops::Not<Output = #wrapper_ident>,
        {
            #vis fn fetch_nand(&self, val: #wrapper_ident, order: #ordering) -> #wrapper_ident {
                let v = self.0.fetch_nand(#val_to_prim, order);
                #from_prim_v
            }
        }

        #[allow(missing_docs)]
        impl #atomic_wrapper
        where
            for<'z> #wrapper_ident: core::ops::BitOr<#wrapper_ident, Output = #wrapper_ident>
        {
            #vis fn fetch_or(&self, val: #wrapper_ident, order: #ordering) -> #wrapper_ident {
                let v = self.0.fetch_or(#val_to_prim, order);
                #from_prim_v
            }
        }

        #[allow(missing_docs)]
        impl #atomic_wrapper
        where
            for<'z> #wrapper_ident: core::ops::BitXor<#wrapper_ident, Output = #wrapper_ident>
        {
            #vis fn fetch_xor(&self, val: #wrapper_ident, order: #ordering) -> #wrapper_ident {
                let v = self.0.fetch_xor(#val_to_prim, order);
                #from_prim_v
            }
        }

    }
    .into()
}

fn extract_wrapper_info(
    input: &DeriveInput,
) -> (
    Ident,
    TokenStream,
    Box<dyn Fn(&Ident) -> TokenStream>,
    Box<dyn Fn(&Ident) -> TokenStream>,
) {
    match &input.data {
        syn::Data::Struct(data_struct) => {
            let field = get_field(data_struct);
            let Some(prim_index) = PRIMITIVE_TYPES
                .iter()
                .position(|p| *p == format!("{}", field.ty.to_token_stream()).as_str())
            else {
                panic!(
                    "Inner type must be a primitive, found: {}",
                    field.ty.to_token_stream()
                );
            };
            let primitive = Ident::new(PRIMITIVE_TYPES[prim_index], field.ty.span());
            let atomic_prim = atomic_from_prim(prim_index, field.ty.span());

            let from_field = field.clone();
            let wrapper_name = input.ident.clone();
            let from_prim = move |prim: &Ident| {
                if let Some(field_name) = &from_field.ident {
                    quote! {
                        #wrapper_name {
                            #field_name: #prim
                        }
                    }
                } else {
                    quote! {
                        #wrapper_name(#prim)
                    }
                }
            };

            let to_prim = move |ident: &Ident| {
                if let Some(field_name) = &field.ident {
                    quote! {
                        #ident.#field_name
                    }
                } else {
                    quote! {
                        #ident.0
                    }
                }
            };

            (
                primitive,
                atomic_prim,
                Box::new(from_prim),
                Box::new(to_prim),
            )
        }
        syn::Data::Enum(_data_enum) => {
            let Some(repr_attr) = input
                .attrs
                .iter()
                .filter_map(|attr| match &attr.meta {
                    syn::Meta::List(list) => Some(list),
                    _ => None,
                })
                .find(|list| {
                    list.path
                        .get_ident()
                        .map(|id| id == "repr")
                        .unwrap_or(false)
                })
            else {
                panic!("AtomicWrapper on Enum requires the use of `#[repr(primitive)]'");
            };

            let Some(prim_index) = PRIMITIVE_TYPES.iter().position(|prim| {
                repr_attr
                    .tokens
                    .clone()
                    .into_iter()
                    .any(|token| token.to_string() == *prim)
            }) else {
                panic!("Could not find primitive in repr attribute");
            };

            let primitive = Ident::new(PRIMITIVE_TYPES[prim_index], repr_attr.span());
            let atomic_prim = atomic_from_prim(prim_index, repr_attr.span());

            let wrapper_name = input.ident.clone();
            let prim_from = primitive.clone();
            let from_prim = move |ident: &Ident| {
                quote! {
                    <#wrapper_name as core::convert::TryFrom<#prim_from>::try_from(#ident)
                        .unwrap()
                }
            };

            let prim_to = primitive.clone();
            let to_prim = move |ident: &Ident| {
                quote! {
                    #ident as #prim_to
                }
            };

            (
                primitive,
                atomic_prim,
                Box::new(from_prim),
                Box::new(to_prim),
            )
        }
        syn::Data::Union(_) => panic!("AtomicWrapper not supported for unions"),
    }
}

fn atomic_from_prim(prim_index: usize, span: Span) -> TokenStream {
    let atomic_prim = Ident::new(ATOMIC_TYPES[prim_index], span);
    let atomic_prim = quote_spanned! { span =>  core::sync::atomic::#atomic_prim };
    atomic_prim
}

fn get_field(data_struct: &DataStruct) -> Field {
    const FIELD_COUNT_ERROR: &str = "AtomicWrapper requires exactly 1 field";

    match &data_struct.fields {
        syn::Fields::Named(fields_named) => {
            assert_eq!(fields_named.named.len(), 1, "{FIELD_COUNT_ERROR}");
            fields_named.named.first().cloned().unwrap()
        }
        syn::Fields::Unnamed(fields_unnamed) => {
            assert_eq!(fields_unnamed.unnamed.len(), 1, "{FIELD_COUNT_ERROR}");
            fields_unnamed.unnamed.first().cloned().unwrap()
        }
        syn::Fields::Unit => panic!("{FIELD_COUNT_ERROR}"),
    }
}
