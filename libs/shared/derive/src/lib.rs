extern crate proc_macro;

mod atomic_wrapper;

use paste::paste;
use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, Expr, ExprLit, Lit, parse_macro_input};

#[proc_macro_derive(AtomicWrapper)]
pub fn derive_atomic_wrapper(input: TokenStream) -> TokenStream {
    atomic_wrapper::derive_atomic_wrapper(input)
}

// TODO replace paste with concat from rust. See: https://doc.rust-lang.org/stable/unstable-book/language-features/macro-metavar-expr-concat.html
// TODO support Other variant. That is valid over all unused discriminants
macro_rules! primitive_enum {
    ($type:ident) => {
        paste! {
        // TODO I coul prob create the doc using #[doc] and make $type work
        /// derive `TryFrom<Number>` for enum with primitive representation
        #[proc_macro_derive([<$type:camel Enum>])]
        pub fn [<derive_ $type _enum>](input: TokenStream) -> TokenStream {
            let DeriveInput { ident, data, .. } = parse_macro_input!(input);

            match data {
                syn::Data::Enum(data) => {
                    let mut last_discriminant = None;
                    let variants = data
                        .variants
                        .iter()
                        .map(|variant| {
                            let disc = if let Some(disc) = variant.discriminant.as_ref() {
                                let Expr::Lit(ExprLit{lit: Lit::Int(ref lit_int), ..}) = disc.1 else {
                                    panic!("$type enum only supports integer discriminants");
                                };
                                let disc = lit_int.base10_parse::<$type>().unwrap();
                                last_discriminant = Some(disc);
                                disc
                            } else {
                                let disc = last_discriminant.map(|l| l + 1).unwrap_or(0);
                                last_discriminant = Some(disc);
                                disc
                            };

                            let var = &variant.ident;
                            quote! {
                                #disc => Ok(#ident::#var)
                            }
                        });

                        let output = quote! {

                            impl TryFrom<$type> for #ident {
                                type Error = shared::primitive_enum::InvalidValue<$type>;

                                fn try_from(value: $type) -> Result<Self, Self::Error> {
                                    match value {
                                        #(#variants),*,
                                        v => Err(shared::primitive_enum::InvalidValue { value: v })
                                    }
                                }
                            }
                        };
                    output.into()
                }
                _ => {
                    panic!("$type enum is only allowed on enums");
                }
            }
        }
        }
    };
}

primitive_enum!(u8);
primitive_enum!(u16);
primitive_enum!(u32);
primitive_enum!(u64);

primitive_enum!(i8);
primitive_enum!(i16);
primitive_enum!(i32);
primitive_enum!(i64);
