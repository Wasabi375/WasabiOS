use proc_macro2::Ident;
use syn::{
    ext::IdentExt,
    parse::{Parse, ParseStream},
    Error, Expr, Result, Token,
};

pub struct Args {
    pub name: Option<Expr>,
    pub expected_exit: Option<Expr>,
}

impl Parse for Args {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut name = None;
        let mut expected_exit = None;

        while !input.is_empty() {
            if input.lookahead1().peek(Ident::peek_any) {
                let ident: Ident = input.parse()?;
                match ident.to_string().as_str() {
                    "name" => {
                        if name.is_some() {
                            return Err(Error::new_spanned(ident, "name can only set once"));
                        }
                        let _: Token!(:) = input.parse()?;
                        name = Some(input.parse()?);
                    }
                    "expected_exit" => {
                        if expected_exit.is_some() {
                            return Err(Error::new_spanned(
                                ident,
                                "expected_exit can only set once",
                            ));
                        }
                        let _: Token!(:) = input.parse()?;
                        expected_exit = Some(input.parse()?);
                    }
                    _ => {
                        return Err(Error::new_spanned(
                            ident,
                            "Expected `name` or `expected_exit`",
                        ))
                    }
                }
            } else {
                let expr: Expr = input.parse()?;

                if name.is_none() {
                    name = Some(expr);
                } else if expected_exit.is_none() {
                    expected_exit = Some(expr);
                } else {
                    return Err(Error::new_spanned(
                        expr,
                        "Name and expected_exit can only be set once",
                    ));
                }
            }
        }

        Ok(Args {
            name,
            expected_exit,
        })
    }
}
