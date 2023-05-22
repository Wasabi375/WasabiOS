extern crate proc_macro;

use paste::paste;
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

macro_rules! primitive_enum {
    ($type:ident) => {
        paste! {
        #[proc_macro_derive([<$type:camel Enum>])]
        pub fn [<derive_ $type _enum>](input: TokenStream) -> TokenStream {
            let DeriveInput { ident, data, .. } = parse_macro_input!(input);

            match data {
                syn::Data::Enum(data) => {
                    let variants = data
                        .variants
                        .iter()
                        .filter(|v| v.discriminant.is_some())
                        .map(|variant| {
                            let disc = &variant.discriminant.as_ref().unwrap().1;
                            let var = &variant.ident;
                            quote! {
                                #disc => Ok(#ident::#var)
                            }
                        });

                        let output = quote! {

                            impl TryFrom<$type> for #ident {
                                type Error = shared::primitive_enum::InvalidValue<$type>;

                                fn try_from(value: u8) -> Result<Self, Self::Error> {
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
                    panic!("U8Enum is only allowed on enums");
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
