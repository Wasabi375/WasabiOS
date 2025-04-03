use proc_macro2::{Ident, Punct};
use syn::{
    ext::IdentExt,
    parse::{Parse, ParseStream},
    Error, Expr, Result, Token,
};

#[derive(Debug)]
pub struct MultiTestArgs {
    pub cfg: Option<Expr>,
}

impl Parse for MultiTestArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut cfg = None;

        while !input.is_empty() {
            let lookahead = input.lookahead1();
            if lookahead.peek(Ident::peek_any) {
                let ident: Ident = input.parse()?;
                match ident.to_string().as_str() {
                    "cfg" => {
                        if cfg.is_some() {
                            return Err(Error::new_spanned(ident, "cfg can only be set once"));
                        }
                        let _: Token![:] = input.parse()?;
                        cfg = Some(input.parse()?);
                    }
                    _ => {
                        return Err(Error::new_spanned(
                            ident,
                            "Expected `name` or `expected_exit`",
                        ))
                    }
                }
            } else if lookahead.peek(Token![,]) {
                // skip comma
                let _: Punct = input.parse()?;
            } else {
                return Err(Error::new(input.span(), "Unexpected argument"));
            }
        }

        Ok(MultiTestArgs { cfg })
    }
}
